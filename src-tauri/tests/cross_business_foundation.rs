use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

#[path = "common/project_test_config.rs"]
mod project_test_config;

use inxaiot_desk_buddy_lib::application::ports::project_access::ProjectAccessPort;
use inxaiot_desk_buddy_lib::application::ports::project_management::ProjectManagementPort;
use inxaiot_desk_buddy_lib::application::project_access::ProjectAccessRequirement;
use inxaiot_desk_buddy_lib::core::error::{AppError, AppResult};
use inxaiot_desk_buddy_lib::domain::common::project::ProjectInput;
use inxaiot_desk_buddy_lib::domain::common::task::{TargetState, TaskRecord, TaskState};
use inxaiot_desk_buddy_lib::formal::{
    app_state::FormalAppState, config::AppPaths, local_store::LocalStore,
    runtime_registry::ProjectRuntimeRegistry, secret_store::MemorySecretStore,
};
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskRepository,
};
use inxaiot_desk_buddy_lib::infrastructure::logging::{
    redactor::SensitiveValueRedactor, task_event_pipeline::TaskEventPipeline,
};
use inxaiot_desk_buddy_lib::infrastructure::stage75_adapter::Stage75Adapter;
use inxaiot_desk_buddy_lib::infrastructure::task_recovery::{
    TaskRecoveryFuture, TaskRecoveryOutcome,
};
use inxaiot_desk_buddy_lib::infrastructure::task_runtime::retry_pending_local_finalization;
use inxaiot_desk_buddy_lib::runtime::{
    event_bus::TaskEventBus,
    job_supervisor::JobSupervisor,
    task_queue::{TaskEnvelope, TaskHandlerRegistry, TaskQueue},
};

async fn state_at(path: &std::path::Path) -> FormalAppState {
    let paths = AppPaths::from_data_dir(path).unwrap();
    paths.ensure().unwrap();
    let local_store = LocalStore::open(&paths.local_db).await.unwrap();
    let task_repository = TaskRepository::new(local_store.pool().clone());
    let task_event_bus = TaskEventBus::new(64).unwrap();
    let task_event_pipeline = TaskEventPipeline::new(
        task_repository.clone(),
        task_event_bus.clone(),
        SensitiveValueRedactor::default(),
    );
    let job_supervisor = JobSupervisor::default();
    let task_handler_registry = TaskHandlerRegistry::default();
    let task_queue = TaskQueue::start(8, 1, task_handler_registry.clone(), job_supervisor.clone())
        .await
        .unwrap();
    FormalAppState {
        paths,
        local_store,
        task_repository,
        task_event_bus,
        task_event_pipeline,
        secret_store: Arc::new(MemorySecretStore::default()),
        runtime_registry: ProjectRuntimeRegistry::default(),
        job_supervisor,
        task_handler_registry,
        task_queue,
        task_recovery_registry: Default::default(),
    }
}

fn local_input() -> ProjectInput {
    ProjectInput {
        name: "本机设备维护".into(),
        platform_url: String::new(),
        db_host: String::new(),
        db_port: 3306,
        db_user: String::new(),
        db_tls_enabled: false,
        db_password: None,
        business_db: String::new(),
        workbench_db: "inxaiot_desk_buddy".into(),
    }
}

fn task(state: &FormalAppState, project: &str, id: &str, operation: &str) -> CreateTask {
    CreateTask {
        id: id.into(),
        local_project_id: project.into(),
        remote_operation_record_id: None,
        domain_type: "test_device".into(),
        operation_type: operation.into(),
        name: "第二业务测试任务".into(),
        priority: 0,
        batch_size: 2,
        concurrency: 1,
        payload_ref: None,
        log_path: state
            .paths
            .project_task_log_path(project, id)
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        targets: vec![
            ("local_device".into(), "local-1".into()),
            ("platform_device".into(), "platform-1".into()),
        ],
    }
}

fn passed(kind: &str, key: &str) -> TargetUpdate {
    TargetUpdate {
        resource_type: kind.into(),
        resource_key: key.into(),
        state: TargetState::Succeeded,
        stage: "检查完成".into(),
        progress_current: 1,
        progress_total: 1,
        fencing_token: None,
        message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
        message_params_json: None,
    }
}

