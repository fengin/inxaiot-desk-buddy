use inxaiot_desk_buddy_lib::formal;

use std::sync::Arc;

use formal::app_state::FormalAppState;
use formal::config::AppPaths;
use formal::error::FormalError;
use formal::job_supervisor::JobSupervisor;
use formal::local_store::LocalStore;
use formal::runtime_registry::{ConnectionHealth, ProjectRuntimeRegistry};
use formal::secret_store::{MemorySecretStore, SecretStore};
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::TaskRepository;
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};
use sqlx::Row;

#[tokio::test]
async fn local_store_runs_migrations_and_respects_data_boundary() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("create app paths");
    let store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");

    let rows = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
        .fetch_all(store.pool())
        .await
        .expect("list sqlite tables");
    let tables = rows
        .iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<Vec<_>>();
    for required in [
        "_sqlx_migrations",
        "local_project",
        "local_project_session",
        "local_preference",
        "local_task",
        "local_task_target",
        "local_task_step",
        "local_aio_import_session",
        "local_aio_import_item",
        "local_recent_artifact",
        "local_host_key",
        "local_secret_cleanup",
    ] {
        assert!(
            tables.iter().any(|table| table == required),
            "missing {required}"
        );
    }
    assert!(
        !tables
            .iter()
            .any(|table| table == "local_project_master_key")
    );
    assert!(!tables.iter().any(|table| table == "aio_node"));
    assert!(!tables.iter().any(|table| table == "operation_record"));
    assert!(!tables.iter().any(|table| table == "resource_lease"));

    let project_columns = sqlx::query("PRAGMA table_info(local_project)")
        .fetch_all(store.pool())
        .await
        .expect("project columns")
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("name").ok())
        .collect::<Vec<_>>();
    assert!(
        project_columns
            .iter()
            .any(|name| name == "db_password_secret_ref")
    );
    assert!(!project_columns.iter().any(|name| name == "db_password"));
    assert!(!project_columns.iter().any(|name| name == "access_token"));

    let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(store.pool())
        .await
        .expect("journal mode");
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(store.pool())
        .await
        .expect("foreign keys");
    assert_eq!(journal_mode.to_lowercase(), "wal");
    assert_eq!(foreign_keys, 1);
    store.close().await;
}

#[test]
fn app_paths_reject_directory_traversal() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    assert!(paths.project_task_dir("project-a", "task-a").is_ok());
    assert!(
        paths
            .project_task_log_path("project-a", "task-a")
            .expect("task log")
            .starts_with(&paths.task_logs_dir)
    );
    assert!(
        paths
            .project_task_dir("project-a", "task-a")
            .expect("task artifacts")
            .starts_with(&paths.task_artifacts_dir)
    );
    assert!(paths.project_task_dir("../project-a", "task-a").is_err());
    assert!(paths.project_task_dir("project-a", "..\\task-a").is_err());
}

#[test]
fn error_dto_has_stable_code_message_key_and_trace_id() {
    let dto = FormalError::InvalidConfig("bad".into()).to_dto();
    assert_eq!(dto.code, "INVALID_CONFIG");
    assert_eq!(dto.message_key, "error.invalid_config");
    assert!(!dto.trace_id.is_empty());
}

#[test]
fn memory_secret_store_round_trip_and_delete() {
    let store = MemorySecretStore::default();
    store.save("project-a/db", b"secret").expect("save secret");
    assert_eq!(store.load("project-a/db").expect("load secret"), b"secret");
    store.delete("project-a/db").expect("delete secret");
    assert!(store.load("project-a/db").is_err());
}

#[tokio::test]
async fn runtime_registry_is_per_project_and_reuses_open_runtime() {
    let registry = ProjectRuntimeRegistry::default();
    let first = registry.open("project-a").await.expect("open project a");
    let same = registry
        .open("project-a")
        .await
        .expect("open project a again");
    let other = registry.open("project-b").await.expect("open project b");
    assert!(Arc::ptr_eq(&first, &same));
    assert!(!Arc::ptr_eq(&first, &other));
    first.set_health(ConnectionHealth::Ready).await;
    assert_eq!(same.health().await, ConnectionHealth::Ready);
    assert_eq!(registry.len().await, 2);
    registry.close("project-a").await.expect("close project a");
    assert!(registry.get("project-a").await.is_none());
    let reopened = registry.open("project-a").await.expect("reopen project a");
    assert!(!Arc::ptr_eq(&first, &reopened));
    assert_eq!(reopened.health().await, ConnectionHealth::Connecting);
}

#[tokio::test]
async fn job_supervisor_owns_cancellation_tokens() {
    let jobs = JobSupervisor::default();
    let token = jobs.register("task-a").await.expect("register task");
    assert_eq!(jobs.active_count().await, 1);
    assert!(jobs.register("task-a").await.is_err());
    jobs.cancel("task-a").await.expect("cancel task");
    assert!(token.is_cancelled());
    assert!(jobs.finish("task-a").await);
    assert_eq!(jobs.active_count().await, 0);
}

#[tokio::test]
async fn app_state_remains_lightweight_and_composed_from_ports() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("ensure paths");
    let local_store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");
    let task_event_bus = TaskEventBus::new(32).expect("task event bus");
    let task_repository = TaskRepository::new(local_store.pool().clone());
    let task_event_pipeline = TaskEventPipeline::new(
        task_repository.clone(),
        task_event_bus.clone(),
        SensitiveValueRedactor::default(),
    );
    let job_supervisor = JobSupervisor::default();
    let task_handler_registry = TaskHandlerRegistry::default();
    let task_queue = TaskQueue::start(8, 2, task_handler_registry.clone(), job_supervisor.clone())
        .await
        .expect("task queue");
    let state = FormalAppState {
        local_store,
        secret_store: Arc::new(MemorySecretStore::default()),
        runtime_registry: ProjectRuntimeRegistry::default(),
        job_supervisor,
        task_handler_registry,
        task_queue,
        task_event_bus,
        task_repository,
        task_event_pipeline,
        paths,
    };
    assert_eq!(state.runtime_registry.len().await, 0);
    assert_eq!(state.job_supervisor.active_count().await, 0);
    assert!(state.paths.logs_dir.is_dir());
    state
        .task_queue
        .shutdown(std::time::Duration::from_secs(1))
        .await;
    state.local_store.close().await;
}

#[test]
#[ignore = "writes and deletes one isolated Windows Credential Manager entry"]
fn windows_credential_manager_round_trip() {
    let store = formal::secret_store::OsSecretStore::new("inxaiot-desk-buddy-poc")
        .expect("os secret store");
    let reference = format!("phase1-{}", uuid::Uuid::now_v7());
    store
        .save(&reference, b"temporary-secret")
        .expect("save os secret");
    assert_eq!(
        store.load(&reference).expect("load os secret"),
        b"temporary-secret"
    );
    store.delete(&reference).expect("delete os secret");
}
