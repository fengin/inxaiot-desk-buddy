use std::collections::BTreeMap;

use inxaiot_desk_buddy_lib::application::ports::task_event::{TaskEventInput, TaskEventSink};
use inxaiot_desk_buddy_lib::application::ports::task_log::{TaskLogQuery, TaskLogStore};
use inxaiot_desk_buddy_lib::domain::common::task::TaskEventLevel;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TaskRepository,
};
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;

#[tokio::test]
async fn concurrent_events_have_monotonic_sequence_bounded_bus_paging_and_redaction() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    sqlx::query(
        "INSERT INTO local_project \
         (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
          db_password_secret_ref, created_at, updated_at) \
         VALUES ('project-a', 'Project A', 'http://platform.test', 'db.test', 3306, \
                 'user', 'business', 'workbench', 'secret-ref', '1', '1')",
    )
    .execute(store.pool())
    .await
    .expect("project fixture");
    let log_path = temp
        .path()
        .join("projects/project-a/tasks/task-a/events.jsonl");
    let repository = TaskRepository::new(store.pool().clone());
    repository
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
            log_path: log_path.to_string_lossy().into_owned(),
            targets: vec![("aio".into(), "A".into())],
        })
        .await
        .expect("create task");
    let pipeline = TaskEventPipeline::new(
        repository.clone(),
        TaskEventBus::new(64).expect("event bus"),
        SensitiveValueRedactor::new(["very-secret-token".into(), "private-key-body".into()]),
    );
    let mut receiver = pipeline.event_bus().subscribe();
    let mut handles = Vec::new();
    for index in 0..20 {
        let pipeline = pipeline.clone();
        handles.push(tokio::spawn(async move {
            pipeline
                .emit(
                    "task-a",
                    TaskEventInput {
                        resource_type: Some("aio".into()),
                        resource_key: Some("A".into()),
                        stage: "upload".into(),
                        status: "running".into(),
                        progress_current: Some(index),
                        progress_total: Some(20),
                        level: TaskEventLevel::Info,
                        message_code: "UPLOAD_PROGRESS".into(),
                        message_params: BTreeMap::from([("index".into(), index.to_string())]),
                        message: Some(format!("uploaded {index}")),
                    },
                )
                .await
                .expect("emit task event")
        }));
    }
    let mut emitted = Vec::new();
    for handle in handles {
        emitted.push(handle.await.expect("event task"));
    }
    emitted.sort_by_key(|event| event.sequence);
    assert_eq!(
        emitted
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        (1_u64..=20).collect::<Vec<_>>()
    );
    let sensitive = pipeline
        .emit(
            "task-a",
            TaskEventInput {
                resource_type: Some("aio".into()),
                resource_key: Some("A".into()),
                stage: "service_check".into(),
                status: "failed".into(),
                progress_current: None,
                progress_total: None,
                level: TaskEventLevel::Warn,
                message_code: "SERVICE_CHECK_FAILED".into(),
                message_params: BTreeMap::from([
                    ("authKey".into(), "very-secret-token".into()),
                    ("reason".into(), "private-key-body unavailable".into()),
                ]),
                message: Some("token=very-secret-token".into()),
            },
        )
        .await
        .expect("emit redacted event");
    assert_eq!(sensitive.sequence, 21);
    assert_eq!(sensitive.message_params["authKey"], "[REDACTED]");
    assert_eq!(sensitive.message.as_deref(), Some("token=[REDACTED]"));
    for _ in 0..21 {
        receiver.recv().await.expect("broadcast event");
    }

    let first_page = pipeline
        .log_store()
        .read_page(
            &log_path,
            &TaskLogQuery {
                levels: Vec::new(),
                keyword: None,
                offset: 0,
                limit: 5,
                newest_first: false,
            },
        )
        .await
        .expect("first log page");
    assert_eq!(first_page.items.len(), 5);
    assert_eq!(first_page.next_offset, 5);
    assert!(first_page.has_more);
    assert!(
        first_page
            .items
            .windows(2)
            .all(|items| items[0].sequence < items[1].sequence)
    );
    let latest_page = pipeline
        .log_store()
        .read_page(
            &log_path,
            &TaskLogQuery {
                levels: Vec::new(),
                keyword: None,
                offset: 0,
                limit: 5,
                newest_first: true,
            },
        )
        .await
        .expect("latest log page");
    assert_eq!(
        latest_page
            .items
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![17, 18, 19, 20, 21]
    );
    assert!(latest_page.has_more);
    let older_page = pipeline
        .log_store()
        .read_page(
            &log_path,
            &TaskLogQuery {
                levels: Vec::new(),
                keyword: None,
                offset: latest_page.next_offset,
                limit: 5,
                newest_first: true,
            },
        )
        .await
        .expect("older log page");
    assert_eq!(
        older_page
            .items
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        vec![12, 13, 14, 15, 16]
    );
    let warn_page = pipeline
        .log_store()
        .read_page(
            &log_path,
            &TaskLogQuery {
                levels: vec![TaskEventLevel::Warn],
                keyword: Some("failed".into()),
                offset: 0,
                limit: 20,
                newest_first: false,
            },
        )
        .await
        .expect("warn log page");
    assert_eq!(warn_page.items, vec![sensitive]);
    let raw_log = tokio::fs::read_to_string(&log_path)
        .await
        .expect("raw task log");
    assert!(!raw_log.contains("very-secret-token"));
    assert!(!raw_log.contains("private-key-body"));
    assert!(raw_log.contains("[REDACTED]"));
    assert_eq!(repository.get("task-a").await.expect("task").sequence, 21);
    store.close().await;
}

#[test]
fn redactor_debug_never_contains_registered_secrets() {
    let redactor = SensitiveValueRedactor::new(["secret-value".into()]);
    assert!(!format!("{redactor:?}").contains("secret-value"));
}