async fn checked(state: &FormalAppState, project: &str, id: &str) {
    let mut input = task(state, project, id, "device_preflight");
    input
        .targets
        .push(("preflight_internal".into(), "common".into()));
    state.task_repository.create(input.clone()).await.unwrap();
    state
        .task_repository
        .transition(id, TaskState::Draft, TaskState::Checking, None, None)
        .await
        .unwrap();
    for (kind, key) in input.targets {
        state
            .task_repository
            .update_target(id, passed(&kind, &key))
            .await
            .unwrap();
    }
    state
        .task_repository
        .bind_preflight_snapshot(id, project, "device_preflight", &"a".repeat(64))
        .await
        .unwrap();
    state
        .task_repository
        .transition(id, TaskState::Checking, TaskState::Succeeded, None, None)
        .await
        .unwrap();
}

async fn close(state: FormalAppState) {
    state.task_queue.shutdown(Duration::from_secs(2)).await;
    state.local_store.close().await;
}

#[tokio::test]
#[ignore = "uses the authorized platform and creates/drops one isolated empty business schema"]
async fn platform_login_and_reads_do_not_require_workbench_or_aio_tables()
-> Result<(), Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::domain::common::project::PlatformLoginRequest;
    use inxaiot_desk_buddy_lib::infrastructure::{
        platform_aio::require_aio_schema, project_context::project_pools,
    };
    let database = project_test_config::database();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let text = std::fs::read_to_string(root.join("test/测试数据说明.txt"))?;
    let value = |label: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(label))
            .map(str::trim)
            .unwrap()
    };
    let start = text.find('{').unwrap();
    let end = start + text[start..].find('}').unwrap() + 1;
    let login: serde_json::Value = serde_json::from_str(&text[start..end])?;
    let business = format!(
        "inxaiot_desk_buddy_cap_{}",
        &uuid::Uuid::now_v7().simple().to_string()[..12]
    );
    let missing_workbench = format!("{business}_ops");
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&database.host)
        .port(database.port)
        .username(&database.username)
        .password(&database.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled);
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    sqlx::query(&format!(
        "CREATE DATABASE `{business}` CHARACTER SET utf8mb4"
    ))
    .execute(&admin)
    .await?;
    let temp = tempfile::tempdir()?;
    let state = state_at(temp.path()).await;
    let result: AppResult<()> = async {
        let adapter = Stage75Adapter::new(&state);
        let id = adapter
            .create_project(ProjectInput {
                name: "独立平台读取验收".into(),
                platform_url: format!("http://{}", value("平台API：")),
                db_host: database.host,
                db_port: database.port,
                db_user: database.username,
                db_tls_enabled: false,
                db_password: Some(database.password),
                business_db: business.clone(),
                workbench_db: missing_workbench,
            })
            .await?
            .project
            .id;
        adapter
            .login_project(
                &id,
                PlatformLoginRequest {
                    username: login["principal"].as_str().unwrap().into(),
                    password: login["credentials"].as_str().unwrap().into(),
                    session_uuid: login["sessionUUID"].as_str().unwrap().into(),
                    image_code: login["imageCode"].as_str().unwrap().into(),
                },
            )
            .await?;
        adapter
            .require_project_access(&id, ProjectAccessRequirement::PlatformRead)
            .await?;
        let pools = project_pools(&state, &id).await?;
        if require_aio_schema(&pools.platform).await.is_ok() {
            return Err(AppError::Conflict("空数据库不应通过一体机表检查".into()));
        }
        if adapter
            .require_project_access(&id, ProjectAccessRequirement::Ready)
            .await
            .is_ok()
        {
            return Err(AppError::Conflict("共享库未建立时不能执行共享操作".into()));
        }
        adapter
            .require_project_access(&id, ProjectAccessRequirement::PlatformRead)
            .await?;
        Ok(())
    }
    .await;
    state.runtime_registry.close_all().await;
    close(state).await;
    sqlx::query(&format!("DROP DATABASE `{business}`"))
        .execute(&admin)
        .await?;
    admin.close().await;
    result?;
    Ok(())
}

