use std::path::Path;

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan};
use crate::domain::common::task::{TargetState, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::deployment_service::{LaunchDeploymentInput, run_submitted_deployment};
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry};

const AIO_OPERATIONS: &[&str] = &["first_deploy", "full_upgrade", "service_upgrade"];
const DEPLOYMENT_PAYLOAD_FILE: &str = "deployment-input.json";

pub fn register_production_task_handlers(
    registry: &TaskHandlerRegistry,
    app_handle: AppHandle,
) -> AppResult<()> {
    register_aio_task_handlers(registry, move |envelope, cancellation| {
        let app_handle = app_handle.clone();
        async move { execute_aio_handler(app_handle, envelope, cancellation).await }
    })
}

pub fn register_aio_task_handlers<F, Fut>(
    registry: &TaskHandlerRegistry,
    handler: F,
) -> AppResult<()>
where
    F: Fn(TaskEnvelope, CancellationToken) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = AppResult<()>> + Send + 'static,
{
    for operation_type in AIO_OPERATIONS {
        let operation_handler = handler.clone();
        registry.register("aio", operation_type, move |envelope, cancellation| {
            operation_handler(envelope, cancellation)
        })?;
    }
    Ok(())
}

async fn execute_aio_handler(
    app_handle: AppHandle,
    envelope: TaskEnvelope,
    cancellation: CancellationToken,
) -> AppResult<()> {
    let state = app_handle.state::<FormalAppState>();
    execute_aio_task(&state, envelope, cancellation).await
}

pub async fn execute_aio_task(
    state: &FormalAppState,
    envelope: TaskEnvelope,
    cancellation: CancellationToken,
) -> AppResult<()> {
    let result = execute_aio_handler_inner(state, &envelope, cancellation).await;
    if let Err(error) = &result {
        converge_handler_failure(state, &envelope.local_task_id, error).await;
    }
    if let Ok(task) = state.task_repository.get(&envelope.local_task_id).await
        && (task.state.is_terminal() || task.state == TaskState::FinalizingFailed)
        && let Err(error) = TaskDataLifecycle::new(&state.paths).finalize_task(
            &task.local_project_id,
            &task.id,
            task.state,
        )
    {
        tracing::error!(
            task_id = %task.id,
            error = %crate::core::log_safety::safe_error(&error),
            "finalize task data failed"
        );
    }
    result
}

async fn execute_aio_handler_inner(
    state: &FormalAppState,
    envelope: &TaskEnvelope,
    cancellation: CancellationToken,
) -> AppResult<()> {
    if cancellation.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let payload_ref = envelope
        .payload_ref
        .as_deref()
        .ok_or_else(|| AppError::InvalidConfig("AIO任务缺少payload_ref".into()))?;
    let expected_dir = state
        .paths
        .project_task_dir(&envelope.local_project_id, &envelope.local_task_id)
        .map_err(crate::infrastructure::project_context::map_formal_error)?;
    let payload_path = Path::new(payload_ref);
    if payload_path.parent() != Some(expected_dir.as_path())
        || payload_path.file_name().and_then(|value| value.to_str())
            != Some(DEPLOYMENT_PAYLOAD_FILE)
    {
        return Err(AppError::InvalidConfig(
            "AIO任务payload不在本任务安全目录".into(),
        ));
    }
    let bytes = tokio::fs::read(payload_path)
        .await
        .map_err(|error| AppError::io("读取AIO任务payload", &error))?;
    let expected_sha256 = envelope
        .payload_sha256
        .as_deref()
        .ok_or_else(|| AppError::InvalidConfig("AIO任务缺少payload_sha256".into()))?;
    verify_payload_sha256(&bytes, expected_sha256)?;
    let input = serde_json::from_slice::<LaunchDeploymentInput>(&bytes)
        .map_err(|_| AppError::InvalidConfig("AIO任务payload无法解析".into()))?;
    input.snapshot.validate(&envelope.local_project_id)?;
    let plan = DeploymentPlan::build(input.snapshot.plan.clone())?;
    if mode_code(plan.mode) != envelope.operation_type {
        return Err(AppError::Conflict("AIO任务操作类型与payload不一致".into()));
    }
    let mut envelope_targets = envelope.resource_keys.clone();
    envelope_targets.sort();
    envelope_targets.dedup();
    if envelope_targets != plan.target_macs {
        return Err(AppError::Conflict("AIO任务目标与payload不一致".into()));
    }
    run_submitted_deployment(
        state,
        &envelope.local_project_id,
        &envelope.local_task_id,
        input,
        cancellation,
    )
    .await
    .map(|_| ())
}

