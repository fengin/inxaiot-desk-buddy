use std::time::Duration;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::{TargetState, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::formal::config::AppPaths;
use crate::formal::resource_lease_repository::{LeaseRequest, ResourceLeaseRepository};
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::client_instance::application_instance_id;
use crate::infrastructure::deployment_finalization::{
    FinalizationLeaseConflict, finalization_lease_conflicts, finalize_deployment_atomically,
    read_pending_local_finalization, shared_operation_state, write_pending_local_finalization,
};
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
use crate::infrastructure::project_context::project_database_for_finalization;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::runtime::job_supervisor::JobOutcome;
use crate::runtime::task_queue::{TaskQueue, TaskQueueResult};

const FINALIZATION_TAKEOVER_TTL: Duration = Duration::from_secs(90);

#[derive(Clone, Debug)]
pub enum FinalizationRetryOutcome {
    Completed,
    TakeoverRequired(Vec<FinalizationLeaseConflict>),
}

pub async fn retry_pending_local_finalizations(state: &FormalAppState) {
    let Ok(tasks) = state.task_repository.list_active().await else {
        return;
    };
    for task in tasks
        .into_iter()
        .filter(|task| task.state == TaskState::FinalizingFailed)
    {
        if let Err(error) = retry_pending_local_finalization(state, &task.id, false).await {
            tracing::warn!(
                task_id = %task.id,
                error = %crate::core::log_safety::safe_error(&error),
                "pending deployment finalization remains deferred"
            );
        }
    }
}

pub async fn retry_pending_local_finalization(
    state: &FormalAppState,
    task_id: &str,
    force_takeover: bool,
) -> AppResult<FinalizationRetryOutcome> {
    let task = state.task_repository.get(task_id).await?;
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

pub async fn reconcile_queue_result(
    repository: &TaskRepository,
    paths: &AppPaths,
    result: &TaskQueueResult,
) {
    let Ok(mut task) = repository.get(&result.local_task_id).await else {
        return;
    };
    if task.state.is_terminal() || task.state == TaskState::FinalizingFailed {
        return;
    }

    let (next, error_code, message, target_state) = match (&result.outcome, task.state) {
        (JobOutcome::Cancelled, TaskState::Queued) => (
            TaskState::Cancelled,
            "QUEUE_CANCELLED",
            "排队任务已取消，未进入执行器",
            TargetState::Cancelled,
        ),
        (JobOutcome::Cancelled, TaskState::Running) => {
            let Ok(updated) = repository
                .transition(
                    &task.id,
                    TaskState::Running,
                    TaskState::Cancelling,
                    Some("TASK_CANCELLED"),
                    Some("执行器已取消任务，正在收敛状态"),
                )
                .await
            else {
                return;
            };
            task = updated;
            (
                TaskState::Cancelled,
                "TASK_CANCELLED",
                "任务已由执行器取消",
                TargetState::Cancelled,
            )
        }
        (JobOutcome::Cancelled, TaskState::Cancelling) => (
            TaskState::Cancelled,
            "TASK_CANCELLED",
            "任务已由执行器取消",
            TargetState::Cancelled,
        ),
        (JobOutcome::Failed(_), TaskState::Queued) => {
            let Ok(updated) = repository
                .transition(
                    &task.id,
                    TaskState::Queued,
                    TaskState::Running,
                    Some("TASK_DISPATCH_FAILED"),
                    Some("任务处理器启动失败"),
                )
                .await
            else {
                return;
            };
            task = updated;
            (
                TaskState::Failed,
                "TASK_DISPATCH_FAILED",
                "任务处理器启动或执行失败",
                TargetState::Failed,
            )
        }
        (JobOutcome::Failed(_), TaskState::Running | TaskState::Cancelling) => (
            TaskState::Failed,
            "TASK_HANDLER_FAILED",
            "任务处理器异常返回，已统一收敛",
            TargetState::Failed,
        ),
        (JobOutcome::Panicked | JobOutcome::Aborted, TaskState::Queued) => {
            let Ok(updated) = repository
                .transition(
                    &task.id,
                    TaskState::Queued,
                    TaskState::Running,
                    Some("TASK_DISPATCH_ABORTED"),
                    Some("任务处理器未能安全启动"),
                )
                .await
            else {
                return;
            };
            task = updated;
            (
                TaskState::Failed,
                "TASK_DISPATCH_ABORTED",
                "任务处理器panic或被中止",
                TargetState::Failed,
            )
        }
        (
            JobOutcome::Panicked | JobOutcome::Aborted | JobOutcome::Completed,
            TaskState::Running | TaskState::Cancelling,
        ) => (
            TaskState::Interrupted,
            "TASK_OUTCOME_UNSAFE",
            "执行器结束但任务未形成安全终态，需要对账",
            TargetState::Interrupted,
        ),
        (JobOutcome::Completed, TaskState::Queued) => {
            let Ok(updated) = repository
                .transition(
                    &task.id,
                    TaskState::Queued,
                    TaskState::Running,
                    Some("TASK_COMPLETED_WITHOUT_STATE"),
                    Some("执行器完成但任务未进入运行态"),
                )
                .await
            else {
                return;
            };
            task = updated;
            (
                TaskState::Failed,
                "TASK_COMPLETED_WITHOUT_STATE",
                "执行器完成但任务没有持久化执行结果",
                TargetState::Failed,
            )
        }
        _ => return,
    };
    if repository
        .transition(&task.id, task.state, next, Some(error_code), Some(message))
        .await
        .is_err()
    {
        return;
    }
    converge_targets(repository, &task.id, next, target_state, error_code).await;
    let _ = TaskDataLifecycle::new(paths).finalize_task(&task.local_project_id, &task.id, next);
}

pub async fn reconcile_untracked_active_tasks(
    repository: &TaskRepository,
    paths: &AppPaths,
    queue: &TaskQueue,
) {
    let Ok(tasks) = repository.list_active().await else {
        return;
    };
    for task in tasks {
        if task.state != TaskState::FinalizingFailed && !queue.is_tracked(&task.id).await {
            reconcile_queue_result(
                repository,
                paths,
                &TaskQueueResult {
                    local_task_id: task.id,
                    outcome: JobOutcome::Aborted,
                },
            )
            .await;
        }
    }
}

async fn converge_targets(
    repository: &TaskRepository,
    task_id: &str,
    task_state: TaskState,
    target_state: TargetState,
    error_code: &str,
) {
    if let Ok(targets) = repository.targets(task_id).await {
        for target in targets {
            if matches!(target.state, TargetState::Pending | TargetState::Running) {
                let total = target.progress_total.max(100);
                let _ = repository
                    .update_target(
                        task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: target_state,
                            stage: task_state.as_str().into(),
                            progress_current: total,
                            progress_total: total,
                            fencing_token: target.fencing_token,
                            message_code: Some(error_code.into()),
                            message_params_json: None,
                        },
                    )
                    .await;
            }
        }
    }
}

pub async fn prepare_shutdown_tasks(state: &FormalAppState) {
    let Ok(tasks) = state.task_repository.list_active().await else {
        return;
    };
    for task in tasks {
        if task.state == TaskState::Running {
            let _ = state
                .task_repository
                .transition(
                    &task.id,
                    TaskState::Running,
                    TaskState::Cancelling,
                    Some("APP_SHUTDOWN_REQUESTED"),
                    Some("应用正在安全关闭，等待当前任务收敛"),
                )
                .await;
        }
    }
}

pub async fn reconcile_shutdown_outcomes(
    state: &FormalAppState,
    outcomes: &[(String, JobOutcome)],
) {
    for (task_id, outcome) in outcomes {
        let Ok(task) = state.task_repository.get(task_id).await else {
            continue;
        };
        if task.state.is_terminal() {
            continue;
        }
        let (next, error_code, message, target_state) = match (task.state, outcome) {
            (TaskState::Queued, JobOutcome::Cancelled) => (
                TaskState::Cancelled,
                "APP_SHUTDOWN_QUEUED",
                "应用关闭，排队任务未执行",
                TargetState::Cancelled,
            ),
            (TaskState::Running, JobOutcome::Cancelled)
            | (TaskState::Cancelling, JobOutcome::Cancelled) => (
                TaskState::Interrupted,
                "APP_SHUTDOWN_INTERRUPTED",
                "应用关闭时任务未形成最终结果，需要对账",
                TargetState::Interrupted,
            ),
            (TaskState::Running, _)
            | (TaskState::Cancelling, _)
            | (TaskState::FinalizingFailed, _) => (
                TaskState::Interrupted,
                "APP_SHUTDOWN_ABORTED",
                "应用关闭等待超时或执行异常，需要对账",
                TargetState::Interrupted,
            ),
            _ => continue,
        };
        if state
            .task_repository
            .transition(task_id, task.state, next, Some(error_code), Some(message))
            .await
            .is_err()
        {
            continue;
        }
        if let Ok(targets) = state.task_repository.targets(task_id).await {
            for target in targets {
                if matches!(target.state, TargetState::Pending | TargetState::Running) {
                    let total = target.progress_total.max(100);
                    let _ = state
                        .task_repository
                        .update_target(
                            task_id,
                            TargetUpdate {
                                resource_type: target.resource_type,
                                resource_key: target.resource_key,
                                state: target_state,
                                stage: next.as_str().into(),
                                progress_current: total,
                                progress_total: total,
                                fencing_token: target.fencing_token,
                                message_code: Some(error_code.into()),
                                message_params_json: None,
                            },
                        )
                        .await;
                }
            }
        }
        let _ = TaskDataLifecycle::new(&state.paths).finalize_task(
            &task.local_project_id,
            task_id,
            next,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::{prepare_shutdown_tasks, reconcile_queue_result, reconcile_shutdown_outcomes};
    use crate::domain::common::task::{TargetState, TaskState};
    use crate::formal::app_state::FormalAppState;
    use crate::formal::config::AppPaths;
    use crate::formal::job_supervisor::{JobOutcome, JobSupervisor};
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TaskRepository};
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue, TaskQueueResult};

    #[tokio::test]
    async fn shutdown_outcomes_persist_queued_cancel_and_running_interrupt() {
        let temp = tempfile::tempdir().expect("temp");
        let paths = AppPaths::from_data_dir(temp.path()).expect("paths");
        paths.ensure().expect("ensure");
        let local_store = LocalStore::open(&paths.local_db).await.expect("store");
        sqlx::query(concat!(
            "INSERT INTO local_project ",
            "(id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, ",
            "db_password_secret_ref, created_at, updated_at) VALUES ",
            "('project', 'project', 'http://platform.test', 'db.test', 3306, 'user', ",
            "'business', 'workbench', 'secret-ref', '1', '1')"
        ))
        .execute(local_store.pool())
        .await
        .expect("project");
        let repository = TaskRepository::new(local_store.pool().clone());
        for task_id in ["queued", "running", "dispatch-failed"] {
            repository
                .create(CreateTask {
                    id: task_id.into(),
                    local_project_id: "project".into(),
                    remote_operation_record_id: None,
                    domain_type: "screen".into(),
                    operation_type: "test".into(),
                    name: task_id.into(),
                    priority: 0,
                    batch_size: 1,
                    concurrency: 1,
                    payload_ref: None,
                    log_path: paths
                        .project_task_log_path("project", task_id)
                        .expect("task log")
                        .to_string_lossy()
                        .into_owned(),
                    targets: vec![("screen".into(), task_id.into())],
                })
                .await
                .expect("task");
            let mut state = TaskState::Draft;
            for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
                repository
                    .transition(task_id, state, next, None, None)
                    .await
                    .expect("transition");
                state = next;
            }
            if task_id == "running" {
                repository
                    .transition(task_id, TaskState::Queued, TaskState::Running, None, None)
                    .await
                    .expect("running");
            }
        }
        let bus = TaskEventBus::new(16).expect("bus");
        let supervisor = JobSupervisor::default();
        let registry = TaskHandlerRegistry::default();
        let queue = TaskQueue::start(4, 1, registry.clone(), supervisor.clone())
            .await
            .expect("queue");
        let state = FormalAppState {
            local_store,
            secret_store: Arc::new(MemorySecretStore::default()),
            runtime_registry: ProjectRuntimeRegistry::default(),
            job_supervisor: supervisor,
            task_handler_registry: registry,
            task_queue: queue,
            task_event_bus: bus.clone(),
            task_repository: repository.clone(),
            task_event_pipeline: TaskEventPipeline::new(
                repository.clone(),
                bus,
                SensitiveValueRedactor::default(),
            ),
            paths,
        };
        reconcile_queue_result(
            &repository,
            &state.paths,
            &TaskQueueResult {
                local_task_id: "dispatch-failed".into(),
                outcome: JobOutcome::Failed("missing handler".into()),
            },
        )
        .await;
        assert_eq!(
            repository
                .get("dispatch-failed")
                .await
                .expect("dispatch failed")
                .state,
            TaskState::Failed
        );
        assert_eq!(
            repository
                .targets("dispatch-failed")
                .await
                .expect("dispatch failed targets")[0]
                .state,
            TargetState::Failed
        );
        prepare_shutdown_tasks(&state).await;
        assert_eq!(
            repository.get("running").await.expect("running").state,
            TaskState::Cancelling
        );
        reconcile_shutdown_outcomes(
            &state,
            &[
                ("queued".into(), JobOutcome::Cancelled),
                ("running".into(), JobOutcome::Aborted),
            ],
        )
        .await;
        assert_eq!(
            repository.get("queued").await.expect("queued").state,
            TaskState::Cancelled
        );
        assert_eq!(
            repository.get("running").await.expect("running").state,
            TaskState::Interrupted
        );
        assert_eq!(
            repository.targets("queued").await.expect("targets")[0].state,
            TargetState::Cancelled
        );
        assert_eq!(
            repository.targets("running").await.expect("targets")[0].state,
            TargetState::Interrupted
        );
        state.task_queue.shutdown(Duration::from_secs(1)).await;
        state.local_store.close().await;
    }
}
