use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::domain::common::task::{StepState, TargetState, TaskState};
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskRepository, TaskStepWrite,
};

async fn insert_project(store: &LocalStore, id: &str) {
    sqlx::query(
        "INSERT INTO local_project \
         (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
          db_password_secret_ref, created_at, updated_at) VALUES (?, ?, 'http://platform.test', \
          'db.test', 3306, 'user', 'business', 'workbench', 'secret-ref', '1', '1')",
    )
    .bind(id)
    .bind(id)
    .execute(store.pool())
    .await
    .expect("project fixture");
}

fn task(id: &str, project: &str) -> CreateTask {
    CreateTask {
        id: id.into(),
        local_project_id: project.into(),
        remote_operation_record_id: Some(format!("operation-{id}")),
        domain_type: "aio".into(),
        operation_type: "full_upgrade".into(),
        name: format!("Task {id}"),
        priority: 5,
        batch_size: 2,
        concurrency: 2,
        payload_ref: Some(format!("payload-{id}")),
        log_path: format!("D:/work/{id}/events.jsonl"),
        targets: vec![
            ("aio".into(), "A".into()),
            ("aio".into(), "B".into()),
            ("aio".into(), "A".into()),
        ],
    }
}

#[test]
fn task_state_machine_rejects_skips_and_terminal_reentry() {
    assert!(TaskState::Draft.can_transition_to(TaskState::Checking));
    assert!(!TaskState::Draft.can_transition_to(TaskState::Running));
    assert!(TaskState::Running.can_transition_to(TaskState::Cancelling));
    assert!(TaskState::Running.can_transition_to(TaskState::FinalizingFailed));
    assert!(TaskState::FinalizingFailed.can_transition_to(TaskState::PartiallySucceeded));
    assert!(TaskState::Succeeded.is_terminal());
    assert!(!TaskState::Succeeded.can_transition_to(TaskState::Running));
}

#[tokio::test]
async fn local_finalization_projection_is_atomic_and_retryable() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-finalize").await;
    let repository = TaskRepository::new(store.pool().clone());
    repository
        .create(task("projection-task", "project-finalize"))
        .await
        .expect("create task");
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
    ] {
        repository
            .transition("projection-task", expected, next, None, None)
            .await
            .expect("advance task");
    }
    let target = |key: &str| TargetUpdate {
        resource_type: "aio".into(),
        resource_key: key.into(),
        state: TargetState::Succeeded,
        stage: "completed".into(),
        progress_current: 100,
        progress_total: 100,
        fencing_token: Some(9),
        message_code: None,
        message_params_json: None,
    };
    let step = |key: &str| TaskStepWrite {
        id: format!("projection-task:{key}:health"),
        resource_type: Some("aio".into()),
        resource_key: Some(key.into()),
        step_code: "health".into(),
        state: StepState::Succeeded,
        error_code: None,
        message: Some("ok".into()),
    };

    assert!(
        repository
            .finalize_projection(
                "projection-task",
                TaskState::Running,
                TaskState::Succeeded,
                &[target("A"), target("MISSING")],
                &[step("A")],
            )
            .await
            .is_err()
    );
    assert_eq!(
        repository.get("projection-task").await.expect("task").state,
        TaskState::Running
    );
    assert!(
        repository
            .targets("projection-task")
            .await
            .expect("targets")
            .iter()
            .all(|target| target.state == TargetState::Pending)
    );

    repository
        .finalize_projection(
            "projection-task",
            TaskState::Running,
            TaskState::Succeeded,
            &[target("A"), target("B")],
            &[step("A"), step("B")],
        )
        .await
        .expect("retry finalization projection");
    assert_eq!(
        repository.get("projection-task").await.expect("task").state,
        TaskState::Succeeded
    );
    assert!(
        repository
            .targets("projection-task")
            .await
            .expect("targets")
            .iter()
            .all(|target| target.state == TargetState::Succeeded)
    );
    assert_eq!(
        repository
            .steps("projection-task")
            .await
            .expect("steps")
            .len(),
        2
    );
    store.close().await;
}