fn verify_payload_sha256(bytes: &[u8], expected_sha256: &str) -> AppResult<()> {
    let actual_sha256 = hex::encode(Sha256::digest(bytes));
    if actual_sha256.eq_ignore_ascii_case(expected_sha256) {
        Ok(())
    } else {
        Err(AppError::Integrity {
            operation: "校验AIO任务payload",
        })
    }
}

async fn converge_handler_failure(state: &FormalAppState, task_id: &str, error: &AppError) {
    let Ok(mut task) = state.task_repository.get(task_id).await else {
        return;
    };
    let cancelled = matches!(error, AppError::Cancelled);
    if task.state == TaskState::Queued && cancelled {
        let _ = state
            .task_repository
            .transition(
                task_id,
                TaskState::Queued,
                TaskState::Cancelled,
                Some("QUEUE_CANCELLED"),
                Some("任务在处理器执行前被取消"),
            )
            .await;
    } else if task.state == TaskState::Queued {
        if let Ok(updated) = state
            .task_repository
            .transition(task_id, TaskState::Queued, TaskState::Running, None, None)
            .await
        {
            task = updated;
        }
        let _ = state
            .task_repository
            .transition(
                task_id,
                task.state,
                TaskState::Failed,
                Some("TASK_HANDLER_FAILED"),
                Some(&error.to_string()),
            )
            .await;
    }
    if let Ok(targets) = state.task_repository.targets(task_id).await {
        for target in targets {
            if matches!(target.state, TargetState::Pending | TargetState::Running) {
                let _ = state
                    .task_repository
                    .update_target(
                        task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: if cancelled {
                                TargetState::Cancelled
                            } else {
                                TargetState::Failed
                            },
                            stage: if cancelled {
                                "cancelled".into()
                            } else {
                                "handler_failed".into()
                            },
                            progress_current: target.progress_total.max(100),
                            progress_total: target.progress_total.max(100),
                            fencing_token: target.fencing_token,
                            message_code: Some(if cancelled {
                                "QUEUE_CANCELLED".into()
                            } else {
                                "TASK_HANDLER_FAILED".into()
                            }),
                            message_params_json: None,
                        },
                    )
                    .await;
            }
        }
    }
}

pub fn deployment_payload_file() -> &'static str {
    DEPLOYMENT_PAYLOAD_FILE
}

fn mode_code(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::FirstDeploy => "first_deploy",
        DeploymentMode::FullUpgrade => "full_upgrade",
        DeploymentMode::ServiceUpgrade => "service_upgrade",
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{
        AIO_OPERATIONS, deployment_payload_file, register_aio_task_handlers, verify_payload_sha256,
    };
    use crate::runtime::task_queue::TaskHandlerRegistry;

    #[test]
    fn aio_handler_keys_and_payload_file_are_explicit() {
        assert_eq!(
            AIO_OPERATIONS,
            &["first_deploy", "full_upgrade", "service_upgrade"]
        );
        assert_eq!(deployment_payload_file(), "deployment-input.json");
        let registry = TaskHandlerRegistry::default();
        register_aio_task_handlers(&registry, |_, _| async { Ok(()) })
            .expect("register aio handlers");
        assert_eq!(
            registry.registered_keys().expect("keys"),
            vec![
                ("aio".into(), "first_deploy".into()),
                ("aio".into(), "full_upgrade".into()),
                ("aio".into(), "service_upgrade".into())
            ]
        );
    }

    #[test]
    fn deployment_payload_hash_rejects_any_mutation() {
        let bytes = br#"{"snapshot":"immutable"}"#;
        let expected = hex::encode(Sha256::digest(bytes));
        verify_payload_sha256(bytes, &expected).expect("original payload");
        assert!(verify_payload_sha256(br#"{"snapshot":"changed"}"#, &expected).is_err());
    }
}
