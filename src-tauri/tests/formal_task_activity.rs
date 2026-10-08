use std::collections::BTreeMap;
use std::sync::Arc;

use inxaiot_desk_buddy_lib::application::ports::task_event::{TaskEventInput, TaskEventSink};
use inxaiot_desk_buddy_lib::domain::common::task::{TargetState, TaskEventLevel, TaskState};
use inxaiot_desk_buddy_lib::formal::app_state::FormalAppState;
use inxaiot_desk_buddy_lib::formal::config::AppPaths;
use inxaiot_desk_buddy_lib::formal::job_supervisor::{JobOutcome, JobSupervisor};
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::formal::runtime_registry::ProjectRuntimeRegistry;
use inxaiot_desk_buddy_lib::formal::secret_store::MemorySecretStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskRepository,
};
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::interface::commands::task_activity::{
    query_activity_tasks, query_task_logs, request_task_cancel,
};
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

#[tokio::test]
async fn activity_dto_aggregates_real_snapshots_logs_and_running_task_cancel() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("ensure paths");
    let local_store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");
    sqlx::query(
        "INSERT INTO local_project \
         (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
          db_password_secret_ref, created_at, updated_at) \
         VALUES ('project-a', 'Project A', 'http://platform.test', 'db.test', 3306, \
                 'user', 'business', 'workbench', 'secret-ref', '1', '1')",
    )
    .execute(local_store.pool())
    .await
    .expect("project fixture");
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
        job_supervisor: job_supervisor.clone(),
        task_recovery_registry:
            inxaiot_desk_buddy_lib::infrastructure::task_handlers::built_in_recovery_registry(),
        task_handler_registry,
        task_queue,
        task_event_bus,
        task_repository: task_repository.clone(),
        task_event_pipeline: task_event_pipeline.clone(),
        paths,
    };
    task_repository
        .create(CreateTask {
            id: "task-a".into(),
            local_project_id: "project-a".into(),
            remote_operation_record_id: Some("operation-a".into()),
            domain_type: "aio".into(),
            operation_type: "full_upgrade".into(),
            name: "AIO full upgrade".into(),
            priority: 1,
            batch_size: 2,
            concurrency: 2,
            payload_ref: None,
            log_path: state
                .paths
                .project_task_log_path("project-a", "task-a")
                .expect("task log")
                .to_string_lossy()
                .into_owned(),
            targets: vec![("aio".into(), "A".into()), ("aio".into(), "B".into())],
        })
        .await
        .expect("create task");
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
    ] {
        task_repository
            .transition("task-a", expected, next, None, None)
            .await
            .expect("task transition");
    }
    task_repository
        .update_target(
            "task-a",
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: "A".into(),
                state: TargetState::Running,
                stage: "upload".into(),
                progress_current: 5,
                progress_total: 10,
                fencing_token: Some(1),
                message_code: None,
                message_params_json: None,
            },
        )
        .await
        .expect("target a");
    task_repository
        .update_target(
            "task-a",
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: "B".into(),
                state: TargetState::Succeeded,
                stage: "completed".into(),
                progress_current: 10,
                progress_total: 10,
                fencing_token: Some(2),
                message_code: None,
                message_params_json: None,
            },
        )
        .await
        .expect("target b");
    task_event_pipeline
        .emit(
            "task-a",
            TaskEventInput {
                resource_type: Some("aio".into()),
                resource_key: Some("A".into()),
                stage: "upload".into(),
                status: "running".into(),
                progress_current: Some(5),
                progress_total: Some(10),
                level: TaskEventLevel::Warn,
                message_code: "UPLOAD_RETRY".into(),
                message_params: BTreeMap::new(),
                message: Some("retry upload".into()),
            },
        )
        .await
        .expect("task event");
    let tasks = query_activity_tasks(&state, "project-a", 20)
        .await
        .expect("activity tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, "running");
    assert_eq!(tasks[0].stage, "upload");
    assert_eq!(tasks[0].progress, Some(75));
    assert_eq!(tasks[0].target_count, 2);
    assert_eq!(tasks[0].completed_count, 1);
    assert!(tasks[0].cancellable);
    let logs = query_task_logs(
        &state,
        "task-a",
        &["WARN".into()],
        Some("retry".into()),
        0,
        20,
        true,
    )
    .await
    .expect("activity logs");
    assert_eq!(logs.items.len(), 1);
    assert_eq!(logs.items[0].source, "A");
    assert_eq!(logs.items[0].level, "WARN");

    task_repository
        .create(CreateTask {
            id: "preflight-task".into(),
            local_project_id: "project-a".into(),
            remote_operation_record_id: None,
            domain_type: "aio".into(),
            operation_type: "deployment_preflight".into(),
            name: "部署检查 · 整包升级".into(),
            priority: 0,
            batch_size: 2,
            concurrency: 1,
            payload_ref: None,
            log_path: state
                .paths
                .project_task_log_path("project-a", "preflight-task")
                .expect("preflight log")
                .to_string_lossy()
                .into_owned(),
            targets: vec![
                ("preflight_internal".into(), "common".into()),
                ("aio".into(), "A".into()),
                ("aio".into(), "B".into()),
            ],
        })
        .await
        .expect("create preflight task");
    task_repository
        .initialize_target_progress("preflight-task", "等待检查", 1)
        .await
        .expect("initialize preflight progress");
    task_repository
        .transition(
            "preflight-task",
            TaskState::Draft,
            TaskState::Checking,
            None,
            None,
        )
        .await
        .expect("start preflight");
    for (key, target_state, stage) in [
        ("A", TargetState::Succeeded, "检查完成"),
        ("B", TargetState::Failed, "检查失败"),
    ] {
        task_repository
            .update_target(
                "preflight-task",
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: key.into(),
                    state: target_state,
                    stage: stage.into(),
                    progress_current: 1,
                    progress_total: 1,
                    fencing_token: None,
                    message_code: Some("PREFLIGHT_TARGET_RESULT".into()),
                    message_params_json: None,
                },
            )
            .await
            .expect("finish preflight target");
    }
    task_repository
        .update_target(
            "preflight-task",
            TargetUpdate {
                resource_type: "preflight_internal".into(),
                resource_key: "common".into(),
                state: TargetState::Succeeded,
                stage: "公共检查完成".into(),
                progress_current: 1,
                progress_total: 1,
                fencing_token: None,
                message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                message_params_json: None,
            },
        )
        .await
        .expect("finish common preflight");
    task_repository
        .transition(
            "preflight-task",
            TaskState::Checking,
            TaskState::Failed,
            Some("PREFLIGHT_BLOCKED"),
            Some("部署执行条件检查未通过"),
        )
        .await
        .expect("finish preflight");
    let preflight_event = task_event_pipeline
        .emit(
            "preflight-task",
            TaskEventInput {
                resource_type: Some("aio".into()),
                resource_key: Some("B".into()),
                stage: "检查失败".into(),
                status: "failed".into(),
                progress_current: Some(2),
                progress_total: Some(2),
                level: TaskEventLevel::Error,
                message_code: "PREFLIGHT_TARGET_FAILED".into(),
                message_params: BTreeMap::new(),
                message: Some("节点B检查失败：SSH连接超时".into()),
            },
        )
        .await
        .expect("preflight result log");
    let event_json = serde_json::to_value(preflight_event).expect("serialize preflight event");
    assert_eq!(event_json["localTaskId"], "preflight-task");
    assert_eq!(event_json["progressCurrent"], 2);
    assert_eq!(event_json["progressTotal"], 2);
    assert_eq!(event_json["stage"], "检查失败");
    let preflight = query_activity_tasks(&state, "project-a", 20)
        .await
        .expect("preflight activity")
        .into_iter()
        .find(|task| task.id == "preflight-task")
        .expect("preflight task dto");
    assert_eq!(preflight.state, "failed");
    assert_eq!(preflight.stage, "检查失败");
    assert_eq!(preflight.progress, Some(100));
    assert_eq!(preflight.completed_count, 2);
    assert!(!preflight.cancellable);
    let preflight_logs = query_task_logs(
        &state,
        "preflight-task",
        &["ERROR".into()],
        Some("SSH连接超时".into()),
        0,
        20,
        true,
    )
    .await
    .expect("preflight logs");
    assert_eq!(preflight_logs.items.len(), 1);
    assert_eq!(preflight_logs.items[0].source, "B");

    job_supervisor
        .spawn("task-a", |cancellation| async move {
            cancellation.cancelled().await;
            Err(inxaiot_desk_buddy_lib::core::error::AppError::Cancelled)
        })
        .await
        .expect("spawn running job");
    let cancelling = request_task_cancel(&state, "task-a")
        .await
        .expect("cancel task");
    assert_eq!(cancelling.state, "cancelling");
    assert!(!cancelling.cancellable);
    assert_eq!(
        job_supervisor.join("task-a").await.expect("join task"),
        JobOutcome::Cancelled
    );
    state
        .task_queue
        .shutdown(std::time::Duration::from_secs(1))
        .await;
    state.local_store.close().await;
}
