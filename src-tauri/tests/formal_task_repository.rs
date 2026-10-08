use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::domain::common::task::{StepState, TargetState, TaskState};
use inxaiot_desk_buddy_lib::formal::config::AppPaths;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskRepository, TaskStepWrite,
};
use inxaiot_desk_buddy_lib::infrastructure::task_data_lifecycle::TaskDataLifecycle;

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

async fn succeeded_preflight(
    repository: &TaskRepository,
    task_id: &str,
    project: &str,
    targets: &[&str],
    snapshot_sha256: &str,
) {
    repository
        .create(CreateTask {
            id: task_id.into(),
            local_project_id: project.into(),
            remote_operation_record_id: None,
            domain_type: "aio".into(),
            operation_type: "deployment_preflight".into(),
            name: "部署检查".into(),
            priority: 0,
            batch_size: u32::try_from(targets.len()).expect("target count"),
            concurrency: 1,
            payload_ref: None,
            log_path: format!("D:/work/{task_id}/events.jsonl"),
            targets: std::iter::once(("preflight_internal".into(), "common".into()))
                .chain(
                    targets
                        .iter()
                        .map(|target| ("aio".into(), (*target).into())),
                )
                .collect(),
        })
        .await
        .expect("create preflight task");
    repository
        .transition(task_id, TaskState::Draft, TaskState::Checking, None, None)
        .await
        .expect("start preflight task");
    repository
        .update_target(
            task_id,
            TargetUpdate {
                resource_type: "preflight_internal".into(),
                resource_key: "common".into(),
                state: TargetState::Succeeded,
                stage: "检查完成".into(),
                progress_current: 3,
                progress_total: 3,
                fencing_token: None,
                message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                message_params_json: None,
            },
        )
        .await
        .expect("complete common preflight");
    for target in targets {
        repository
            .update_target(
                task_id,
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: (*target).into(),
                    state: TargetState::Succeeded,
                    stage: "检查完成".into(),
                    progress_current: 2,
                    progress_total: 2,
                    fencing_token: None,
                    message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                    message_params_json: None,
                },
            )
            .await
            .expect("complete target preflight");
    }
    repository
        .bind_preflight_snapshot(task_id, project, "deployment_preflight", snapshot_sha256)
        .await
        .expect("bind preflight snapshot");
    repository
        .transition(
            task_id,
            TaskState::Checking,
            TaskState::Succeeded,
            None,
            None,
        )
        .await
        .expect("finish preflight task");
}

#[tokio::test]
async fn succeeded_preflight_is_bound_and_consumed_once_when_queued_task_is_created() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    insert_project(&store, "project-b").await;
    let repository = TaskRepository::new(store.pool().clone());
    let snapshot_sha256 = "a".repeat(64);
    succeeded_preflight(
        &repository,
        "preflight-once",
        "project-a",
        &["A", "B"],
        &snapshot_sha256,
    )
    .await;

    let mut deployment = task("deployment-a", "project-a");
    deployment.remote_operation_record_id = None;
    let created = repository
        .create_queued_from_preflight(
            "preflight-once",
            "deployment_preflight",
            &snapshot_sha256,
            deployment.clone(),
        )
        .await
        .expect("create queued task from preflight");
    assert_eq!(created.state, TaskState::Queued);
    assert_eq!(
        repository
            .targets("deployment-a")
            .await
            .expect("deployment targets")
            .iter()
            .map(|target| target.resource_key.as_str())
            .collect::<Vec<_>>(),
        vec!["A", "B"]
    );

    deployment.id = "deployment-duplicate".into();
    assert!(matches!(
        repository
            .create_queued_from_preflight("preflight-once", "deployment_preflight", &snapshot_sha256, deployment)
            .await,
        Err(AppError::Conflict(message)) if message.contains("不能重复提交")
    ));
    assert!(repository.get("deployment-duplicate").await.is_err());
    store.close().await;
}

