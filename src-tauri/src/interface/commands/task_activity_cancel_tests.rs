use super::request_task_cancel;
use crate::{
    core::error::AppError,
    domain::common::task::TaskState,
    formal::{app_state::FormalAppState, config::AppPaths, local_store::LocalStore,
        runtime_registry::ProjectRuntimeRegistry, secret_store::MemorySecretStore},
    infrastructure::{local_sqlite::task_repository::{CreateTask, TaskRepository},
        logging::{redactor::SensitiveValueRedactor, task_event_pipeline::TaskEventPipeline}},
    runtime::{event_bus::TaskEventBus, job_supervisor::{JobOutcome, JobSupervisor},
        task_queue::{TaskHandlerRegistry, TaskQueue}},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;

async fn cancel_while_event_lock_is_held(domain: &str, operation: &str) {
    let directory = tempfile::tempdir().unwrap();
    let paths = AppPaths::from_data_dir(directory.path()).unwrap();
    paths.ensure().unwrap();
    let store = LocalStore::open(&paths.local_db).await.unwrap();
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES('cancel-lock','取消锁回归','','',3306,'','','','','1','1')")
        .execute(store.pool()).await.unwrap();
    let repository = TaskRepository::new(store.pool().clone());
    let bus = TaskEventBus::new(16).unwrap();
    let pipeline = TaskEventPipeline::new(repository.clone(), bus.clone(), SensitiveValueRedactor::default());
    let supervisor = JobSupervisor::default();
    let registry = TaskHandlerRegistry::default();
    let queue = TaskQueue::start(8, 1, registry.clone(), supervisor.clone()).await.unwrap();
    let state = Arc::new(FormalAppState {
        paths, local_store: store, task_repository: repository.clone(), task_event_bus: bus,
        task_event_pipeline: pipeline.clone(), secret_store: Arc::new(MemorySecretStore::default()),
        runtime_registry: ProjectRuntimeRegistry::default(), job_supervisor: supervisor.clone(),
        task_handler_registry: registry, task_queue: queue,
        task_recovery_registry: crate::infrastructure::task_handlers::built_in_recovery_registry(),
    });
    let log = directory.path().join("task-a.jsonl");
    repository.create(CreateTask {
        id: "task-a".into(), local_project_id: "cancel-lock".into(), remote_operation_record_id: None,
        domain_type: domain.into(), operation_type: operation.into(), name: "在途取消".into(),
        priority: 0, batch_size: 1, concurrency: 1, payload_ref: None,
        log_path: log.to_string_lossy().into_owned(), targets: vec![(domain.into(), "target-a".into())],
    }).await.unwrap();
    for (from, to) in [(TaskState::Draft, TaskState::Checking), (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued), (TaskState::Queued, TaskState::Running)] {
        repository.transition("task-a", from, to, None, None).await.unwrap();
    }
    let cancelled = Arc::new(Notify::new());
    let safe_step_finished = Arc::new(Notify::new());
    supervisor.spawn("task-a", {
        let cancelled = cancelled.clone();
        let safe_step_finished = safe_step_finished.clone();
        move |token| async move {
            token.cancelled().await;
            cancelled.notify_one();
            // 已发出的设备安全步骤由执行器决定结束时机，取消接口不负责丢弃它。
            safe_step_finished.notified().await;
            Err(AppError::Cancelled)
        }
    }).await.unwrap();
    let guard = pipeline.hold_task_lock_for_test("task-a").await;
    let request = {
        let state = state.clone();
        tokio::spawn(async move { request_task_cancel(&state, "task-a").await })
    };
    tokio::time::timeout(Duration::from_secs(1), cancelled.notified()).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(4), request).await.unwrap().unwrap().unwrap();
    assert_eq!(result.state, "cancelling");
    assert_eq!(repository.get("task-a").await.unwrap().state, TaskState::Cancelling);
    assert_eq!(supervisor.active_count().await, 1);
    assert!(!log.exists(), "未取得日志锁时不能写入残缺日志");
    let repeated = tokio::time::timeout(Duration::from_secs(1), request_task_cancel(&state, "task-a"))
        .await.unwrap().unwrap();
    assert_eq!(repeated.state, "cancelling");
    drop(guard);
    safe_step_finished.notify_one();
    assert_eq!(supervisor.join("task-a").await.unwrap(), JobOutcome::Cancelled);
    state.task_queue.shutdown(Duration::from_secs(1)).await;
    state.local_store.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn screen_cancel_returns_while_same_task_event_lock_is_busy() {
    cancel_while_event_lock_is_held("smart_screen", "install").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn aio_cancel_keeps_existing_safe_step_and_returns_despite_busy_event_lock() {
    cancel_while_event_lock_is_held("aio", "full_upgrade").await;
}
