use std::time::Duration;

use sqlx::MySqlPool;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

use crate::application::deployment_executor::{DeploymentExecutionSummary, DeploymentTargetState};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan};
use crate::domain::common::task::{StepState, TargetState, TaskRecord, TaskState};
use crate::formal::aio_node_repository::ServiceVersionWrite;
use crate::formal::app_state::FormalAppState;
use crate::formal::operation_repository::{
    OperationFinalResult, OperationRepository, OperationStart, TargetFinalResult,
};
use crate::formal::resource_lease_repository::{LeaseGrant, LeaseRequest, ResourceLeaseRepository};
use crate::infrastructure::deployment_finalization::{
    AtomicDeploymentFinalization, AtomicTargetFinalization, PendingLocalFinalization,
    finalize_deployment_atomically, shared_operation_state, write_pending_local_finalization,
};
use crate::infrastructure::deployment_progress::{
    DEPLOYMENT_PROGRESS_TOTAL, DEPLOYMENT_REMOTE_PROGRESS_START,
};
use crate::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskStepWrite,
};
use crate::infrastructure::project_context::{map_formal_error, project_database};

#[derive(Clone)]
pub struct DeploymentControlHandle {
    pub local_task_id: String,
    pub operation_id: String,
    pub operation_version: u64,
    pub plan: DeploymentPlan,
    pub leases: Vec<LeaseGrant>,
}

pub const DEPLOYMENT_LEASE_TTL: Duration = Duration::from_secs(90);
pub const DEPLOYMENT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

pub struct DeploymentHeartbeatGuard {
    stop: CancellationToken,
    join: JoinHandle<AppResult<u64>>,
}

pub async fn create_deployment_task(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    plan: &DeploymentPlan,
    remote_operation_record_id: Option<String>,
) -> AppResult<TaskRecord> {
    create_deployment_task_with_payload(
        state,
        local_project_id,
        local_task_id,
        plan,
        remote_operation_record_id,
        None,
    )
    .await
}

pub async fn create_deployment_task_with_payload(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    plan: &DeploymentPlan,
    remote_operation_record_id: Option<String>,
    payload_ref: Option<String>,
) -> AppResult<TaskRecord> {
    let operation_type = mode_code(plan.mode);
    state
        .task_repository
        .create(CreateTask {
            id: local_task_id.to_string(),
            local_project_id: local_project_id.into(),
            remote_operation_record_id,
            domain_type: "aio".into(),
            operation_type: operation_type.into(),
            name: format!("{} · {}", mode_label(plan.mode), plan.artifact_name),
            priority: 0,
            batch_size: plan.batch_size,
            concurrency: plan.concurrency,
            payload_ref,
            log_path: state
                .paths
                .project_task_log_path(local_project_id, local_task_id)
                .map_err(map_formal_error)?
                .to_string_lossy()
                .into_owned(),
            targets: plan
                .target_macs
                .iter()
                .map(|mac| ("aio".into(), mac.clone()))
                .collect(),
        })
        .await
}

impl DeploymentHeartbeatGuard {
    pub async fn stop(self) -> AppResult<u64> {
        self.stop.cancel();
        self.join
            .await
            .map_err(|_| AppError::Conflict("部署心跳任务异常结束".into()))?
    }
}

pub fn start_deployment_heartbeat(
    pool: MySqlPool,
    operation_id: String,
    operation_version: u64,
    leases: Vec<LeaseGrant>,
    execution_cancellation: CancellationToken,
) -> DeploymentHeartbeatGuard {
    start_deployment_heartbeat_with_timing(
        pool,
        operation_id,
        operation_version,
        leases,
        execution_cancellation,
        DEPLOYMENT_HEARTBEAT_INTERVAL,
        DEPLOYMENT_LEASE_TTL,
    )
}