#[tokio::test]
async fn operation_link_rejects_a_task_that_started_cancelling() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    let repository = TaskRepository::new(store.pool().clone());
    let mut input = task("cancel-race", "project-a");
    input.remote_operation_record_id = None;
    repository.create(input).await.expect("create task");
    let mut current = TaskState::Draft;
    for next in [
        TaskState::Checking,
        TaskState::Ready,
        TaskState::Queued,
        TaskState::Running,
        TaskState::Cancelling,
    ] {
        repository
            .transition("cancel-race", current, next, None, None)
            .await
            .expect("advance task");
        current = next;
    }

    assert!(matches!(
        repository
            .link_operation_in_state("cancel-race", "operation-race", TaskState::Running)
            .await,
        Err(AppError::Conflict(message)) if message.contains("状态已变化")
    ));
    let task = repository.get("cancel-race").await.expect("task");
    assert_eq!(task.state, TaskState::Cancelling);
    assert!(task.remote_operation_record_id.is_none());
    store.close().await;
}

#[tokio::test]
async fn preflight_submission_rejects_wrong_project_snapshot_or_targets() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    insert_project(&store, "project-b").await;
    let repository = TaskRepository::new(store.pool().clone());
    let snapshot_sha256 = "b".repeat(64);
    succeeded_preflight(
        &repository,
        "preflight-guard",
        "project-a",
        &["A", "B"],
        &snapshot_sha256,
    )
    .await;

    let wrong_project = task("wrong-project", "project-b");
    assert!(matches!(
        repository
            .create_queued_from_preflight("preflight-guard", "deployment_preflight", &snapshot_sha256, wrong_project)
            .await,
        Err(AppError::Conflict(message)) if message.contains("不属于当前项目")
    ));
    let wrong_snapshot = task("wrong-snapshot", "project-a");
    assert!(matches!(
        repository
            .create_queued_from_preflight("preflight-guard", "deployment_preflight", &"c".repeat(64), wrong_snapshot)
            .await,
        Err(AppError::Conflict(message)) if message.contains("执行内容")
    ));
    let mut wrong_targets = task("wrong-targets", "project-a");
    wrong_targets.targets = vec![("aio".into(), "A".into())];
    wrong_targets.batch_size = 1;
    wrong_targets.concurrency = 1;
    assert!(matches!(
        repository
            .create_queued_from_preflight("preflight-guard", "deployment_preflight", &snapshot_sha256, wrong_targets)
            .await,
        Err(AppError::Conflict(message)) if message.contains("目标")
    ));
    store.close().await;
}

#[test]
fn task_state_machine_rejects_skips_and_terminal_reentry() {
    assert!(TaskState::Draft.can_transition_to(TaskState::Checking));
    assert!(!TaskState::Draft.can_transition_to(TaskState::Running));
    assert!(TaskState::Checking.can_transition_to(TaskState::Succeeded));
    assert!(TaskState::Checking.can_transition_to(TaskState::Failed));
    assert!(TaskState::Checking.can_transition_to(TaskState::Interrupted));
    assert!(TaskState::Running.can_transition_to(TaskState::Cancelling));
    assert!(TaskState::Running.can_transition_to(TaskState::FinalizingFailed));
    assert!(TaskState::FinalizingFailed.can_transition_to(TaskState::PartiallySucceeded));
    assert!(TaskState::Succeeded.is_terminal());
    assert!(!TaskState::Succeeded.can_transition_to(TaskState::Running));
}