#[tokio::test]
async fn task_target_step_sequence_and_project_isolation_are_atomic() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    insert_project(&store, "project-b").await;
    let repository = TaskRepository::new(store.pool().clone());
    let created = repository
        .create(task("task-a", "project-a"))
        .await
        .expect("create task");
    repository
        .create(task("task-b", "project-b"))
        .await
        .expect("create other project task");
    assert_eq!(created.state, TaskState::Draft);
    assert_eq!(created.sequence, 0);
    assert_eq!(
        repository.targets("task-a").await.expect("targets").len(),
        2
    );
    assert!(matches!(
        repository
            .transition("task-a", TaskState::Draft, TaskState::Running, None, None,)
            .await,
        Err(AppError::Conflict(_))
    ));
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
    ] {
        repository
            .transition("task-a", expected, next, None, None)
            .await
            .expect("valid task transition");
    }
    let target_sequence = repository
        .update_target(
            "task-a",
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: "A".into(),
                state: TargetState::Running,
                stage: "upload".into(),
                progress_current: 5,
                progress_total: 10,
                fencing_token: Some(7),
                message_code: Some("UPLOAD_PROGRESS".into()),
                message_params_json: Some("{\"file\":\"release\"}".into()),
            },
        )
        .await
        .expect("update target");
    assert_eq!(target_sequence, 5);
    let step_sequence = repository
        .save_step(
            "task-a",
            TaskStepWrite {
                id: "step-a".into(),
                resource_type: Some("aio".into()),
                resource_key: Some("A".into()),
                step_code: "upload".into(),
                state: StepState::Running,
                error_code: None,
                message: None,
            },
        )
        .await
        .expect("start step");
    assert_eq!(step_sequence, 6);
    repository
        .save_step(
            "task-a",
            TaskStepWrite {
                id: "step-a".into(),
                resource_type: Some("aio".into()),
                resource_key: Some("A".into()),
                step_code: "upload".into(),
                state: StepState::Succeeded,
                error_code: None,
                message: Some("uploaded".into()),
            },
        )
        .await
        .expect("finish step");
    let finished = repository
        .transition(
            "task-a",
            TaskState::Running,
            TaskState::PartiallySucceeded,
            None,
            Some("one target needs attention"),
        )
        .await
        .expect("finalize task");
    assert_eq!(finished.sequence, 8);
    assert!(finished.ended_at.is_some());
    let target = repository.targets("task-a").await.expect("targets")[0].clone();
    assert_eq!(target.progress_current, 5);
    assert_eq!(target.progress_total, 10);
    let step = repository.steps("task-a").await.expect("steps")[0].clone();
    assert_eq!(step.state, StepState::Succeeded);
    assert!(step.started_at.is_some());
    assert!(step.ended_at.is_some());
    assert_eq!(
        repository
            .list_recent("project-a", 20)
            .await
            .expect("project a tasks")
            .len(),
        1
    );
    assert_eq!(
        repository
            .list_recent("project-b", 20)
            .await
            .expect("project b tasks")
            .len(),
        1
    );
    repository
        .delete_test_task("task-a")
        .await
        .expect("delete exact task");
    assert!(repository.get("task-a").await.is_err());
    assert!(repository.get("task-b").await.is_ok());
    store.close().await;
}

#[tokio::test]
async fn startup_recovery_marks_only_executing_tasks_interrupted_without_replay() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let database = temp.path().join("local.db");
    let store = LocalStore::open(&database).await.expect("local store");
    insert_project(&store, "project-a").await;
    let repository = TaskRepository::new(store.pool().clone());
    repository
        .create(task("running-task", "project-a"))
        .await
        .expect("create running task");
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
    ] {
        repository
            .transition("running-task", expected, next, None, None)
            .await
            .expect("transition running task");
    }
    repository
        .update_target(
            "running-task",
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: "A".into(),
                state: TargetState::Running,
                stage: "remote_command".into(),
                progress_current: 1,
                progress_total: 2,
                fencing_token: Some(9),
                message_code: None,
                message_params_json: None,
            },
        )
        .await
        .expect("running target");
    repository
        .save_step(
            "running-task",
            TaskStepWrite {
                id: "running-step".into(),
                resource_type: Some("aio".into()),
                resource_key: Some("A".into()),
                step_code: "remote_command".into(),
                state: StepState::Running,
                error_code: None,
                message: None,
            },
        )
        .await
        .expect("running step");
    repository
        .create(task("draft-task", "project-a"))
        .await
        .expect("create draft task");
    store.close().await;

    let reopened = LocalStore::open(&database)
        .await
        .expect("reopen local store");
    let recovered_repository = TaskRepository::new(reopened.pool().clone());
    assert_eq!(
        recovered_repository
            .recover_interrupted()
            .await
            .expect("recover interrupted tasks"),
        vec!["running-task"]
    );
    let recovered = recovered_repository
        .get("running-task")
        .await
        .expect("recovered task");
    assert_eq!(recovered.state, TaskState::Interrupted);
    assert_eq!(recovered.error_code.as_deref(), Some("APP_INTERRUPTED"));
    let targets = recovered_repository
        .targets("running-task")
        .await
        .expect("recovered targets");
    assert_eq!(targets[0].state, TargetState::Interrupted);
    assert_eq!(targets[1].state, TargetState::Unknown);
    assert_eq!(
        recovered_repository
            .steps("running-task")
            .await
            .expect("recovered steps")[0]
            .state,
        StepState::Interrupted
    );
    assert_eq!(
        recovered_repository
            .get("draft-task")
            .await
            .expect("draft task")
            .state,
        TaskState::Draft
    );
    assert!(
        recovered_repository
            .recover_interrupted()
            .await
            .expect("idempotent recovery")
            .is_empty()
    );
    reopened.close().await;
}
