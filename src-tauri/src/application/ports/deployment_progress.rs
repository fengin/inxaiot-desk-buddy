use crate::core::error::AppResult;
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel};

#[derive(Clone, Debug)]
pub struct DeploymentProgressEvent {
    pub mac: String,
    pub stage: String,
    pub step_code: Option<String>,
    pub step_state: Option<StepState>,
    pub target_state: TargetState,
    pub progress_current: u64,
    pub progress_total: u64,
    pub level: TaskEventLevel,
    pub message_code: String,
    pub message: Option<String>,
}

pub trait DeploymentProgressSink: Send + Sync {
    fn emit(&self, event: DeploymentProgressEvent) -> AppResult<()>;
}

#[derive(Default)]
pub struct NoopDeploymentProgressSink;

impl DeploymentProgressSink for NoopDeploymentProgressSink {
    fn emit(&self, _event: DeploymentProgressEvent) -> AppResult<()> {
        Ok(())
    }
}