#[tokio::test]
async fn checking_task_is_active_and_target_progress_is_initialized_atomically() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-preflight").await;
    let repository = TaskRepository::new(store.pool().clone());
    repository
        .create(task("preflight-task", "project-preflight"))
        .await
        .expect("create preflight task");
    repository
        .initialize_target_progress("preflight-task", "等待检查", 1)
        .await
        .expect("initialize all targets");
    repository
        .transition(
            "preflight-task",
            TaskState::Draft,
            TaskState::Checking,
            None,
            None,
        )
        .await
        .expect("start checking");

    let counts = repository.active_counts().await.expect("active counts");
    assert_eq!(counts.total, 1);
    assert_eq!(counts.running, 1);
    assert_eq!(
        repository.list_active().await.expect("active tasks").len(),
        1
    );
    assert!(
        repository
            .targets("preflight-task")
            .await
            .expect("targets")
            .iter()
            .all(|target| {
                target.stage == "等待检查"
                    && target.progress_current == 0
                    && target.progress_total == 1
            })
    );

    repository
        .transition(
            "preflight-task",
            TaskState::Checking,
            TaskState::Succeeded,
            None,
            None,
        )
        .await
        .expect("finish checking");
    assert_eq!(repository.active_counts().await.expect("counts").total, 0);
    store.close().await;
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
async fn clearing_terminal_tasks_keeps_retryable_active_and_other_project_records() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    insert_project(&store, "project-b").await;
    let repository = TaskRepository::new(store.pool().clone());
    for task_id in ["succeeded", "failed", "check-failed", "draft"] {
        repository
            .create(task(task_id, "project-a"))
            .await
            .expect("create project-a task");
    }
    repository
        .create(task("other-project", "project-b"))
        .await
        .expect("create project-b task");
    for (task_id, final_state) in [
        ("succeeded", TaskState::Succeeded),
        ("failed", TaskState::Failed),
        ("other-project", TaskState::Interrupted),
    ] {
        for (expected, next) in [
            (TaskState::Draft, TaskState::Checking),
            (TaskState::Checking, TaskState::Ready),
            (TaskState::Ready, TaskState::Queued),
            (TaskState::Queued, TaskState::Running),
        ] {
            repository
                .transition(task_id, expected, next, None, None)
                .await
                .expect("advance terminal task");
        }
        repository
            .transition(task_id, TaskState::Running, final_state, None, None)
            .await
            .expect("finish terminal task");
    }
    repository
        .transition(
            "check-failed",
            TaskState::Draft,
            TaskState::Checking,
            None,
            None,
        )
        .await
        .expect("start retryable task");
    repository
        .transition(
            "check-failed",
            TaskState::Checking,
            TaskState::CheckFailed,
            None,
            None,
        )
        .await
        .expect("mark retryable task");

    assert_eq!(
        repository
            .clear_terminal_for_project("project-a")
            .await
            .expect("clear terminal tasks"),
        2
    );
    assert!(repository.get("succeeded").await.is_err());
    assert!(repository.get("failed").await.is_err());
    assert_eq!(
        repository
            .get("check-failed")
            .await
            .expect("retryable task")
            .state,
        TaskState::CheckFailed
    );
    assert_eq!(
        repository.get("draft").await.expect("draft task").state,
        TaskState::Draft
    );
    assert_eq!(
        repository
            .get("other-project")
            .await
            .expect("other project task")
            .state,
        TaskState::Interrupted
    );
    store.close().await;
}

