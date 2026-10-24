use std::{path::PathBuf, sync::Arc};

use inxaiot_desk_buddy_lib::{
    domain::common::task::TaskState,
    formal::{
        app_state::FormalAppState, config::AppPaths, local_store::LocalStore,
        runtime_registry::ProjectRuntimeRegistry, secret_store::MemorySecretStore,
    },
    infrastructure::{
        local_sqlite::task_repository::{CreateTask, TaskRepository},
        logging::{redactor::SensitiveValueRedactor, task_event_pipeline::TaskEventPipeline},
        task_data_lifecycle::TaskDataLifecycle,
        task_runtime::{cleanup_finished_task_artifacts, cleanup_orphan_task_artifacts},
    },
    interface::commands::task_activity::{clear_finished_task_records, query_activity_tasks},
    runtime::{
        event_bus::TaskEventBus,
        job_supervisor::JobSupervisor,
        task_queue::{TaskHandlerRegistry, TaskQueue},
    },
};

async fn fixture() -> (tempfile::TempDir, FormalAppState) {
    let temp = tempfile::tempdir().unwrap();
    let paths = AppPaths::from_data_dir(temp.path()).unwrap();
    paths.ensure().unwrap();
    let local_store = LocalStore::open(&paths.local_db).await.unwrap();
    sqlx::query("INSERT INTO local_project (id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES ('project','cleanup fixture','http://test.invalid','test.invalid',3306,'test','business','workbench','test-secret','1','1')")
        .execute(local_store.pool()).await.unwrap();
    let task_repository = TaskRepository::new(local_store.pool().clone());
    let task_event_bus = TaskEventBus::new(16).unwrap();
    let task_event_pipeline = TaskEventPipeline::new(
        task_repository.clone(),
        task_event_bus.clone(),
        SensitiveValueRedactor::default(),
    );
    let job_supervisor = JobSupervisor::default();
    let task_handler_registry = TaskHandlerRegistry::default();
    let task_queue = TaskQueue::start(4, 1, task_handler_registry.clone(), job_supervisor.clone())
        .await
        .unwrap();
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
    (temp, state)
}

async fn task(
    state: &FormalAppState,
    id: &str,
    domain: &str,
    operation: &str,
    final_state: TaskState,
) {
    state
        .task_repository
        .create(CreateTask {
            id: id.into(),
            local_project_id: "project".into(),
            remote_operation_record_id: None,
            domain_type: domain.into(),
            operation_type: operation.into(),
            name: id.into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: state
                .paths
                .project_task_log_path("project", id)
                .unwrap()
                .to_string_lossy()
                .into(),
            targets: vec![(domain.into(), "device".into())],
        })
        .await
        .unwrap();
    let mut current = TaskState::Draft;
    for next in [
        TaskState::Checking,
        TaskState::Ready,
        TaskState::Queued,
        TaskState::Running,
    ] {
        state
            .task_repository
            .transition(id, current, next, None, None)
            .await
            .unwrap();
        current = next;
        if next == final_state {
            return;
        }
    }
    if final_state != current {
        state
            .task_repository
            .transition(id, current, final_state, None, None)
            .await
            .unwrap();
    }
}

fn file(state: &FormalAppState, id: &str, name: &str) -> PathBuf {
    let path = state
        .paths
        .project_task_dir("project", id)
        .unwrap()
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, format!("retained fixture: {id}/{name}")).unwrap();
    path
}

fn log(state: &FormalAppState, id: &str) -> PathBuf {
    let path = state.paths.project_task_log_path("project", id).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{\"message\":\"fixture evidence\"}\n").unwrap();
    path
}