pub fn start_deployment_heartbeat_with_timing(
    pool: MySqlPool,
    operation_id: String,
    operation_version: u64,
    leases: Vec<LeaseGrant>,
    execution_cancellation: CancellationToken,
    interval: Duration,
    ttl: Duration,
) -> DeploymentHeartbeatGuard {
    let stop = CancellationToken::new();
    let worker_stop = stop.clone();
    let join = tokio::spawn(async move {
        let leases_repository = ResourceLeaseRepository::new(pool.clone());
        let operations = OperationRepository::new(pool);
        let mut version = operation_version;
        let mut ticker = tokio::time::interval(interval.max(Duration::from_millis(10)));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        ticker.tick().await;
        loop {
            tokio::select! {
                _ = worker_stop.cancelled() => return Ok(version),
                _ = ticker.tick() => {
                    let heartbeat_result = async {
                        for lease in &leases {
                            leases_repository
                                .heartbeat(lease, ttl)
                                .await
                                .map_err(map_formal_error)?;
                        }
                        version = operations
                            .heartbeat(&operation_id, version)
                            .await
                            .map_err(map_formal_error)?;
                        AppResult::Ok(())
                    }
                    .await;
                    if let Err(error) = heartbeat_result {
                        execution_cancellation.cancel();
                        return Err(AppError::Conflict(format!(
                            "部署租约或操作心跳失效，已停止派发新步骤：{error}"
                        )));
                    }
                }
            }
        }
    });
    DeploymentHeartbeatGuard { stop, join }
}