#[tokio::test]
async fn clearing_screen_task_records_keeps_logs_and_protected_project_records() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("ensure app paths");
    let store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    insert_project(&store, "project-b").await;
    let repository = TaskRepository::new(store.pool().clone());
    let fixtures = [
        ("screen-completed", "project-a", TaskState::Succeeded),
        ("screen-running", "project-a", TaskState::Running),
        ("screen-guarded", "project-a", TaskState::Failed),
        (
            "screen-finalizing",
            "project-a",
            TaskState::FinalizingFailed,
        ),
        ("screen-other-project", "project-b", TaskState::Succeeded),
    ];
    for (id, project, final_state) in fixtures {
        let log = paths.project_task_log_path(project, id).expect("task log");
        let mut input = task(id, project);
        input.domain_type = "screen".into();
        input.operation_type = "inspect".into();
        input.payload_ref = None;
        input.remote_operation_record_id = None;
        input.targets = vec![("screen".into(), "screen-a".into())];
        input.log_path = log.to_string_lossy().into_owned();
        repository.create(input).await.expect("create screen task");
        for (expected, next) in [
            (TaskState::Draft, TaskState::Checking),
            (TaskState::Checking, TaskState::Ready),
            (TaskState::Ready, TaskState::Queued),
            (TaskState::Queued, TaskState::Running),
        ] {
            repository
                .transition(id, expected, next, None, None)
                .await
                .expect("advance screen task");
        }
        if final_state != TaskState::Running {
            repository
                .transition(id, TaskState::Running, final_state, None, None)
                .await
                .expect("finish screen task");
        }
        // 使用真实迁移后的屏任务表，验证公共清理会触发其关联清理。
        sqlx::query("INSERT INTO local_screen_task_data(local_task_id,local_project_id,plan_json,plan_sha256,result_json,updated_at) VALUES(?,?,?,?,'{\"targets\":{},\"finished\":true}','1')")
            .bind(id).bind(project).bind(format!("{{\"projectId\":\"{project}\"}}"))
            .bind("a".repeat(64)).execute(store.pool()).await.expect("screen task data");
        std::fs::create_dir_all(log.parent().expect("log parent")).expect("log directory");
        std::fs::write(&log, format!("log for {id}")).expect("task log bytes");
    }
    repository
        .protect_results("screen-guarded", "结果尚待核实")
        .await
        .expect("protect screen results");
    succeeded_preflight(
        &repository,
        "unsubmitted-preflight",
        "project-a",
        &["A"],
        &"b".repeat(64),
    )
    .await;
    let preflight_log = paths
        .project_task_log_path("project-a", "unsubmitted-preflight")
        .expect("preflight log");
    std::fs::write(&preflight_log, b"unsubmitted preflight log").expect("preflight log bytes");
    sqlx::query("UPDATE local_task SET log_path=? WHERE id='unsubmitted-preflight'")
        .bind(preflight_log.to_string_lossy().as_ref())
        .execute(store.pool())
        .await
        .expect("preflight log path");

    assert_eq!(
        repository
            .clear_terminal_for_project("project-a")
            .await
            .expect("clear finished task records"),
        2
    );
    assert!(repository.get("screen-completed").await.is_err());
    assert!(
        repository
            .targets("screen-completed")
            .await
            .expect("deleted screen targets")
            .is_empty()
    );
    let removed_screen_data: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_screen_task_data WHERE local_task_id='screen-completed'",
    )
    .fetch_one(store.pool())
    .await
    .expect("deleted screen task data");
    assert_eq!(removed_screen_data, 0);
    for (id, project, expected_state) in fixtures {
        assert_eq!(
            std::fs::read(
                paths
                    .project_task_log_path(project, id)
                    .expect("task log path")
            )
            .expect("preserved log file"),
            format!("log for {id}").into_bytes()
        );
        if id != "screen-completed" {
            assert_eq!(
                repository
                    .get(id)
                    .await
                    .expect("protected task remains")
                    .state,
                expected_state
            );
            let data_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM local_screen_task_data WHERE local_task_id=?",
            )
            .bind(id)
            .fetch_one(store.pool())
            .await
            .expect("preserved screen task data");
            assert_eq!(data_count, 1);
        }
    }
    assert!(
        repository
            .results_protected("screen-guarded")
            .await
            .expect("result guard remains")
    );
    assert!(repository.get("unsubmitted-preflight").await.is_err());
    assert_eq!(
        std::fs::read(preflight_log).expect("preflight log remains"),
        b"unsubmitted preflight log"
    );
    assert_eq!(
        repository
            .clear_terminal_for_project("project-a")
            .await
            .expect("repeat clearing"),
        0
    );
    store.close().await;
}