#[tokio::test]
async fn completed_cleanup_removes_screen_apk_and_aio_images_but_preserves_evidence() {
    let (_temp, state) = fixture().await;
    task(
        &state,
        "screen",
        "smart_screen",
        "install",
        TaskState::Succeeded,
    )
    .await;
    let apk = file(&state, "screen", "xiaoxin.apk");
    let diagnostic = file(&state, "screen", "diagnostic-preserved.json");
    let screen_log = log(&state, "screen");
    sqlx::query("INSERT INTO local_screen_task_data VALUES ('screen','project',?,?,'{}','1')")
        .bind(r#"{"detail":{"apk":{"appVersion":"2.1.3","sha256":"original-package-hash"}}}"#)
        .bind("plan-hash")
        .execute(state.local_store.pool())
        .await
        .unwrap();
    cleanup_finished_task_artifacts(&state.task_repository, &state.paths, "screen")
        .await
        .unwrap();
    assert!(!apk.exists());
    assert!(diagnostic.is_file());
    assert!(screen_log.is_file());
    let saved: String = sqlx::query_scalar(
        "SELECT plan_json FROM local_screen_task_data WHERE local_task_id='screen'",
    )
    .fetch_one(state.local_store.pool())
    .await
    .unwrap();
    assert!(saved.contains("original-package-hash"));
    assert_eq!(
        state.task_repository.get("screen").await.unwrap().state,
        TaskState::Succeeded
    );

    task(&state, "aio", "aio", "full_upgrade", TaskState::Succeeded).await;
    let tar = file(&state, "aio", "release.tar");
    let image = file(&state, "aio", "generated-release/images/app.tar");
    let aio_log = log(&state, "aio");
    cleanup_finished_task_artifacts(&state.task_repository, &state.paths, "aio")
        .await
        .unwrap();
    assert!(!tar.exists() && !image.exists());
    assert!(aio_log.is_file());
    cleanup_finished_task_artifacts(&state.task_repository, &state.paths, "screen")
        .await
        .unwrap();
}

#[tokio::test]
async fn active_guarded_and_pending_finalization_tasks_keep_their_packages() {
    let (_temp, state) = fixture().await;
    for (id, status) in [
        ("queued", TaskState::Queued),
        ("running", TaskState::Running),
        ("finalizing", TaskState::FinalizingFailed),
        ("guarded", TaskState::Succeeded),
    ] {
        task(&state, id, "smart_screen", "install", status).await;
        let apk = file(&state, id, "xiaoxin.apk");
        if id == "guarded" {
            state
                .task_repository
                .protect_results(id, "待核实")
                .await
                .unwrap();
        }
        cleanup_finished_task_artifacts(&state.task_repository, &state.paths, id)
            .await
            .unwrap();
        assert!(apk.is_file(), "{id} must keep its package");
    }
    state
        .task_repository
        .resolve_results("guarded")
        .await
        .unwrap();
    cleanup_finished_task_artifacts(&state.task_repository, &state.paths, "guarded")
        .await
        .unwrap();
    assert!(
        !state
            .paths
            .project_task_dir("project", "guarded")
            .unwrap()
            .join("xiaoxin.apk")
            .exists()
    );
}

#[tokio::test]
async fn history_clear_keeps_diagnostics_logs_and_interrupted_recovery_records() {
    let (_temp, state) = fixture().await;
    task(
        &state,
        "install",
        "smart_screen",
        "install",
        TaskState::Succeeded,
    )
    .await;
    task(
        &state,
        "diagnostics",
        "smart_screen",
        "diagnostics",
        TaskState::Succeeded,
    )
    .await;
    task(
        &state,
        "pending",
        "aio",
        "full_upgrade",
        TaskState::Interrupted,
    )
    .await;
    task(
        &state,
        "guarded",
        "smart_screen",
        "install",
        TaskState::Succeeded,
    )
    .await;
    state
        .task_repository
        .protect_results("guarded", "待核实")
        .await
        .unwrap();
    let apk = file(&state, "install", "xiaoxin.apk");
    let report = file(&state, "diagnostics", "diagnostic-device.json");
    let evidence = file(&state, "pending", "pending-local-finalization.json");
    let pending_tar = file(&state, "pending", "release.tar");
    let guarded = file(&state, "guarded", "xiaoxin.apk");
    let install_log = log(&state, "install");
    let diagnostic_log = log(&state, "diagnostics");
    assert_eq!(
        clear_finished_task_records(&state, "project")
            .await
            .unwrap(),
        2
    );
    assert!(!apk.exists());
    assert!(report.is_file() && install_log.is_file() && diagnostic_log.is_file());
    assert!(evidence.is_file() && pending_tar.is_file() && guarded.is_file());
    assert!(state.task_repository.get("pending").await.is_ok());
    assert!(state.task_repository.get("guarded").await.is_ok());
    assert!(state.task_repository.get("install").await.is_err());
}

#[tokio::test]
async fn interrupted_aio_without_finalization_marker_is_not_treated_as_completed() {
    let (_temp, state) = fixture().await;
    task(
        &state,
        "interrupted-no-marker",
        "aio",
        "full_upgrade",
        TaskState::Interrupted,
    )
    .await;
    let archive = file(&state, "interrupted-no-marker", "release.tar");
    let payload = file(&state, "interrupted-no-marker", "deployment-input.json");
    let task_log = log(&state, "interrupted-no-marker");
    assert!(
        !archive
            .parent()
            .unwrap()
            .join("pending-local-finalization.json")
            .exists()
    );
    cleanup_finished_task_artifacts(
        &state.task_repository,
        &state.paths,
        "interrupted-no-marker",
    )
    .await
    .unwrap();
    assert_eq!(
        clear_finished_task_records(&state, "project")
            .await
            .unwrap(),
        0
    );
    assert!(archive.is_file() && payload.is_file() && task_log.is_file());
    assert_eq!(
        state
            .task_repository
            .get("interrupted-no-marker")
            .await
            .unwrap()
            .state,
        TaskState::Interrupted
    );
    let activity = query_activity_tasks(&state, "project", 10).await.unwrap();
    assert!(
        !activity
            .iter()
            .find(|item| item.id == "interrupted-no-marker")
            .unwrap()
            .clearable
    );
}

#[tokio::test]
async fn startup_removes_old_expiry_marker_even_for_guarded_interrupted_aio() {
    let (_temp, state) = fixture().await;
    task(
        &state,
        "protected-interrupted",
        "aio",
        "full_upgrade",
        TaskState::Interrupted,
    )
    .await;
    state
        .task_repository
        .protect_results("protected-interrupted", "远端结果待核实")
        .await
        .unwrap();
    task(
        &state,
        "expired-completed",
        "smart_screen",
        "install",
        TaskState::Succeeded,
    )
    .await;
    let protected_log = log(&state, "protected-interrupted");
    let completed_log = log(&state, "expired-completed");
    for (id, path) in [
        ("protected-interrupted", &protected_log),
        ("expired-completed", &completed_log),
    ] {
        let sidecar = format!("{}.retention.json", path.to_string_lossy());
        std::fs::write(
            sidecar,
            serde_json::json!({"taskId":id,"taskState":"interrupted","deleteAfterEpochSeconds":1})
                .to_string(),
        )
        .unwrap();
    }
    let candidates = state
        .task_repository
        .list_artifact_cleanup_candidates()
        .await
        .unwrap();
    assert!(
        candidates
            .iter()
            .any(|task| task.id == "protected-interrupted")
    );
    for task in candidates {
        cleanup_finished_task_artifacts(&state.task_repository, &state.paths, &task.id)
            .await
            .unwrap();
    }
    assert_eq!(
        TaskDataLifecycle::new(&state.paths)
            .sweep_expired_logs()
            .unwrap(),
        1
    );
    assert!(protected_log.is_file());
    assert!(
        !PathBuf::from(format!(
            "{}.retention.json",
            protected_log.to_string_lossy()
        ))
        .exists()
    );
    assert!(!completed_log.exists());
    assert!(
        state
            .task_repository
            .results_protected("protected-interrupted")
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn startup_orphan_cleanup_respects_other_task_plans_and_recent_package_references() {
    let (_temp, state) = fixture().await;
    let free_apk = file(&state, "orphan", "xiaoxin.apk");
    let report = file(&state, "orphan", "diagnostic-device.json");
    let referenced = file(&state, "referenced", "service-image.tar");
    let recent = file(&state, "recent", "xiaoxin.apk");
    let pending = file(&state, "no-record-pending", "release.tar");
    file(
        &state,
        "no-record-pending",
        "pending-local-finalization.json",
    );
    task(
        &state,
        "consumer",
        "aio",
        "service_upgrade",
        TaskState::Succeeded,
    )
    .await;
    sqlx::query("UPDATE local_task SET payload_ref=? WHERE id='consumer'")
        .bind(referenced.to_string_lossy().as_ref())
        .execute(state.local_store.pool())
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO local_recent_artifact VALUES ('project','smart_screen','apk','',?,'1')",
    )
    .bind(recent.to_string_lossy().as_ref())
    .execute(state.local_store.pool())
    .await
    .unwrap();
    assert_eq!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .unwrap(),
        1
    );
    assert!(!free_apk.exists());
    assert!(report.is_file() && referenced.is_file() && recent.is_file() && pending.is_file());

    sqlx::query("UPDATE local_task SET payload_ref=NULL WHERE id='consumer'")
        .execute(state.local_store.pool())
        .await
        .unwrap();
    sqlx::query("DELETE FROM local_recent_artifact")
        .execute(state.local_store.pool())
        .await
        .unwrap();
    assert_eq!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .unwrap(),
        2
    );
    assert!(pending.is_file() && report.is_file());
}

