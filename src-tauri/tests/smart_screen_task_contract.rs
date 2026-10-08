#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::{model::ScreenFields, operation::ScreenOperationInput},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{device, task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
fn fields(ip: &str) -> ScreenFields {
    ScreenFields {
        name: "检查目标".into(),
        ip: ip.into(),
        size: "4".into(),
        ..Default::default()
    }
}
fn input(ids: Vec<String>) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "inspect".into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 2,
        retry_of_operation_id: None, expected_targets: Default::default(),
    }
}

#[tokio::test]
async fn retry_links_only_finished_same_project_action_and_targets() {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    let temp=tempfile::tempdir().unwrap();let state=support::state_at(temp.path(),false).await;
    let project=Stage75Adapter::new(&state).create_project(support::local_input()).await.unwrap().project.id;
    let repo=ScreenRepository::new(state.local_store.pool().clone());
    let screen=repo.save_local(&project,&fields("192.0.2.5"),None,None).await.unwrap();
    let request=input(vec![screen]);let preview=tasks::preflight(&state,&project,request.clone()).await.unwrap();
    let task=tasks::submit(&state,&project,&preview.id,request.clone()).await.unwrap();
    let mut retry=request;retry.retry_of_operation_id=Some(task.clone());
    assert!(tasks::validate_retry(&state,&project,&retry).await.is_err());
    state.task_repository.transition(&task,TaskState::Queued,TaskState::Running,None,None).await.unwrap();
    state.task_repository.transition(&task,TaskState::Running,TaskState::Failed,None,None).await.unwrap();
    assert!(tasks::validate_retry(&state,&project,&retry).await.is_ok());
    assert!(tasks::validate_retry(&state,"other-project",&retry).await.is_err());
    let checked=tasks::preflight(&state,&project,retry.clone()).await.unwrap();
    let (saved,_)=task_data::read_plan(state.local_store.pool(),&project,&checked.id).await.unwrap();
    assert_eq!(saved.input.retry_of_operation_id.as_deref(),Some(task.as_str()));
    retry.action="reboot".into();assert!(tasks::validate_retry(&state,&project,&retry).await.is_err());
    retry.action="inspect".into();retry.target_ids=vec!["other-screen".into()];
    assert!(tasks::validate_retry(&state,&project,&retry).await.is_err());
    support::close(state).await;
}

#[tokio::test]
async fn changed_address_and_cross_project_preview_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let adapter = Stage75Adapter::new(&state);
    let a = adapter
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let b = adapter
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let id = repo
        .save_local(&a, &fields("192.0.2.1"), None, None)
        .await
        .unwrap();
    let request = input(vec![id.clone()]);
    let preview = tasks::preflight(&state, &a, request.clone()).await.unwrap();
    assert!(
        tasks::submit(&state, &b, &preview.id, request.clone())
            .await
            .is_err()
    );
    repo.save_local(&a, &fields("192.0.2.2"), Some(&id), Some(1))
        .await
        .unwrap();
    assert!(
        tasks::submit(&state, &a, &preview.id, request)
            .await
            .is_err()
    );
    support::close(state).await;
}

#[tokio::test]
async fn cleared_screen_preview_cannot_execute_and_recheck_creates_an_independent_task() {
    use inxaiot_desk_buddy_lib::core::error::AppError;
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    use inxaiot_desk_buddy_lib::infrastructure::smart_screen::{maintenance, previews};
    use inxaiot_desk_buddy_lib::interface::commands::task_activity::query_activity_tasks;

    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input()).await.unwrap().project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let screen = repo.save_local(&project, &fields("192.0.2.8"), None, None).await.unwrap();
    let request = input(vec![screen]);
    let checked = tasks::preflight(&state, &project, request.clone()).await.unwrap();
    let (plan, hash) = task_data::read_plan(state.local_store.pool(), &project, &checked.id).await.unwrap();
    let activity = query_activity_tasks(&state, &project, 20).await.unwrap();
    assert!(activity.iter().find(|task| task.id == checked.id).unwrap().clearable);

    assert_eq!(state.task_repository.clear_terminal_for_project(&project).await.unwrap(), 1);
    let error = tasks::submit(&state, &project, &checked.id, request.clone()).await.unwrap_err();
    assert!(matches!(error, AppError::NotFound(message) if message == "检查记录已清空，请重新检查后再执行"));
    let mut maintenance_request = request.clone();
    maintenance_request.action = "reboot".into();
    let error = maintenance::submit(&state, &project, &checked.id, maintenance_request).await.unwrap_err();
    assert!(matches!(error, AppError::NotFound(message) if message == "检查记录已清空，请重新检查后再执行"));
    assert!(repo.snapshot(&project).await.unwrap().observations.is_empty());
    assert!(state.task_repository.list_recent(&project, 20).await.unwrap().is_empty());

    // 模拟页面已读出检查快照、提交前检查才被清理的次序。内存中的旧快照不能绕过事务校验。
    let error = previews::queue_with_id(
        &state, &checked.id, "screen_preflight", &hash, &plan, "stale-screen-execution".into(),
    ).await.unwrap_err();
    assert!(matches!(error, AppError::NotFound(message) if message == "检查记录已清空，请重新检查后再执行"));
    assert!(state.task_repository.get("stale-screen-execution").await.is_err());
    let orphan_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data WHERE local_project_id=?")
        .bind(&project).fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(orphan_count, 0);

    let checked_again = tasks::preflight(&state, &project, request.clone()).await.unwrap();
    let execution = tasks::submit(&state, &project, &checked_again.id, request).await.unwrap();
    assert_eq!(state.task_repository.clear_terminal_for_project(&project).await.unwrap(), 1);
    assert!(state.task_repository.get(&checked_again.id).await.is_err());
    assert_eq!(state.task_repository.get(&execution).await.unwrap().state, TaskState::Queued);
    assert_eq!(task_data::read_plan(state.local_store.pool(), &project, &execution).await.unwrap().0.targets.len(), 1);
    assert!(repo.snapshot(&project).await.unwrap().observations.is_empty());
    support::close(state).await;
}