pub async fn start_deployment_control(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    plan: DeploymentPlan,
    operator_name: &str,
    instance_id: &str,
    cancellation: &CancellationToken,
) -> AppResult<DeploymentControlHandle> {
    if cancellation.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let pools = project_database(state, local_project_id).await?;
    let operations = OperationRepository::new(pools.workbench.clone());
    let operation_type = mode_code(plan.mode);
    let record = operations
        .start(OperationStart {
            domain_type: "aio".into(),
            operation_type: operation_type.into(),
            operation_name: mode_label(plan.mode).into(),
            operator_name: operator_name.into(),
            instance_id: instance_id.into(),
            targets: plan
                .target_macs
                .iter()
                .map(|mac| ("aio".into(), mac.clone()))
                .collect(),
            artifact_name: Some(plan.artifact_name.clone()),
            artifact_version: Some(plan.artifact_version.clone()),
            operation_summary: Some(serde_json::json!({
                "mode": operation_type,
                "serviceName": plan.service_name,
                "imageName": plan.image_name,
                "batchSize": plan.batch_size,
                "concurrency": plan.concurrency
            })),
            retry_of_operation_id: None,
        })
        .await
        .map_err(map_formal_error)?;
    if cancellation.is_cancelled() {
        finalize_started_operation(
            &operations,
            &record,
            &plan.target_macs,
            "用户在共享操作创建期间取消了任务",
            true,
        )
        .await;
        return Err(AppError::Cancelled);
    }
    let lease_repository = ResourceLeaseRepository::new(pools.workbench.clone());
    let leases = match lease_repository
        .acquire_many(
            plan.target_macs
                .iter()
                .map(|mac| LeaseRequest {
                    resource_type: "aio".into(),
                    resource_key: mac.clone(),
                    domain_type: "aio".into(),
                    operation_id: record.id.clone(),
                    owner_instance_id: instance_id.into(),
                    owner_user: operator_name.into(),
                    ttl: DEPLOYMENT_LEASE_TTL,
                })
                .collect(),
        )
        .await
    {
        Ok(leases) => leases,
        Err(error) => {
            let summary = error.to_string();
            let cancelled = cancellation.is_cancelled()
                || deployment_start_was_cancelled(state, local_task_id).await;
            finalize_started_operation(
                &operations,
                &record,
                &plan.target_macs,
                &summary,
                cancelled,
            )
            .await;
            return Err(map_formal_error(error));
        }
    };
    if cancellation.is_cancelled() {
        let error = AppError::Cancelled;
        rollback_started_control(
            StartedControlRollback {
                state,
                local_task_id,
                lease_repository: &lease_repository,
                leases: &leases,
                operations: &operations,
                record: &record,
                targets: &plan.target_macs,
                cancellation_requested: true,
            },
            &error,
        )
        .await;
        return Err(error);
    }
    let task_result = match state.task_repository.get(local_task_id).await {
        Ok(task) => {
            if !matches!(
                task.state,
                TaskState::Draft | TaskState::Queued | TaskState::Running
            ) || task.local_project_id != local_project_id
                || task.domain_type != "aio"
                || task.operation_type != operation_type
                || task.remote_operation_record_id.is_some()
            {
                Err(AppError::Conflict(format!(
                    "预创建任务状态或归属不匹配：{local_task_id}"
                )))
            } else {
                state
                    .task_repository
                    .link_operation_in_state(local_task_id, &record.id, task.state)
                    .await
            }
        }
        Err(AppError::NotFound(_)) => {
            create_deployment_task(
                state,
                local_project_id,
                local_task_id,
                &plan,
                Some(record.id.clone()),
            )
            .await
        }
        Err(error) => Err(error),
    };
    let task = match task_result {
        Ok(task) => task,
        Err(error) => {
            rollback_started_control(
                StartedControlRollback {
                    state,
                    local_task_id,
                    lease_repository: &lease_repository,
                    leases: &leases,
                    operations: &operations,
                    record: &record,
                    targets: &plan.target_macs,
                    cancellation_requested: cancellation.is_cancelled(),
                },
                &error,
            )
            .await;
            return Err(error);
        }
    };
    let local_setup = async {
        let mut current = task.state;
        let transitions = match current {
            TaskState::Running => Vec::new(),
            TaskState::Queued => vec![TaskState::Running],
            _ => vec![
                TaskState::Checking,
                TaskState::Ready,
                TaskState::Queued,
                TaskState::Running,
            ],
        };
        for next in transitions {
            state
                .task_repository
                .transition(local_task_id, current, next, None, None)
                .await?;
            current = next;
        }
        let latest = state.task_repository.get(local_task_id).await?;
        if latest.state != TaskState::Running {
            return Err(AppError::Conflict(format!(
                "部署任务已不再执行，停止创建控制面：{}",
                latest.state.as_str()
            )));
        }
        for lease in &leases {
            state
                .task_repository
                .update_target(
                    local_task_id,
                    TargetUpdate {
                        resource_type: "aio".into(),
                        resource_key: lease.resource_key.clone(),
                        state: TargetState::Running,
                        stage: "lease_acquired".into(),
                        progress_current: DEPLOYMENT_REMOTE_PROGRESS_START,
                        progress_total: DEPLOYMENT_PROGRESS_TOTAL,
                        fencing_token: Some(lease.fencing_token),
                        message_code: None,
                        message_params_json: None,
                    },
                )
                .await?;
            for step in &plan.steps {
                state
                    .task_repository
                    .save_step(
                        local_task_id,
                        TaskStepWrite {
                            id: step_id(local_task_id, &lease.resource_key, &step.code),
                            resource_type: Some("aio".into()),
                            resource_key: Some(lease.resource_key.clone()),
                            step_code: step.code.clone(),
                            state: StepState::Pending,
                            error_code: None,
                            message: Some(step.label.clone()),
                        },
                    )
                    .await?;
            }
        }
        AppResult::Ok(())
    }
    .await;
    if let Err(error) = local_setup {
        rollback_started_control(
            StartedControlRollback {
                state,
                local_task_id,
                lease_repository: &lease_repository,
                leases: &leases,
                operations: &operations,
                record: &record,
                targets: &plan.target_macs,
                cancellation_requested: cancellation.is_cancelled(),
            },
            &error,
        )
        .await;
        return Err(error);
    }
    Ok(DeploymentControlHandle {
        local_task_id: local_task_id.to_string(),
        operation_id: record.id,
        operation_version: record.version,
        plan,
        leases,
    })
}

struct StartedControlRollback<'a> {
    state: &'a FormalAppState,
    local_task_id: &'a str,
    lease_repository: &'a ResourceLeaseRepository,
    leases: &'a [LeaseGrant],
    operations: &'a OperationRepository,
    record: &'a crate::formal::operation_repository::OperationRecord,
    targets: &'a [String],
    cancellation_requested: bool,
}

