use inxaiot_desk_buddy_lib::formal;

use std::sync::Arc;

use formal::app_state::FormalAppState;
use formal::config::AppPaths;
use formal::job_supervisor::JobSupervisor;
use formal::local_store::LocalStore;
use formal::runtime_registry::ProjectRuntimeRegistry;
use formal::secret_store::MemorySecretStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::TaskRepository;
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

async fn shutdown(state: &FormalAppState) {
    state
        .task_queue
        .shutdown(std::time::Duration::from_secs(1))
        .await;
    state.runtime_registry.close_all().await;
    state.local_store.close().await;
}

#[tokio::test]
async fn application_shutdown_closes_project_runtimes_and_sqlite_pool() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("ensure app paths");
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
        task_recovery_registry:
            inxaiot_desk_buddy_lib::infrastructure::task_handlers::built_in_recovery_registry(),
        task_handler_registry,
        task_queue,
        task_event_bus,
        task_repository,
        task_event_pipeline,
        paths,
    };
    state
        .runtime_registry
        .open("project-a")
        .await
        .expect("open runtime a");
    state
        .runtime_registry
        .open("project-b")
        .await
        .expect("open runtime b");
    assert_eq!(state.runtime_registry.len().await, 2);
    shutdown(&state).await;
    assert_eq!(state.runtime_registry.len().await, 0);
    assert!(state.local_store.pool().is_closed());
}