#[tokio::test]
async fn screen_activity_shows_one_execution_and_clear_counts_it_once() {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    use inxaiot_desk_buddy_lib::interface::commands::task_activity::query_activity_tasks;

    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input()).await.unwrap().project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let screen = repo.save_local(&project, &fields("192.0.2.9"), None, None).await.unwrap();
    let mut request = input(vec![screen]);
    request.action = "ping".into();
    let preview = tasks::preflight(&state, &project, request.clone()).await.unwrap();
    let before = query_activity_tasks(&state, &project, 20).await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].id, preview.id);

    let execution = tasks::submit(&state, &project, &preview.id, request).await.unwrap();
    let after = query_activity_tasks(&state, &project, 20).await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, execution);
    assert_eq!(after[0].operation_type, "ping");
    assert_eq!(state.task_repository.list_recent(&project, 20).await.unwrap().len(), 2);
    assert!(task_data::read_plan(state.local_store.pool(), &project, &preview.id).await.is_ok());
    state.task_repository.transition(&execution, TaskState::Queued, TaskState::Running, None, None).await.unwrap();
    state.task_repository.transition(&execution, TaskState::Running, TaskState::Succeeded, None, None).await.unwrap();

    assert_eq!(state.task_repository.clear_terminal_activity_for_project(&project).await.unwrap(), 1);
    assert!(query_activity_tasks(&state, &project, 20).await.unwrap().is_empty());
    assert!(state.task_repository.list_recent(&project, 20).await.unwrap().is_empty());
    let snapshot_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data WHERE local_project_id=?")
        .bind(&project).fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(snapshot_count, 0);
    support::close(state).await;
}

#[tokio::test]
async fn selected_subset_consumes_original_preview_only_once() {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let first = repo
        .save_local(&project, &fields("192.0.2.1"), None, None)
        .await
        .unwrap();
    let second = repo
        .save_local(&project, &fields("192.0.2.2"), None, None)
        .await
        .unwrap();
    let all = input(vec![first, second.clone()]);
    let preview = tasks::preflight(&state, &project, all.clone())
        .await
        .unwrap();
    let selected = ScreenOperationInput {
        target_ids: vec![second],
        ..all
    };
    let task = tasks::submit(&state, &project, &preview.id, selected.clone())
        .await
        .unwrap();
    assert!(
        tasks::submit(&state, &project, &preview.id, selected)
            .await
            .is_err()
    );
    let (plan, _) = task_data::read_plan(state.local_store.pool(), &project, &task)
        .await
        .unwrap();
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(
        state
            .task_repository
            .list_recent(&project, 100)
            .await
            .unwrap()
            .iter()
            .filter(|t| t.operation_type == "inspect")
            .count(),
        1
    );
    assert!(
        repo.snapshot(&project)
            .await
            .unwrap()
            .observations
            .is_empty()
    );
    support::close(state).await;
}

#[tokio::test]
async fn unsupported_actions_and_cancelled_commands_never_execute() {
    let mut request = input(vec!["missing".into()]);
    request.action = "shell".into();
    assert!(
        inxaiot_desk_buddy_lib::application::smart_screen::operations::validate_input(&request)
            .is_err()
    );
    let cancelled = tokio_util::sync::CancellationToken::new();
    cancelled.cancel();
    let result = device::run(
        std::path::Path::new("not-an-executable"),
        &[],
        std::time::Duration::from_secs(1),
        cancelled,
    )
    .await;
    assert!(matches!(
        result,
        Err(inxaiot_desk_buddy_lib::core::error::AppError::Cancelled)
    ));
}