async fn rollback_started_control(context: StartedControlRollback<'_>, error: &AppError) {
    for lease in context.leases {
        let _ = context.lease_repository.release(lease).await;
    }
    let cancelled = context.cancellation_requested
        || deployment_start_was_cancelled(context.state, context.local_task_id).await;
    finalize_started_operation(
        context.operations,
        context.record,
        context.targets,
        &error.to_string(),
        cancelled,
    )
    .await;
}

async fn deployment_start_was_cancelled(state: &FormalAppState, local_task_id: &str) -> bool {
    state
        .task_repository
        .get(local_task_id)
        .await
        .is_ok_and(|task| matches!(task.state, TaskState::Cancelling | TaskState::Cancelled))
}

async fn finalize_started_operation(
    operations: &OperationRepository,
    record: &crate::formal::operation_repository::OperationRecord,
    targets: &[String],
    summary: &str,
    cancelled: bool,
) {
    let result_state = if cancelled { "cancelled" } else { "failed" };
    let result_summary = if cancelled {
        "部署启动已取消"
    } else {
        "部署启动失败"
    };
    let error_code = if cancelled {
        "CANCELLED"
    } else {
        "DEPLOYMENT_START_FAILED"
    };
    for mac in targets {
        let _ = operations
            .finalize_target(TargetFinalResult {
                operation_id: record.id.clone(),
                resource_type: "aio".into(),
                resource_key: mac.clone(),
                result_state: result_state.into(),
                before_version: None,
                after_version: None,
                result_summary: Some(result_summary.into()),
                error_code: Some(error_code.into()),
                error_summary: Some(summary.into()),
            })
            .await;
    }
    let _ = operations
        .finalize(OperationFinalResult {
            operation_id: record.id.clone(),
            expected_version: record.version,
            state: result_state.into(),
            result_summary: Some(result_summary.into()),
            error_code: Some(error_code.into()),
            error_summary: Some(summary.into()),
        })
        .await;
}

