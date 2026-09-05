use std::collections::BTreeMap;

use crate::application::ports::deployment_workflow::DeploymentSubmissionPort;
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::DeploymentPlan;
use crate::domain::aio::deployment_workflow::{
    DeploymentExecutionSnapshot, DeploymentTaskSubmission,
};
use crate::domain::common::task::{TargetState, TaskEventLevel, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::deployment_control::{mode_code, mode_label};
use crate::infrastructure::deployment_service::LaunchDeploymentInput;
use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TargetUpdate};
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::infrastructure::task_handlers::deployment_payload_file;
use crate::runtime::task_queue::TaskEnvelope;
use sha2::{Digest, Sha256};

pub struct Stage75BSubmissionAdapter<'a> {
    state: &'a FormalAppState,
}

impl<'a> Stage75BSubmissionAdapter<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl DeploymentSubmissionPort for Stage75BSubmissionAdapter<'_> {
    async fn submit(
        &self,
        project_id: &str,
        preflight_task_id: &str,
        execution_snapshot: &DeploymentExecutionSnapshot,
    ) -> AppResult<DeploymentTaskSubmission> {
        uuid::Uuid::parse_str(preflight_task_id)
            .map_err(|_| AppError::InvalidConfig("预检任务ID无效".into()))?;
        record_preflight_submission_event(
            self.state,
            preflight_task_id,
            TaskEventLevel::Info,
            "PREFLIGHT_SUBMISSION_STARTED",
            "submitting",
            "正在根据已通过的检查结果创建部署任务".into(),
            BTreeMap::new(),
        )
        .await;
        let result = self
            .submit_inner(project_id, preflight_task_id, execution_snapshot)
            .await;
        match &result {
            Ok(submission) => {
                record_preflight_submission_event(
                    self.state,
                    preflight_task_id,
                    TaskEventLevel::Info,
                    "PREFLIGHT_SUBMISSION_SUCCEEDED",
                    "succeeded",
                    format!("部署任务创建成功：{}", submission.task_id),
                    BTreeMap::from([("deploymentTaskId".into(), submission.task_id.clone())]),
                )
                .await;
            }
            Err(error) => {
                let reason = self
                    .state
                    .task_event_pipeline
                    .redact_text(&error.to_string());
                record_preflight_submission_event(
                    self.state,
                    preflight_task_id,
                    TaskEventLevel::Error,
                    "PREFLIGHT_SUBMISSION_FAILED",
                    "failed",
                    format!("部署任务提交失败：{reason}"),
                    BTreeMap::new(),
                )
                .await;
            }
        }
        result
    }
}

impl Stage75BSubmissionAdapter<'_> {
    async fn submit_inner(
        &self,
        project_id: &str,
        preflight_task_id: &str,
        execution_snapshot: &DeploymentExecutionSnapshot,
    ) -> AppResult<DeploymentTaskSubmission> {
        execution_snapshot.validate(project_id)?;
        let snapshot_sha256 = execution_snapshot.integrity_sha256()?;
        let plan = DeploymentPlan::build(execution_snapshot.plan.clone())?;
        let task_id = uuid::Uuid::now_v7().to_string();
        let task_dir = self
            .state
            .paths
            .project_task_dir(project_id, &task_id)
            .map_err(crate::infrastructure::project_context::map_formal_error)?;
        std::fs::create_dir_all(&task_dir)
            .map_err(|error| AppError::io("创建排队任务目录", &error))?;
        let payload_path = task_dir.join(deployment_payload_file());
        let temporary_path = task_dir.join("deployment-input.json.tmp");
        let input = LaunchDeploymentInput {
            snapshot: execution_snapshot.clone(),
        };
        let payload = serde_json::to_vec(&input)
            .map_err(|_| AppError::InvalidConfig("序列化AIO任务payload失败".into()))?;
        let payload_sha256 = hex::encode(Sha256::digest(&payload));
        std::fs::write(&temporary_path, payload)
            .map_err(|error| AppError::io("写入AIO任务payload临时文件", &error))?;
        std::fs::rename(&temporary_path, &payload_path)
            .map_err(|error| AppError::io("发布AIO任务payload", &error))?;
        let task = match self
            .state
            .task_repository
            .create_queued_from_preflight(
                preflight_task_id,
                &snapshot_sha256,
                CreateTask {
                    id: task_id.clone(),
                    local_project_id: project_id.into(),
                    remote_operation_record_id: None,
                    domain_type: "aio".into(),
                    operation_type: mode_code(plan.mode).into(),
                    name: format!("{} · {}", mode_label(plan.mode), plan.artifact_name),
                    priority: 0,
                    batch_size: plan.batch_size,
                    concurrency: plan.concurrency,
                    payload_ref: Some(payload_path.to_string_lossy().into_owned()),
                    log_path: self
                        .state
                        .paths
                        .project_task_log_path(project_id, &task_id)
                        .map_err(crate::infrastructure::project_context::map_formal_error)?
                        .to_string_lossy()
                        .into_owned(),
                    targets: plan
                        .target_macs
                        .iter()
                        .map(|mac| ("aio".into(), mac.clone()))
                        .collect(),
                },
            )
            .await
        {
            Ok(task) => task,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&task_dir);
                return Err(error);
            }
        };
        let envelope = TaskEnvelope {
            local_task_id: task_id.clone(),
            local_project_id: project_id.into(),
            domain_type: "aio".into(),
            operation_type: task.operation_type.clone(),
            resource_keys: plan.target_macs,
            priority: task.priority,
            payload_ref: task.payload_ref.clone(),
            payload_sha256: Some(payload_sha256),
        };
        if let Err(error) = self.state.task_queue.enqueue(envelope).await {
            mark_enqueue_failed(self.state, &task_id, &error.to_string()).await;
            let _ = TaskDataLifecycle::new(&self.state.paths).finalize_task(
                project_id,
                &task_id,
                TaskState::Cancelled,
            );
            return Err(error);
        }
        Ok(DeploymentTaskSubmission {
            task_id,
            state: task.state.as_str().into(),
            submitted_at: task.created_at,
        })
    }
}