#[tokio::test]
async fn pending_aio_payload_file_defers_orphan_cleanup_until_the_task_is_confirmed() {
    let (_temp, state) = fixture().await;
    let old_image = file(&state, "previous-task", "service-image.tar");
    let other_apk = file(&state, "unreferenced-screen", "xiaoxin.apk");
    task(
        &state,
        "aio-consumer",
        "aio",
        "service_upgrade",
        TaskState::Running,
    )
    .await;
    let payload = file(&state, "aio-consumer", "deployment-input.json");
    std::fs::write(
        &payload,
        serde_json::json!({"snapshot":{"plan":{"image_files":[{"file_path":old_image}]}}})
            .to_string(),
    )
    .unwrap();
    sqlx::query("UPDATE local_task SET payload_ref=? WHERE id='aio-consumer'")
        .bind(payload.to_string_lossy().as_ref())
        .execute(state.local_store.pool())
        .await
        .unwrap();
    // 数据库只有 input.json 路径，旧镜像路径只在其文件正文内。
    assert!(
        !state
            .task_repository
            .artifact_directory_referenced("previous-task")
            .await
            .unwrap()
    );
    assert_eq!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .unwrap(),
        0
    );
    assert!(old_image.is_file() && other_apk.is_file());
    state
        .task_repository
        .transition(
            "aio-consumer",
            TaskState::Running,
            TaskState::Succeeded,
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .unwrap(),
        2
    );
    assert!(payload.is_file());
}