#[tokio::test]
async fn queued_second_business_can_be_cancelled_without_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_at(temp.path()).await;
    let project = Stage75Adapter::new(&state)
        .create_project(local_input())
        .await
        .unwrap()
        .project
        .id;
    for id in ["blocking-task", "cancel-before-start"] {
        state
            .task_repository
            .create(task(&state, &project, id, "wait"))
            .await
            .unwrap();
        for (from, to) in [
            (TaskState::Draft, TaskState::Checking),
            (TaskState::Checking, TaskState::Ready),
            (TaskState::Ready, TaskState::Queued),
        ] {
            state
                .task_repository
                .transition(id, from, to, None, None)
                .await
                .unwrap();
        }
    }
    let started = Arc::new(tokio::sync::Notify::new());
    let started_handler = started.clone();
    let counter = Arc::new(AtomicUsize::new(0));
    let calls = counter.clone();
    let repository = state.task_repository.clone();
    state
        .task_handler_registry
        .register("test_device", "wait", move |envelope, cancellation| {
            let started = started_handler.clone();
            let calls = calls.clone();
            let repository = repository.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                repository
                    .transition(
                        &envelope.local_task_id,
                        TaskState::Queued,
                        TaskState::Running,
                        None,
                        None,
                    )
                    .await?;
                started.notify_one();
                cancellation.cancelled().await;
                Err(AppError::Cancelled)
            }
        })
        .unwrap();
    for id in ["blocking-task", "cancel-before-start"] {
        state
            .task_queue
            .enqueue(TaskEnvelope {
                local_task_id: id.into(),
                local_project_id: project.clone(),
                domain_type: "test_device".into(),
                operation_type: "wait".into(),
                resource_keys: vec!["local-1".into()],
                priority: 0,
                payload_ref: None,
                payload_sha256: None,
            })
            .await
            .unwrap();
        if id == "blocking-task" {
            tokio::time::timeout(Duration::from_secs(3), started.notified())
                .await
                .unwrap();
        }
    }
    let cancelled =
        inxaiot_desk_buddy_lib::interface::commands::task_activity::request_task_cancel(
            &state,
            "cancel-before-start",
        )
        .await
        .unwrap();
    assert_eq!(cancelled.state, "cancelled");
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    close(state).await;
}

#[tokio::test]
async fn local_project_works_without_any_remote_connection_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_at(temp.path()).await;
    let input = local_input();
    input.validate_for_create().unwrap();
    assert!(input.validate_for_test(None).is_err());
    let adapter = Stage75Adapter::new(&state);
    let project = adapter.create_project(input).await.unwrap();
    let id = project.project.id;
    adapter.switch_project(&id).await.unwrap();
    adapter
        .require_project_access(&id, ProjectAccessRequirement::Configured)
        .await
        .unwrap();
    for requirement in [
        ProjectAccessRequirement::ActiveSession,
        ProjectAccessRequirement::PlatformRead,
        ProjectAccessRequirement::Ready,
    ] {
        assert!(
            adapter
                .require_project_access(&id, requirement)
                .await
                .is_err()
        );
    }
    assert!(state.runtime_registry.get(&id).await.is_none());
    let secret_ref: String =
        sqlx::query_scalar("SELECT db_password_secret_ref FROM local_project WHERE id = ?")
            .bind(&id)
            .fetch_one(state.local_store.pool())
            .await
            .unwrap();
    assert!(secret_ref.is_empty());
    close(state).await;
    let reopened = state_at(temp.path()).await;
    let project = Stage75Adapter::new(&reopened)
        .switch_project(&id)
        .await
        .unwrap();
    assert!(project.project.platform_url.is_empty());
    assert!(reopened.runtime_registry.get(&id).await.is_none());
    Stage75Adapter::new(&reopened)
        .delete_project(&id)
        .await
        .unwrap();
    close(reopened).await;
}

