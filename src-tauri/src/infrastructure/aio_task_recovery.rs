use std::time::Duration;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskState;
use crate::formal::app_state::FormalAppState;
use crate::formal::resource_lease_repository::{LeaseRequest, ResourceLeaseRepository};
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::client_instance::application_instance_id;
use crate::infrastructure::deployment_finalization::{
    finalization_lease_conflicts, finalize_deployment_atomically, read_pending_local_finalization,
    shared_operation_state, write_pending_local_finalization,
};
use crate::infrastructure::project_context::project_database_for_finalization;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;

use crate::infrastructure::task_recovery::TaskRecoveryOutcome as FinalizationRetryOutcome;

const FINALIZATION_TAKEOVER_TTL: Duration = Duration::from_secs(90);

pub fn recover<'a>(
    state: &'a FormalAppState,
    task: &'a crate::domain::common::task::TaskRecord,
    force_takeover: bool,
) -> crate::infrastructure::task_recovery::TaskRecoveryFuture<'a> {
    Box::pin(retry_aio_finalization(state, task, force_takeover))
}

pub async fn retry_aio_finalization(
    state: &FormalAppState,
    task: &crate::domain::common::task::TaskRecord,
    force_takeover: bool,
) -> AppResult<FinalizationRetryOutcome> {
    if task.state != TaskState::FinalizingFailed {
        return Err(AppError::Conflict(format!(
            "当前任务不需要补写结果：{}",
            task.state.as_str()
        )));
    }
    let task_dir = state
        .paths
        .project_task_dir(&task.local_project_id, &task.id)
        .map_err(AppError::from)?;
    let mut projection = read_pending_local_finalization(&task_dir)?;
    if task.remote_operation_record_id.as_deref() != Some(projection.operation_id.as_str()) {
        return Err(AppError::Conflict("本地任务与待补写共享操作不一致".into()));
    }
    let pools = project_database_for_finalization(state, &task.local_project_id).await?;
    let shared_state = shared_operation_state(&pools.workbench, &projection.operation_id)
        .await?
        .ok_or_else(|| AppError::NotFound("待补写共享操作不存在".into()))?;
    if shared_state == projection.final_state.as_str() {
        finalize_local_projection(state, &task, &projection).await?;
        return Ok(FinalizationRetryOutcome::Completed);
    }
    if shared_state != "running" {
        return Err(AppError::Conflict(format!(
            "共享任务已经变为{shared_state}，不能用旧结果覆盖"
        )));
    }

    let conflicts = finalization_lease_conflicts(&pools.workbench, &projection.shared).await?;
    if let Some(conflict) = conflicts.iter().find(|conflict| !conflict.takeover_allowed) {
        return Err(AppError::Conflict(format!(
            "目标{}的占用任务已经结束或租约记录异常，不能用旧结果覆盖",
            conflict.resource_key
        )));
    }
    if !conflicts.is_empty() && !force_takeover {
        return Ok(FinalizationRetryOutcome::TakeoverRequired(conflicts));
    }
    if force_takeover && !conflicts.is_empty() {
        let operator = project_operator(state, &task.local_project_id).await?;
        let requests = projection
            .shared
            .leases
            .iter()
            .map(|lease| LeaseRequest {
                resource_type: lease.resource_type.clone(),
                resource_key: lease.resource_key.clone(),
                domain_type: "aio".into(),
                operation_id: projection.operation_id.clone(),
                owner_instance_id: application_instance_id().into(),
                owner_user: operator.clone(),
                ttl: FINALIZATION_TAKEOVER_TTL,
            })
            .collect();
        let repository = ResourceLeaseRepository::new(pools.workbench.clone());
        let grants = repository
            .force_acquire_many(requests)
            .await
            .map_err(AppError::from)?;
        projection.shared.leases = grants.clone();
        if let Err(error) = write_pending_local_finalization(&task_dir, &projection) {
            for grant in &grants {
                let _ = repository.release(grant).await;
            }
            return Err(error);
        }
    }

    match finalize_deployment_atomically(&pools.workbench, projection.shared.clone()).await {
        Ok(()) => {}
        Err(error) if !force_takeover => {
            let conflicts =
                finalization_lease_conflicts(&pools.workbench, &projection.shared).await?;
            if !conflicts.is_empty() {
                return Ok(FinalizationRetryOutcome::TakeoverRequired(conflicts));
            }
            return Err(error);
        }
        Err(error) => return Err(error),
    }
    finalize_local_projection(state, &task, &projection).await?;
    Ok(FinalizationRetryOutcome::Completed)
}

async fn finalize_local_projection(
    state: &FormalAppState,
    task: &crate::domain::common::task::TaskRecord,
    projection: &crate::infrastructure::deployment_finalization::PendingLocalFinalization,
) -> AppResult<()> {
    state
        .task_repository
        .finalize_projection(
            &task.id,
            TaskState::FinalizingFailed,
            projection.final_state,
            &projection.targets,
            &projection.steps,
        )
        .await?;
    if let Err(error) = TaskDataLifecycle::new(&state.paths).finalize_task(
        &task.local_project_id,
        &task.id,
        projection.final_state,
    ) {
        tracing::warn!(
            task_id = %task.id,
            error = %crate::core::log_safety::safe_error(&error),
            "finalized task data cleanup remains deferred"
        );
    }
    Ok(())
}