#[tokio::test]
async fn unqueued_plan_and_database_read_errors_prevent_orphan_deletion() {
    let (_temp, state) = fixture().await;
    let apk = file(&state, "staged-plan", "xiaoxin.apk");
    sqlx::query(
        "INSERT INTO local_screen_task_data VALUES ('staged-plan','project','{}','hash','{}','1')",
    )
    .execute(state.local_store.pool())
    .await
    .unwrap();
    assert_eq!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .unwrap(),
        0
    );
    assert!(apk.is_file());
    sqlx::query("DELETE FROM local_screen_task_data")
        .execute(state.local_store.pool())
        .await
        .unwrap();
    sqlx::query("DROP TABLE local_recent_artifact")
        .execute(state.local_store.pool())
        .await
        .unwrap();
    assert!(
        cleanup_orphan_task_artifacts(&state.task_repository, &state.paths)
            .await
            .is_err()
    );
    assert!(apk.is_file());
}

#[tokio::test]
async fn artifact_delete_failure_does_not_change_install_result_or_block_history_clear() {
    let (_temp, state) = fixture().await;
    task(
        &state,
        "blocked",
        "smart_screen",
        "install",
        TaskState::Succeeded,
    )
    .await;
    let package = state
        .paths
        .project_task_dir("project", "blocked")
        .unwrap()
        .join("xiaoxin.apk");
    // 用错误文件类型稳定模拟不可清理，而不依赖管理员权限或平台文件锁行为。
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("keep.txt"), "unknown data").unwrap();
    let log = log(&state, "blocked");
    assert!(
        cleanup_finished_task_artifacts(&state.task_repository, &state.paths, "blocked")
            .await
            .is_err()
    );
    assert_eq!(
        state.task_repository.get("blocked").await.unwrap().state,
        TaskState::Succeeded
    );
    assert_eq!(
        clear_finished_task_records(&state, "project")
            .await
            .unwrap(),
        1
    );
    assert!(package.join("keep.txt").is_file() && log.is_file());
}
