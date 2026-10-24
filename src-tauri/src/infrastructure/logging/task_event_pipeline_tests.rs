use super::*;
use crate::domain::common::task::TaskEventLevel;
use crate::formal::local_store::LocalStore;
use crate::infrastructure::local_sqlite::task_repository::CreateTask;
use std::collections::BTreeMap;

async fn setup() -> (tempfile::TempDir, LocalStore, TaskEventPipeline) {
    let directory = tempfile::tempdir().unwrap();
    let store = LocalStore::open(directory.path().join("local.db")).await.unwrap();
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES('event-lock','事件锁回归','','',3306,'','','','','1','1')")
        .execute(store.pool()).await.unwrap();
    let repository = TaskRepository::new(store.pool().clone());
    for id in ["task-a", "task-b"] {
        repository.create(CreateTask {
            id: id.into(), local_project_id: "event-lock".into(), remote_operation_record_id: None,
            domain_type: "aio".into(), operation_type: "full_upgrade".into(), name: "取消事件回归".into(),
            priority: 0, batch_size: 1, concurrency: 1, payload_ref: None,
            log_path: directory.path().join(format!("{id}.jsonl")).to_string_lossy().into_owned(),
            targets: vec![("aio".into(), id.into())],
        }).await.unwrap();
    }
    let pipeline = TaskEventPipeline::new(repository, TaskEventBus::new(16).unwrap(), SensitiveValueRedactor::default());
    (directory, store, pipeline)
}

fn input() -> TaskEventInput {
    TaskEventInput {
        resource_type: None, resource_key: None, stage: "cancelling".into(), status: "cancelling".into(),
        progress_current: None, progress_total: None, level: TaskEventLevel::Warn,
        message_code: "TASK_CANCELLING".into(), message_params: BTreeMap::new(),
        message: Some("用户请求取消，等待当前安全步骤结束".into()),
    }
}

#[tokio::test]
async fn waiting_for_event_lock_is_bounded_without_sequence_or_partial_log() {
    let (directory, store, pipeline) = setup().await;
    let mut events = pipeline.event_bus().subscribe();
    let guard = pipeline.hold_task_lock_for_test("task-a").await;
    let outcome = tokio::time::timeout(Duration::from_secs(1),
        pipeline.emit_with_lock_timeout("task-a", input(), Duration::from_millis(25))).await.unwrap();
    assert!(matches!(outcome, Err(AppError::Timeout { operation: "等待任务事件日志锁" })));
    assert_eq!(pipeline.repository.get("task-a").await.unwrap().sequence, 0);
    assert!(!directory.path().join("task-a.jsonl").exists());
    assert!(matches!(events.try_recv(), Err(tokio::sync::broadcast::error::TryRecvError::Empty)));
    drop(guard);
    let event = pipeline.emit_with_lock_timeout("task-a", input(), Duration::from_secs(1)).await.unwrap();
    assert_eq!(event.sequence, 1);
    let raw = tokio::fs::read_to_string(directory.path().join("task-a.jsonl")).await.unwrap();
    assert!(raw.ends_with('\n'));
    assert_eq!(raw.lines().count(), 1);
    let logged: TaskEvent = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(logged.sequence, event.sequence);
    assert_eq!(events.recv().await.unwrap().event_id, event.event_id);
    store.close().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acquired_lock_does_not_timeout_halfway_through_persistent_write() {
    let (directory, store, pipeline) = setup().await;
    let writer = store.pool().begin_with("BEGIN IMMEDIATE").await.unwrap();
    let lock = pipeline.task_lock("task-a").unwrap();
    let worker = {
        let pipeline = pipeline.clone();
        tokio::spawn(async move {
            pipeline.emit_with_lock_timeout("task-a", input(), Duration::from_millis(20)).await
        })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        while lock.try_lock().is_ok() { tokio::task::yield_now().await; }
    }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!worker.is_finished(), "等待上限只能作用于取得事件锁，不能取消已经开始的持久化");
    writer.commit().await.unwrap();
    let event = tokio::time::timeout(Duration::from_secs(2), worker).await.unwrap().unwrap().unwrap();
    assert_eq!(event.sequence, 1);
    let raw = tokio::fs::read_to_string(directory.path().join("task-a.jsonl")).await.unwrap();
    assert_eq!(raw.lines().count(), 1);
    assert!(raw.ends_with('\n'));
    let logged: TaskEvent = serde_json::from_str(raw.trim()).unwrap();
    assert_eq!(logged.event_id, event.event_id);
    store.close().await;
}

#[tokio::test]
async fn held_task_event_lock_does_not_block_another_business_task() {
    let (directory, store, pipeline) = setup().await;
    let guard = pipeline.hold_task_lock_for_test("task-a").await;
    let other = tokio::time::timeout(Duration::from_secs(1),
        pipeline.emit_with_lock_timeout("task-b", input(), Duration::from_millis(25))).await.unwrap().unwrap();
    assert_eq!(other.sequence, 1);
    assert!(directory.path().join("task-b.jsonl").exists());
    assert_eq!(pipeline.repository.get("task-a").await.unwrap().sequence, 0);
    drop(guard);
    store.close().await;
}