pub async fn finalize_deployment_control(
    state: &FormalAppState,
    local_project_id: &str,
    handle: DeploymentControlHandle,
    summary: DeploymentExecutionSummary,
) -> AppResult<()> {
    let pools = project_database(state, local_project_id).await?;
    let mut shared_targets = Vec::with_capacity(summary.targets.len());
    for target in &summary.targets {
        let (state_name, error_code) = match target.state {
            DeploymentTargetState::Succeeded => ("succeeded", None),
            DeploymentTargetState::Failed | DeploymentTargetState::Panicked => {
                ("failed", Some("DEPLOYMENT_FAILED".into()))
            }
            DeploymentTargetState::Cancelled => ("cancelled", Some("CANCELLED".into())),
        };
        let target_result = TargetFinalResult {
            operation_id: handle.operation_id.clone(),
            resource_type: "aio".into(),
            resource_key: target.mac.clone(),
            result_state: state_name.into(),
            before_version: None,
            after_version: (state_name == "succeeded")
                .then(|| handle.plan.artifact_version.clone()),
            result_summary: Some(if state_name == "succeeded" {
                "部署步骤和健康检查完成".into()
            } else {
                "部署未成功".into()
            }),
            error_code: error_code.clone(),
            error_summary: target.error.clone(),
        };
        let mut service_versions = Vec::new();
        if state_name == "succeeded" {
            for (service, image) in &handle.plan.images {
                let (image_name, version) = image
                    .rsplit_once(':')
                    .map(|(name, version)| (name.to_string(), version.to_string()))
                    .unwrap_or_else(|| (image.clone(), image.clone()));
                service_versions.push(ServiceVersionWrite {
                    mac: target.mac.clone(),
                    service_name: service.clone(),
                    expected_image_name: Some(image_name.clone()),
                    expected_version: Some(version.clone()),
                    observed_image_name: None,
                    observed_version: None,
                    source_operation_id: Some(handle.operation_id.clone()),
                });
            }
        }
        shared_targets.push(AtomicTargetFinalization {
            result: target_result,
            service_versions,
            mark_operation_success: state_name == "succeeded",
        });
    }
    let final_state = if summary.failure_count == 0 && summary.cancelled_count == 0 {
        "succeeded"
    } else if summary.success_count > 0 {
        "partially_succeeded"
    } else if summary.cancelled_count > 0 && summary.failure_count == 0 {
        "cancelled"
    } else {
        "failed"
    };
    let current_task = state.task_repository.get(&handle.local_task_id).await?;
    let task_final = if current_task.state == TaskState::Cancelling {
        if summary.failure_count > 0 {
            TaskState::Failed
        } else {
            TaskState::Cancelled
        }
    } else {
        TaskState::parse(final_state)?
    };
    let mut local_targets = Vec::with_capacity(summary.targets.len());
    let mut local_steps = Vec::with_capacity(summary.targets.len() * handle.plan.steps.len());
    for target in &summary.targets {
        let (target_state, step_state, error_code, terminal_stage) =
            local_target_terminal_projection(&target.state);
        local_targets.push(TargetUpdate {
            resource_type: "aio".into(),
            resource_key: target.mac.clone(),
            state: target_state,
            stage: terminal_stage.into(),
            progress_current: 100,
            progress_total: 100,
            fencing_token: None,
            message_code: None,
            message_params_json: None,
        });
        local_steps.extend(handle.plan.steps.iter().map(|step| TaskStepWrite {
            id: step_id(&handle.local_task_id, &target.mac, &step.code),
            resource_type: Some("aio".into()),
            resource_key: Some(target.mac.clone()),
            step_code: step.code.clone(),
            state: step_state,
            error_code: error_code.clone(),
            message: target.error.clone().or_else(|| Some(step.label.clone())),
        }));
    }
    let task_dir = state
        .paths
        .project_task_dir(local_project_id, &handle.local_task_id)
        .map_err(map_formal_error)?;
    write_pending_local_finalization(
        &task_dir,
        &PendingLocalFinalization {
            operation_id: handle.operation_id.clone(),
            final_state: task_final,
            targets: local_targets.clone(),
            steps: local_steps.clone(),
        },
    )?;
    let atomic_finalization = AtomicDeploymentFinalization {
        operation: OperationFinalResult {
            operation_id: handle.operation_id.clone(),
            expected_version: handle.operation_version,
            state: final_state.into(),
            result_summary: Some(format!(
                "成功{}，失败{}，取消{}",
                summary.success_count, summary.failure_count, summary.cancelled_count
            )),
            error_code: None,
            error_summary: None,
        },
        targets: shared_targets,
        leases: handle.leases.clone(),
    };
    let mut shared_error = None;
    for attempt in 0..3 {
        match finalize_deployment_atomically(&pools.workbench, atomic_finalization.clone()).await {
            Ok(()) => {
                shared_error = None;
                break;
            }
            Err(error) => {
                if shared_operation_state(&pools.workbench, &handle.operation_id)
                    .await
                    .ok()
                    .flatten()
                    .as_deref()
                    == Some(final_state)
                {
                    shared_error = None;
                    break;
                }
                shared_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(100 * (attempt + 1))).await;
                }
            }
        }
    }
    if let Some(error) = shared_error {
        return Err(error);
    }
    let mut last_error = None;
    for attempt in 0..3 {
        match state
            .task_repository
            .finalize_projection(
                &handle.local_task_id,
                current_task.state,
                task_final,
                &local_targets,
                &local_steps,
            )
            .await
        {
            Ok(()) => return Ok(()),
            Err(error) => {
                if state
                    .task_repository
                    .get(&handle.local_task_id)
                    .await
                    .is_ok_and(|task| task.state == task_final)
                {
                    return Ok(());
                }
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(50 * (attempt + 1))).await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::Conflict("本地最终化投影失败".into())))
}

fn local_target_terminal_projection(
    state: &DeploymentTargetState,
) -> (TargetState, StepState, Option<String>, &'static str) {
    match state {
        DeploymentTargetState::Succeeded => (
            TargetState::Succeeded,
            StepState::Succeeded,
            None,
            "completed",
        ),
        DeploymentTargetState::Failed | DeploymentTargetState::Panicked => (
            TargetState::Failed,
            StepState::Failed,
            Some("DEPLOYMENT_FAILED".to_string()),
            "failed",
        ),
        DeploymentTargetState::Cancelled => (
            TargetState::Cancelled,
            StepState::Cancelled,
            Some("CANCELLED".to_string()),
            "cancelled",
        ),
    }
}

