use super::{task_data, tasks};
use crate::core::error::{AppError, AppResult};
use crate::domain::{
    common::task::{TargetState, TaskState},
    smart_screen::operation::ScreenPlan,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::runtime::task_queue::TaskEnvelope;

pub async fn save(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    kind: &str,
) -> AppResult<()> {
    let hash = task_data::save_plan(state.local_store.pool(), id, plan).await?;
    let mut input = tasks::create_task(state, id, plan, kind)?;
    input
        .targets
        .push(("preflight_internal".into(), "common".into()));
    state.task_repository.create(input.clone()).await?;
    state
        .task_repository
        .transition(id, TaskState::Draft, TaskState::Checking, None, None)
        .await?;
    for (resource_type, resource_key) in input.targets {
        state
            .task_repository
            .update_target(
                id,
                TargetUpdate {
                    resource_type,
                    resource_key,
                    state: TargetState::Succeeded,
                    stage: "检查完成".into(),
                    progress_current: 1,
                    progress_total: 1,
                    fencing_token: None,
                    message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                    message_params_json: None,
                },
            )
            .await?;
    }
    state
        .task_repository
        .bind_preflight_snapshot(id, &plan.project_id, kind, &hash)
        .await?;
    state
        .task_repository
        .transition(id, TaskState::Checking, TaskState::Succeeded, None, None)
        .await?;
    Ok(())
}
pub async fn queue(
    state: &FormalAppState,
    preview: &str,
    kind: &str,
    hash: &str,
    plan: &ScreenPlan,
) -> AppResult<String> {
    queue_with_id(
        state,
        preview,
        kind,
        hash,
        plan,
        uuid::Uuid::now_v7().to_string(),
    )
    .await
}

pub async fn queue_with_id(
    state: &FormalAppState,
    preview: &str,
    kind: &str,
    hash: &str,
    plan: &ScreenPlan,
    id: String,
) -> AppResult<String> {
    if plan.targets.is_empty() {
        return Err(AppError::InvalidConfig("请选择通过检查的目标".into()));
    }
    super::takeover::before_submit(state, plan).await?;
    task_data::save_plan(state.local_store.pool(), &id, plan).await?;
    let input = tasks::create_task(state, &id, plan, &plan.input.action)?;
    state
        .task_repository
        .create_queued_from_preflight_selection(preview, kind, hash, input)
        .await?;
    if !crate::domain::smart_screen::operation::READ_ACTIONS.contains(&plan.input.action.as_str()) {
        if let Err(error) = state
            .task_repository
            .protect_results(&id, "屏操作结果尚未全部核实保存")
            .await
        {
            let _ = state
                .task_repository
                .transition(
                    &id,
                    TaskState::Queued,
                    TaskState::Cancelled,
                    None,
                    Some("任务未开始"),
                )
                .await;
            let _ = state.task_repository.resolve_results(&id).await;
            return Err(error);
        }
    }
    let envelope = TaskEnvelope {
        local_task_id: id.clone(),
        local_project_id: plan.project_id.clone(),
        domain_type: tasks::DOMAIN.into(),
        operation_type: plan.input.action.clone(),
        resource_keys: plan
            .targets
            .iter()
            .map(|s| format!("{}:{}", plan.project_id, s.id))
            .collect(),
        priority: 0,
        payload_ref: None,
        payload_sha256: None,
    };
    if let Err(error) = state.task_queue.enqueue(envelope).await {
        state.task_repository.resolve_results(&id).await?;
        state
            .task_repository
            .transition(
                &id,
                TaskState::Queued,
                TaskState::Cancelled,
                None,
                Some("任务未开始，未执行设备或平台写入"),
            )
            .await?;
        return Err(error);
    }
    Ok(id)
}