async fn record_preflight_submission_event(
    state: &FormalAppState,
    preflight_task_id: &str,
    level: TaskEventLevel,
    message_code: &str,
    status: &str,
    message: String,
    message_params: BTreeMap<String, String>,
) {
    if let Err(error) = state
        .task_event_pipeline
        .emit(
            preflight_task_id,
            TaskEventInput {
                resource_type: None,
                resource_key: None,
                stage: "提交部署任务".into(),
                status: status.into(),
                progress_current: None,
                progress_total: None,
                level,
                message_code: message_code.into(),
                message_params,
                message: Some(message),
            },
        )
        .await
    {
        tracing::warn!(
            preflight_task_id,
            message_code,
            error = ?crate::core::log_safety::safe_error(&error),
            "persist deployment submission event to preflight log failed"
        );
    }
}

async fn mark_enqueue_failed(state: &FormalAppState, task_id: &str, message: &str) {
    let _ = state
        .task_repository
        .transition(
            task_id,
            TaskState::Queued,
            TaskState::Cancelled,
            Some("QUEUE_REJECTED"),
            Some(message),
        )
        .await;
    if let Ok(targets) = state.task_repository.targets(task_id).await {
        for target in targets {
            if target.state == TargetState::Pending {
                let _ = state
                    .task_repository
                    .update_target(
                        task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: TargetState::Cancelled,
                            stage: "queue_rejected".into(),
                            progress_current: 100,
                            progress_total: 100,
                            fencing_token: None,
                            message_code: Some("QUEUE_REJECTED".into()),
                            message_params_json: None,
                        },
                    )
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::Value;

    use super::*;
    use crate::domain::aio::deployment::{
        DeploymentImageInput, DeploymentMode, DeploymentPlanInput,
    };
    use crate::domain::aio::deployment_workflow::{
        DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION, DeploymentTargetSnapshot,
    };
    use crate::domain::aio::inventory::WorkbenchNodeSnapshot;
    use crate::formal::config::AppPaths;
    use crate::formal::job_supervisor::JobSupervisor;
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

    async fn state() -> (tempfile::TempDir, FormalAppState) {
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
                task_handler_registry,
                task_queue,
                task_event_bus,
                task_repository,
                task_event_pipeline,
                paths,
            },
        )
    }

    fn snapshot() -> DeploymentExecutionSnapshot {
        let node = WorkbenchNodeSnapshot {
            mac_normalized: "001122334455".into(),
            name: "Test AIO".into(),
            ip: "192.0.2.10".into(),
            building_id: None,
            region_id: None,
            addr_alias: None,
            floor: None,
            location: None,
            remark: None,
            platform_aio_id: None,
            management_state: "pending".into(),
            source: "test".into(),
            last_operation_id: None,
            version: 1,
        };
        DeploymentExecutionSnapshot {
            schema_version: DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION,
            local_project_id: "project".into(),
            checked_at: "2026-09-05T00:00:00Z".into(),
            profile_version: 1,
            artifact_fingerprint: "a".repeat(64),
            plan: DeploymentPlanInput {
                mode: DeploymentMode::FullUpgrade,
                target_macs: vec![node.mac_normalized.clone()],
                image_files: vec![DeploymentImageInput {
                    service_name: "device-edge".into(),
                    file_path: "C:/fixture/device-edge.tar".into(),
                    image_tag: "device-edge:1.0.0".into(),
                }],
                artifact_path: "C:/fixture/device-edge.tar".into(),
                artifact_name: "镜像组合".into(),
                artifact_version: "generated".into(),
                service_name: None,
                image_name: None,
                service_image_environment_variable: None,
                images: BTreeMap::from([("device-edge".into(), "device-edge:1.0.0".into())]),
                batch_size: 1,
                concurrency: 1,
            },
            targets: vec![DeploymentTargetSnapshot {
                ssh_host: node.ip.clone(),
                ssh_port: 22,
                host_key_algorithm: "ssh-ed25519".into(),
                host_key_fingerprint: "SHA256:test".into(),
                host_key_accepted_at: "2026-09-05T00:00:00Z".into(),
                node,
            }],
        }
    }

    async fn succeeded_preflight(
        state: &FormalAppState,
        task_id: &str,
        snapshot: &DeploymentExecutionSnapshot,
    ) -> std::path::PathBuf {
        let log_path = state
            .paths
            .project_task_log_path("project", task_id)
            .expect("preflight log path");
        state
            .task_repository
            .create(CreateTask {
                id: task_id.into(),
                local_project_id: "project".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "deployment_preflight".into(),
                name: "部署检查".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: log_path.to_string_lossy().into_owned(),
                targets: vec![
                    ("preflight_internal".into(), "common".into()),
                    ("aio".into(), "001122334455".into()),
                ],
            })
            .await
            .expect("create preflight task");
        state
            .task_repository
            .transition(task_id, TaskState::Draft, TaskState::Checking, None, None)
            .await
            .expect("start preflight task");
        for (resource_type, resource_key, total) in [
            ("preflight_internal", "common", 3),
            ("aio", "001122334455", 2),
        ] {
            state
                .task_repository
                .update_target(
                    task_id,
                    TargetUpdate {
                        resource_type: resource_type.into(),
                        resource_key: resource_key.into(),
                        state: TargetState::Succeeded,
                        stage: "检查完成".into(),
                        progress_current: total,
                        progress_total: total,
                        fencing_token: None,
                        message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                        message_params_json: None,
                    },
                )
                .await
                .expect("finish preflight target");
        }
        state
            .task_repository
            .bind_preflight_snapshot(
                task_id,
                "project",
                &snapshot.integrity_sha256().expect("snapshot hash"),
            )
            .await
            .expect("bind preflight snapshot");
        state
            .task_repository
            .transition(
                task_id,
                TaskState::Checking,
                TaskState::Succeeded,
                None,
                None,
            )
            .await
            .expect("finish preflight task");
        log_path
    }

    fn message_codes(log_path: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(log_path)
            .expect("submission log")
            .lines()
            .map(|line| {
                serde_json::from_str::<Value>(line).expect("task event json")["messageCode"]
                    .as_str()
                    .expect("message code")
                    .to_string()
            })
            .collect()
    }

    #[tokio::test]
    async fn submission_success_is_recorded_in_the_consumed_preflight_log() {
        let (_temp, state) = state().await;
        let snapshot = snapshot();
        let preflight_task_id = uuid::Uuid::now_v7().to_string();
        let log_path = succeeded_preflight(&state, &preflight_task_id, &snapshot).await;

        let submission = Stage75BSubmissionAdapter::new(&state)
            .submit("project", &preflight_task_id, &snapshot)
            .await
            .expect("submit deployment");
        assert_eq!(
            message_codes(&log_path),
            vec![
                "PREFLIGHT_SUBMISSION_STARTED",
                "PREFLIGHT_SUBMISSION_SUCCEEDED"
            ]
        );
        assert_eq!(
            state
                .task_repository
                .get(&submission.task_id)
                .await
                .expect("queued deployment")
                .state,
            TaskState::Queued
        );

        state
            .task_queue
            .shutdown(std::time::Duration::from_secs(1))
            .await;
        state.local_store.close().await;
    }

    #[tokio::test]
    async fn submission_failure_is_recorded_without_replacing_the_original_error() {
        let (_temp, state) = state().await;
        let snapshot = snapshot();
        let preflight_task_id = uuid::Uuid::now_v7().to_string();
        let log_path = succeeded_preflight(&state, &preflight_task_id, &snapshot).await;
        let mut invalid_snapshot = snapshot.clone();
        invalid_snapshot.schema_version = 0;

        let error = Stage75BSubmissionAdapter::new(&state)
            .submit("project", &preflight_task_id, &invalid_snapshot)
            .await
            .expect_err("invalid snapshot must fail");
        assert!(matches!(
            error,
            AppError::InvalidConfig(message) if message.contains("执行快照无效")
        ));
        assert_eq!(
            message_codes(&log_path),
            vec![
                "PREFLIGHT_SUBMISSION_STARTED",
                "PREFLIGHT_SUBMISSION_FAILED"
            ]
        );
        assert_eq!(
            state
                .task_repository
                .get(&preflight_task_id)
                .await
                .expect("preflight remains")
                .state,
            TaskState::Succeeded
        );

        state
            .task_queue
            .shutdown(std::time::Duration::from_secs(1))
            .await;
        state.local_store.close().await;
    }
}