pub async fn mark_deployment_interrupted(
    state: &FormalAppState,
    handle: &DeploymentControlHandle,
    error_code: &str,
    summary: &str,
) -> AppResult<()> {
    let current_progress = state
        .task_repository
        .targets(&handle.local_task_id)
        .await?
        .into_iter()
        .map(|target| {
            (
                (target.resource_type, target.resource_key),
                (target.progress_current, target.progress_total),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    for lease in &handle.leases {
        let (progress_current, progress_total) = current_progress
            .get(&(lease.resource_type.clone(), lease.resource_key.clone()))
            .copied()
            .unwrap_or((DEPLOYMENT_REMOTE_PROGRESS_START, DEPLOYMENT_PROGRESS_TOTAL));
        let progress_total = progress_total.max(DEPLOYMENT_PROGRESS_TOTAL);
        state
            .task_repository
            .update_target(
                &handle.local_task_id,
                TargetUpdate {
                    resource_type: lease.resource_type.clone(),
                    resource_key: lease.resource_key.clone(),
                    state: TargetState::Interrupted,
                    stage: "needs_reconcile".into(),
                    progress_current: progress_current.min(progress_total),
                    progress_total,
                    fencing_token: Some(lease.fencing_token),
                    message_code: Some(error_code.into()),
                    message_params_json: None,
                },
            )
            .await?;
    }
    let task = state.task_repository.get(&handle.local_task_id).await?;
    if !task.state.is_terminal() {
        state
            .task_repository
            .transition(
                &handle.local_task_id,
                task.state,
                TaskState::Interrupted,
                Some(error_code),
                Some(summary),
            )
            .await?;
    }
    Ok(())
}

pub async fn mark_deployment_finalizing_failed(
    state: &FormalAppState,
    handle: &DeploymentControlHandle,
    summary: &str,
) -> AppResult<()> {
    let task = state.task_repository.get(&handle.local_task_id).await?;
    if !task.state.is_terminal() && task.state != TaskState::FinalizingFailed {
        state
            .task_repository
            .transition(
                &handle.local_task_id,
                task.state,
                TaskState::FinalizingFailed,
                Some("FINALIZATION_FAILED"),
                Some(summary),
            )
            .await?;
    }
    Ok(())
}

pub(crate) fn mode_code(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::FirstDeploy => "first_deploy",
        DeploymentMode::FullUpgrade => "full_upgrade",
        DeploymentMode::ServiceUpgrade => "service_upgrade",
    }
}

pub(crate) fn mode_label(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::FirstDeploy => "首次部署",
        DeploymentMode::FullUpgrade => "整包升级",
        DeploymentMode::ServiceUpgrade => "单服升级",
    }
}

fn step_id(task_id: &str, mac: &str, code: &str) -> String {
    format!("{task_id}:{mac}:{code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_target_terminal_stage_matches_its_result() {
        let cases = [
            (
                DeploymentTargetState::Succeeded,
                TargetState::Succeeded,
                StepState::Succeeded,
                None,
                "completed",
            ),
            (
                DeploymentTargetState::Failed,
                TargetState::Failed,
                StepState::Failed,
                Some("DEPLOYMENT_FAILED"),
                "failed",
            ),
            (
                DeploymentTargetState::Panicked,
                TargetState::Failed,
                StepState::Failed,
                Some("DEPLOYMENT_FAILED"),
                "failed",
            ),
            (
                DeploymentTargetState::Cancelled,
                TargetState::Cancelled,
                StepState::Cancelled,
                Some("CANCELLED"),
                "cancelled",
            ),
        ];

        for (input, expected_target, expected_step, expected_error, expected_stage) in cases {
            let (target, step, error, stage) = local_target_terminal_projection(&input);
            assert_eq!(target, expected_target);
            assert_eq!(step, expected_step);
            assert_eq!(error.as_deref(), expected_error);
            assert_eq!(stage, expected_stage);
        }
    }
}