#[tokio::test]
async fn second_business_checks_mixed_targets_once_and_executes_in_shared_queue() {
    let temp = tempfile::tempdir().unwrap();
    let state = state_at(temp.path()).await;
    let project = Stage75Adapter::new(&state)
        .create_project(local_input())
        .await
        .unwrap()
        .project
        .id;
    checked(&state, &project, "check-1").await;
    assert_eq!(
        state
            .task_repository
            .clear_terminal_for_project(&project)
            .await
            .unwrap(),
        0
    );
    let activity =
        inxaiot_desk_buddy_lib::interface::commands::task_activity::query_activity_tasks(
            &state, &project, 20,
        )
        .await
        .unwrap();
    assert_eq!(activity[0].target_count, 2);
    assert!(!activity[0].clearable);
    let input = task(&state, &project, "execute-1", "inspect");
    let mut wrong_domain = input.clone();
    wrong_domain.domain_type = "aio".into();
    assert!(
        state
            .task_repository
            .create_queued_from_preflight(
                "check-1",
                "device_preflight",
                &"a".repeat(64),
                wrong_domain
            )
            .await
            .is_err()
    );
    assert!(
        state
            .task_repository
            .create_queued_from_preflight(
                "check-1",
                "deployment_preflight",
                &"a".repeat(64),
                input.clone()
            )
            .await
            .is_err()
    );
    state
        .task_repository
        .create_queued_from_preflight(
            "check-1",
            "device_preflight",
            &"a".repeat(64),
            input.clone(),
        )
        .await
        .unwrap();
    let mut repeated = input;
    repeated.id = "execute-duplicate".into();
    assert!(
        state
            .task_repository
            .create_queued_from_preflight("check-1", "device_preflight", &"a".repeat(64), repeated)
            .await
            .is_err()
    );
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let repository = state.task_repository.clone();
    state
        .task_handler_registry
        .register("test_device", "inspect", move |envelope, _| {
            let repository = repository.clone();
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                repository
                    .transition(
                        &envelope.local_task_id,
                        TaskState::Queued,
                        TaskState::Running,
                        None,
                        None,
                    )
                    .await?;
                for (kind, key) in [
                    ("local_device", "local-1"),
                    ("platform_device", "platform-1"),
                ] {
                    repository
                        .update_target(&envelope.local_task_id, passed(kind, key))
                        .await?;
                }
                repository
                    .transition(
                        &envelope.local_task_id,
                        TaskState::Running,
                        TaskState::Succeeded,
                        None,
                        None,
                    )
                    .await?;
                Ok(())
            }
        })
        .unwrap();
    let mut results = state.task_queue.subscribe_results();
    state
        .task_queue
        .enqueue(TaskEnvelope {
            local_task_id: "execute-1".into(),
            local_project_id: project,
            domain_type: "test_device".into(),
            operation_type: "inspect".into(),
            resource_keys: vec!["local-1".into(), "platform-1".into()],
            priority: 0,
            payload_ref: None,
            payload_sha256: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), results.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let result = state.task_repository.get("execute-1").await.unwrap();
    assert_eq!(result.state, TaskState::Succeeded);
    assert!(result.remote_operation_record_id.is_none());
    close(state).await;
}

fn recover_local<'a>(
    state: &'a FormalAppState,
    task: &'a TaskRecord,
    _: bool,
) -> TaskRecoveryFuture<'a> {
    Box::pin(async move {
        state
            .task_repository
            .finalize_projection(
                &task.id,
                TaskState::FinalizingFailed,
                TaskState::Succeeded,
                &[
                    passed("local_device", "local-1"),
                    passed("platform_device", "platform-1"),
                ],
                &[],
            )
            .await?;
        Ok(TaskRecoveryOutcome::Completed)
    })
}

#[tokio::test]
async fn restart_routes_pending_results_by_business_without_reexecuting_devices() -> AppResult<()> {
    let temp = tempfile::tempdir().unwrap();
    let state = state_at(temp.path()).await;
    let project = Stage75Adapter::new(&state)
        .create_project(local_input())
        .await?
        .project
        .id;
    state
        .task_repository
        .create(task(&state, &project, "pending-1", "inspect"))
        .await?;
    for (from, to) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
        (TaskState::Running, TaskState::FinalizingFailed),
    ] {
        state
            .task_repository
            .transition("pending-1", from, to, None, None)
            .await?;
    }
    close(state).await;
    let state = state_at(temp.path()).await;
    Stage75Adapter::new(&state).switch_project(&project).await?;
    // 未注册的业务必须保留原记录，不能尝试读取一体机部署文件。
    assert!(matches!(
        retry_pending_local_finalization(&state, "pending-1", false).await,
        Err(AppError::NotFound(_))
    ));
    assert_eq!(
        state.task_repository.get("pending-1").await?.state,
        TaskState::FinalizingFailed
    );
    state
        .task_recovery_registry
        .register("test_device", "inspect", recover_local)?;
    assert!(
        state
            .task_recovery_registry
            .register("test_device", "inspect", recover_local)
            .is_err()
    );
    assert!(matches!(
        retry_pending_local_finalization(&state, "pending-1", false).await?,
        TaskRecoveryOutcome::Completed
    ));
    assert_eq!(
        state.task_repository.get("pending-1").await?.state,
        TaskState::Succeeded
    );
    assert!(
        retry_pending_local_finalization(&state, "pending-1", false)
            .await
            .is_err()
    );
    assert!(state.runtime_registry.get(&project).await.is_none());
    close(state).await;
    Ok(())
}