#[tokio::test]
async fn clearing_screen_task_log_preserves_task_snapshot_and_other_logs() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
    paths.ensure().expect("ensure app paths");
    let store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    let repository = TaskRepository::new(store.pool().clone());
    let task_id = "screen-log-only";
    let log = paths
        .project_task_log_path("project-a", task_id)
        .expect("task log path");
    let mut input = task(task_id, "project-a");
    input.domain_type = "screen".into();
    input.operation_type = "inspect".into();
    input.targets = vec![("screen".into(), "screen-a".into())];
    input.log_path = log.to_string_lossy().into_owned();
    repository.create(input).await.expect("create screen task");
    for (expected, next) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
        (TaskState::Running, TaskState::Succeeded),
    ] {
        repository
            .transition(task_id, expected, next, None, None)
            .await
            .expect("finish screen task");
    }
    let plan = r#"{"projectId":"project-a","screenIds":["screen-a"]}"#;
    let results = r#"{"targets":{"screen-a":{"message":"检查完成"}},"finished":true}"#;
    sqlx::query("INSERT INTO local_screen_task_data(local_task_id,local_project_id,plan_json,plan_sha256,result_json,updated_at) VALUES(?,'project-a',?,?,?,'1')")
        .bind(task_id).bind(plan).bind("c".repeat(64)).bind(results).execute(store.pool()).await.expect("screen task snapshot");
    std::fs::create_dir_all(log.parent().expect("log parent")).expect("log directory");
    std::fs::write(&log, b"selected screen task log").expect("selected log");
    let other_log = paths
        .project_task_log_path("project-a", "other-task")
        .expect("other log path");
    std::fs::write(&other_log, b"other task log").expect("other log");
    let lifecycle = TaskDataLifecycle::new(&paths);
    lifecycle
        .finalize_task("project-a", task_id, TaskState::Succeeded)
        .expect("record log retention");
    let retention = std::path::PathBuf::from(format!("{}.retention.json", log.to_string_lossy()));
    assert!(retention.is_file());
    let task_before = repository
        .get(task_id)
        .await
        .expect("task before clear log");

    lifecycle
        .clear_task_log("project-a", task_id)
        .expect("clear selected task log");
    assert!(!log.exists());
    assert!(!retention.exists());
    let task_after = repository
        .get(task_id)
        .await
        .expect("task remains after log clearing");
    assert_eq!(task_after.state, task_before.state);
    assert_eq!(task_after.updated_at, task_before.updated_at);
    assert_eq!(task_after.sequence, task_before.sequence);
    assert_eq!(
        repository
            .targets(task_id)
            .await
            .expect("task targets remain")
            .len(),
        1
    );
    let saved_data: (String, String, String) = sqlx::query_as("SELECT plan_json,plan_sha256,result_json FROM local_screen_task_data WHERE local_task_id=?")
        .bind(task_id).fetch_one(store.pool()).await.expect("screen snapshot remains");
    assert_eq!(saved_data, (plan.into(), "c".repeat(64), results.into()));
    assert_eq!(
        std::fs::read(other_log).expect("other log remains"),
        b"other task log"
    );
    lifecycle
        .clear_task_log("project-a", task_id)
        .expect("repeat log clear");
    assert_eq!(
        repository
            .list_recent("project-a", 20)
            .await
            .expect("tasks remain listed")
            .len(),
        1
    );
    store.close().await;
}

#[tokio::test]
async fn clearing_preflight_requires_recheck_but_preserves_already_queued_execution() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    insert_project(&store, "project-a").await;
    let repository = TaskRepository::new(store.pool().clone());
    let snapshot_sha256 = "d".repeat(64);
    succeeded_preflight(
        &repository,
        "preflight-pending-submission",
        "project-a",
        &["A", "B"],
        &snapshot_sha256,
    )
    .await;

    assert_eq!(
        repository
            .clear_terminal_for_project("project-a")
            .await
            .expect("clear completed preflight"),
        1
    );
    let mut deployment = task("deployment-after-recheck", "project-a");
    deployment.remote_operation_record_id = None;
    let error = repository
        .create_queued_from_preflight(
            "preflight-pending-submission",
            "deployment_preflight",
            &snapshot_sha256,
            deployment.clone(),
        )
        .await
        .expect_err("cleared preflight cannot authorize execution");
    assert!(
        matches!(error, AppError::NotFound(message) if message == "检查记录已清空，请重新检查后再执行")
    );
    assert!(repository.get(&deployment.id).await.is_err());
    assert!(
        repository
            .targets(&deployment.id)
            .await
            .expect("no execution targets")
            .is_empty()
    );

    succeeded_preflight(
        &repository,
        "preflight-rechecked",
        "project-a",
        &["A", "B"],
        &snapshot_sha256,
    )
    .await;
    repository
        .create_queued_from_preflight(
            "preflight-rechecked",
            "deployment_preflight",
            &snapshot_sha256,
            deployment,
        )
        .await
        .expect("consume successful preflight");
    assert_eq!(
        repository
            .clear_terminal_for_project("project-a")
            .await
            .expect("clear consumed preflight"),
        1
    );
    assert!(repository.get("preflight-rechecked").await.is_err());
    assert_eq!(
        repository
            .get("deployment-after-recheck")
            .await
            .expect("queued deployment remains")
            .state,
        TaskState::Queued
    );
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
    repository
        .create(task("checking-task", "project-a"))
        .await
        .expect("create checking task");
    repository
        .initialize_target_progress("checking-task", "等待检查", 1)
        .await
        .expect("initialize checking targets");
    repository
        .transition(
            "checking-task",
            TaskState::Draft,
            TaskState::Checking,
            None,
            None,
        )
        .await
        .expect("start checking task");
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
        vec!["checking-task", "running-task"]
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
    assert_eq!(
        recovered_repository
            .get("checking-task")
            .await
            .expect("checking task")
            .state,
        TaskState::Interrupted
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
