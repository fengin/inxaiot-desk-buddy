use crate::application::ports::deployment_workflow::{
    DeploymentPreflightPort, DeploymentSubmissionPort,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::DeploymentPlanInput;
use crate::domain::aio::deployment_workflow::DeploymentTaskSubmission;
use crate::domain::common::task::{TargetState, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::deployment_control::create_deployment_task_with_payload;
use crate::infrastructure::deployment_service::LaunchDeploymentInput;
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::infrastructure::stage75b_preflight_adapter::Stage75BPreflightAdapter;
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
        input: &DeploymentPlanInput,
    ) -> AppResult<DeploymentTaskSubmission> {
        let preflight = Stage75BPreflightAdapter::new(self.state)
            .preflight(project_id, input)
            .await?;
        if !preflight.ready {
            let blockers = preflight
                .checks
                .iter()
                .filter(|check| {
                    check.blocking
                        && check.status
                            == crate::domain::aio::deployment_workflow::PreflightStatus::Failed
                })
                .take(5)
                .map(|check| {
                    check
                        .target_mac
                        .as_deref()
                        .map(|mac| format!("{}({mac})：{}", check.label, check.message))
                        .unwrap_or_else(|| format!("{}：{}", check.label, check.message))
                })
                .collect::<Vec<_>>();
            return Err(AppError::Conflict(format!(
                "部署预检存在阻断：{}",
                blockers.join("；")
            )));
        }
        let snapshot = preflight
            .execution_snapshot
            .ok_or_else(|| AppError::Conflict("部署预检未形成不可变执行快照".into()))?;
        snapshot.validate(project_id)?;
        let plan = crate::domain::aio::deployment::DeploymentPlan::build(snapshot.plan.clone())?;
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
        let input = LaunchDeploymentInput { snapshot };
        let payload = serde_json::to_vec(&input)
            .map_err(|_| AppError::InvalidConfig("序列化AIO任务payload失败".into()))?;
        let payload_sha256 = hex::encode(Sha256::digest(&payload));
        std::fs::write(&temporary_path, payload)
            .map_err(|error| AppError::io("写入AIO任务payload临时文件", &error))?;
        std::fs::rename(&temporary_path, &payload_path)
            .map_err(|error| AppError::io("发布AIO任务payload", &error))?;
        let mut task = create_deployment_task_with_payload(
            self.state,
            project_id,
            &task_id,
            &plan,
            None,
            Some(payload_path.to_string_lossy().into_owned()),
        )
        .await?;
        for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
            task = self
                .state
                .task_repository
                .transition(&task_id, task.state, next, None, None)
                .await?;
        }
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
