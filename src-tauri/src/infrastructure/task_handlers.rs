use std::collections::BTreeMap;
use std::path::Path;

use sha2::{Digest, Sha256};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan};
use crate::domain::common::task::{TargetState, TaskEventLevel, TaskState};
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
    crate::infrastructure::smart_screen::tasks::register(registry, app_handle.clone())?;
    crate::infrastructure::smart_screen::registration::register(registry, app_handle.clone())?;
    crate::infrastructure::smart_screen::value_updates::register(registry, app_handle.clone())?;
    crate::infrastructure::smart_screen::maintenance::register(registry, app_handle.clone())?;
    let inspection_app = app_handle.clone();
    registry.register(
        "aio",
        crate::infrastructure::service_inspection::SERVICE_INSPECTION_OPERATION,
        move |envelope, cancellation| {
            let app = inspection_app.clone();
            async move {
                let state = app.state::<FormalAppState>();
                crate::infrastructure::service_inspection::execute_service_inspection(
                    &state,
                    envelope,
                    cancellation,
                )
                .await
            }
        },
    )?;
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
        record_early_handler_failure(state, &envelope.local_task_id, error).await;
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

async fn record_early_handler_failure(state: &FormalAppState, task_id: &str, error: &AppError) {
    let Ok(task) = state.task_repository.get(task_id).await else {
        return;
    };
    if task.state.is_terminal() || task.state == TaskState::FinalizingFailed {
        return;
    }
    let cancelled = matches!(error, AppError::Cancelled);
    let _ = state
        .task_event_pipeline
        .emit(
            task_id,
            TaskEventInput {
                resource_type: None,
                resource_key: None,
                stage: "启动部署任务".into(),
                status: if cancelled { "cancelled" } else { "failed" }.into(),
                progress_current: None,
                progress_total: None,
                level: if cancelled {
                    TaskEventLevel::Warn
                } else {
                    TaskEventLevel::Error
                },
                message_code: if cancelled {
                    "TASK_HANDLER_CANCELLED"
                } else {
                    "TASK_HANDLER_FAILED"
                }
                .into(),
                message_params: BTreeMap::new(),
                message: Some(if cancelled {
                    "部署任务在启动前已取消".into()
                } else {
                    format!("部署任务启动失败：{error}")
                }),
            },
        )
        .await;
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
    use std::sync::Arc;

    use sha2::{Digest, Sha256};

    use super::{
        AIO_OPERATIONS, deployment_payload_file, execute_aio_task, register_aio_task_handlers,
        verify_payload_sha256,
    };
    use crate::domain::common::task::TaskState;
    use crate::formal::app_state::FormalAppState;
    use crate::formal::config::AppPaths;
    use crate::formal::job_supervisor::JobSupervisor;
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TaskRepository};
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry, TaskQueue};

    async fn local_state() -> (tempfile::TempDir, FormalAppState) {
        let temp = tempfile::tempdir().expect("temporary app data");
        let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
        paths.ensure().expect("app directories");
        let local_store = LocalStore::open(&paths.local_db)
            .await
            .expect("local store");
        sqlx::query(
            "INSERT INTO local_project \
             (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
              db_password_secret_ref, created_at, updated_at) VALUES \
             ('project', 'Project', 'http://platform.test', 'db.test', 3306, 'user', \
              'business', 'workbench', 'secret-ref', '1', '1')",
        )
        .execute(local_store.pool())
        .await
        .expect("project fixture");
        let task_repository = TaskRepository::new(local_store.pool().clone());
        let task_event_bus = TaskEventBus::new(32).expect("task event bus");
        let task_event_pipeline = TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus.clone(),
            SensitiveValueRedactor::default(),
        );
        let job_supervisor = JobSupervisor::default();
        let task_handler_registry = TaskHandlerRegistry::default();
        let task_queue =
            TaskQueue::start(8, 1, task_handler_registry.clone(), job_supervisor.clone())
                .await
                .expect("task queue");
        (
            temp,
            FormalAppState {
                local_store,
                secret_store: Arc::new(MemorySecretStore::default()),
                runtime_registry: ProjectRuntimeRegistry::default(),
                job_supervisor,
                task_recovery_registry:
                    crate::infrastructure::task_handlers::built_in_recovery_registry(),
                task_handler_registry,
                task_queue,
                task_event_bus,
                task_repository,
                task_event_pipeline,
                paths,
            },
        )
    }

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

    #[tokio::test]
    async fn invalid_payload_is_written_to_task_log_before_failure_converges() {
        let (_temp, state) = local_state().await;
        let task_id = "early-handler-failure";
        let log_path = state
            .paths
            .project_task_log_path("project", task_id)
            .expect("log path");
        state
            .task_repository
            .create(CreateTask {
                id: task_id.into(),
                local_project_id: "project".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                name: "整包升级".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: log_path.to_string_lossy().into_owned(),
                targets: vec![("aio".into(), "001122334455".into())],
            })
            .await
            .expect("task");
        let mut current = TaskState::Draft;
        for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
            state
                .task_repository
                .transition(task_id, current, next, None, None)
                .await
                .expect("task transition");
            current = next;
        }
        let error = execute_aio_task(
            &state,
            TaskEnvelope {
                local_task_id: task_id.into(),
                local_project_id: "project".into(),
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                resource_keys: vec!["001122334455".into()],
                priority: 0,
                payload_ref: None,
                payload_sha256: Some("a".repeat(64)),
            },
            tokio_util::sync::CancellationToken::new(),
        )
        .await
        .expect_err("invalid payload");
        assert!(error.to_string().contains("payload_ref"));
        assert_eq!(
            state
                .task_repository
                .get(task_id)
                .await
                .expect("failed task")
                .state,
            TaskState::Failed
        );
        let log = tokio::fs::read_to_string(log_path)
            .await
            .expect("handler failure log");
        assert!(log.contains("TASK_HANDLER_FAILED"));
        assert!(log.contains("payload_ref"));
    }
}

pub fn built_in_recovery_registry() -> crate::infrastructure::task_recovery::TaskRecoveryRegistry {
    let registry = crate::infrastructure::task_recovery::TaskRecoveryRegistry::default();
    registry.register("smart_screen", "register", crate::infrastructure::smart_screen::registration::recover)
        .expect("登记结果处理方法不能重复注册");
    registry.register("smart_screen", "merge", crate::infrastructure::smart_screen::registration::recover)
        .expect("合并结果处理方法不能重复注册");
    for operation in ["version_sync","status"] {
        registry.register("smart_screen", operation, crate::infrastructure::smart_screen::value_updates::recover)
            .expect("业务结果处理方法不能重复注册");
    }
    for operation in crate::domain::smart_screen::operation::WRITE_ACTIONS {
        registry.register("smart_screen", operation, crate::infrastructure::smart_screen::maintenance::recover)
            .expect("设备写操作结果处理方法不能重复注册");
    }
    for operation in crate::domain::smart_screen::operation::READ_ACTIONS {
        registry.register("smart_screen", operation, crate::infrastructure::smart_screen::tasks::recover)
            .expect("屏结果处理方法不能重复注册");
    }
    for operation in AIO_OPERATIONS {
        registry
            .register(
                "aio",
                operation,
                crate::infrastructure::aio_task_recovery::recover,
            )
            .expect("内置结果处理方法不能重复注册");
    }
    registry
}