#[tokio::test]
async fn failed_task_with_protected_results_cannot_be_cleaned_or_lose_project() {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let adapter = Stage75Adapter::new(&state);
    let project = adapter
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let id = repo
        .save_local(&project, &fields("192.0.2.4"), None, None)
        .await
        .unwrap();
    let request = input(vec![id.clone()]);
    let preview = tasks::preflight(&state, &project, request.clone())
        .await
        .unwrap();
    let task = tasks::submit(&state, &project, &preview.id, request)
        .await
        .unwrap();
    state
        .task_repository
        .protect_results(&task, "测试保留外部结果")
        .await
        .unwrap();
    state
        .task_repository
        .transition(&task, TaskState::Queued, TaskState::Running, None, None)
        .await
        .unwrap();
    state
        .task_repository
        .transition(&task, TaskState::Running, TaskState::Failed, None, None)
        .await
        .unwrap();
    state
        .task_repository
        .clear_terminal_for_project(&project)
        .await
        .unwrap();
    assert!(state.task_repository.get(&task).await.is_ok());
    assert!(
        state
            .task_repository
            .has_active_for_project(&project)
            .await
            .unwrap()
    );
    assert!(adapter.delete_project(&project).await.is_err());
    assert!(repo.remove_local(&project, &id).await.is_err());
    assert!(
        !state
            .task_repository
            .list_artifact_cleanup_candidates()
            .await
            .unwrap()
            .iter()
            .any(|t| t.id == task)
    );
    state
        .task_repository
        .begin_result_recovery(&task)
        .await
        .unwrap();
    state.task_repository.resolve_results(&task).await.unwrap();
    state
        .task_repository
        .transition(
            &task,
            TaskState::FinalizingFailed,
            TaskState::Failed,
            None,
            None,
        )
        .await
        .unwrap();
    adapter.delete_project(&project).await.unwrap();
    support::close(state).await;
}

#[tokio::test]
async fn interrupted_read_only_task_is_resolved_without_device_execution() {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let id = repo
        .save_local(&project, &fields("192.0.2.3"), None, None)
        .await
        .unwrap();
    let request = input(vec![id]);
    let preview = tasks::preflight(&state, &project, request.clone())
        .await
        .unwrap();
    let task = tasks::submit(&state, &project, &preview.id, request)
        .await
        .unwrap();
    state
        .task_repository
        .transition(&task, TaskState::Queued, TaskState::Running, None, None)
        .await
        .unwrap();
    state
        .task_repository
        .transition(
            &task,
            TaskState::Running,
            TaskState::Interrupted,
            None,
            None,
        )
        .await
        .unwrap();
    tasks::verify(&state, &project, &task).await.unwrap();
    assert_eq!(
        state.task_repository.get(&task).await.unwrap().state,
        TaskState::Failed
    );
    assert!(
        repo.snapshot(&project)
            .await
            .unwrap()
            .observations
            .is_empty()
    );
    assert!(
        task_data::read_results(state.local_store.pool(), &project, &task)
            .await
            .unwrap()
            .finished
    );
    support::close(state).await;
}

#[tokio::test]
async fn cancelled_maintenance_batch_never_calls_devices_and_releases_local_guards() {
    use inxaiot_desk_buddy_lib::{
        domain::{common::task::TaskState, smart_screen::operation::ScreenPlan},
        infrastructure::smart_screen::{maintenance, previews},
        runtime::task_queue::TaskEnvelope,
    };
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut targets = Vec::new();
    for ip in ["192.0.2.10", "192.0.2.11", "192.0.2.12"] {
        let id = repo
            .save_local(&project, &fields(ip), None, None)
            .await
            .unwrap();
        targets.push(repo.asset(&project, &id).await.unwrap());
    }
    let mut request = input(targets.iter().map(|screen| screen.id.clone()).collect());
    request.action = "reboot".into();
    let plan = ScreenPlan {
        project_id: project.clone(),
        input: request.clone(),
        targets,
        business_project_id: None,
        data_source_id: None,
        operator: "测试".into(),
        instance_id: "测试电脑".into(),
        created_at: inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::now(),
        detail: serde_json::to_value(maintenance::MaintenancePlan {
            apk: None,
            app_config: None,
            observations: Default::default(),
            request_ids: Default::default(),
        })
        .unwrap(),
    };
    let preview = uuid::Uuid::now_v7().to_string();
    previews::save(&state, &preview, &plan, maintenance::PREVIEW)
        .await
        .unwrap();
    let task = maintenance::submit(&state, &project, &preview, request)
        .await
        .unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel();
    maintenance::run(
        &state,
        TaskEnvelope {
            local_task_id: task.clone(),
            local_project_id: project.clone(),
            domain_type: "smart_screen".into(),
            operation_type: "reboot".into(),
            resource_keys: vec![],
            priority: 0,
            payload_ref: None,
            payload_sha256: None,
        },
        token,
    )
    .await
    .unwrap();
    assert_eq!(
        state.task_repository.get(&task).await.unwrap().state,
        TaskState::Cancelled
    );
    assert!(
        !state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    assert!(
        repo.snapshot(&project)
            .await
            .unwrap()
            .observations
            .is_empty()
    );
    assert!(
        task_data::read_results(state.local_store.pool(), &project, &task)
            .await
            .unwrap()
            .targets
            .values()
            .all(|r| r.device
                == inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Cancelled)
    );
    support::close(state).await;
}
