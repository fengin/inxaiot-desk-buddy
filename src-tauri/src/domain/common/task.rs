use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Draft,
    Checking,
    CheckFailed,
    Ready,
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    PartiallySucceeded,
    Failed,
    Interrupted,
    FinalizingFailed,
}

impl TaskState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Checking => "checking",
            Self::CheckFailed => "check_failed",
            Self::Ready => "ready",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Cancelling => "cancelling",
            Self::Cancelled => "cancelled",
            Self::Succeeded => "succeeded",
            Self::PartiallySucceeded => "partially_succeeded",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::FinalizingFailed => "finalizing_failed",
        }
    }

    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "draft" => Ok(Self::Draft),
            "checking" => Ok(Self::Checking),
            "check_failed" => Ok(Self::CheckFailed),
            "ready" => Ok(Self::Ready),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "cancelling" => Ok(Self::Cancelling),
            "cancelled" => Ok(Self::Cancelled),
            "succeeded" => Ok(Self::Succeeded),
            "partially_succeeded" => Ok(Self::PartiallySucceeded),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "finalizing_failed" => Ok(Self::FinalizingFailed),
            _ => Err(AppError::InvalidConfig(format!("未知任务状态：{value}"))),
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Cancelled
                | Self::Succeeded
                | Self::PartiallySucceeded
                | Self::Failed
                | Self::Interrupted
        )
    }

    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Checking)
                | (
                    Self::Checking,
                    Self::CheckFailed
                        | Self::Ready
                        | Self::Succeeded
                        | Self::Failed
                        | Self::Interrupted
                )
                | (Self::CheckFailed, Self::Checking)
                | (Self::Ready, Self::Queued)
                | (Self::Queued, Self::Running | Self::Cancelled)
                | (
                    Self::Running,
                    Self::Cancelling
                        | Self::Succeeded
                        | Self::PartiallySucceeded
                        | Self::Failed
                        | Self::Interrupted
                        | Self::FinalizingFailed
                )
                | (
                    Self::Cancelling,
                    Self::Cancelled | Self::Failed | Self::Interrupted
                )
                | (
                    Self::FinalizingFailed,
                    Self::Succeeded | Self::PartiallySucceeded | Self::Failed | Self::Interrupted
                )
        )
    }

    pub fn ensure_transition(self, next: Self) -> AppResult<()> {
        if self.can_transition_to(next) {
            Ok(())
        } else {
            Err(AppError::Conflict(format!(
                "任务状态不能从 {} 变为 {}",
                self.as_str(),
                next.as_str()
            )))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
    Unknown,
}

impl TargetState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(value: &str) -> AppResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "running" => Ok(Self::Running),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            "interrupted" => Ok(Self::Interrupted),
            "unknown" => Ok(Self::Unknown),
            _ => Err(AppError::InvalidConfig(format!("未知目标状态：{value}"))),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepState {
    Pending,
    Running,
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
    Interrupted,
}

impl StepState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    pub id: String,
    pub local_project_id: String,
    pub remote_operation_record_id: Option<String>,
    pub domain_type: String,
    pub operation_type: String,
    pub name: String,
    pub state: TaskState,
    pub priority: i32,
    pub batch_size: u32,
    pub concurrency: u32,
    pub payload_ref: Option<String>,
    pub sequence: u64,
    pub log_path: String,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskTargetRecord {
    pub local_task_id: String,
    pub resource_type: String,
    pub resource_key: String,
    pub state: TargetState,
    pub stage: String,
    pub progress_current: u64,
    pub progress_total: u64,
    pub fencing_token: Option<u64>,
    pub message_code: Option<String>,
    pub message_params_json: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStepRecord {
    pub id: String,
    pub local_task_id: String,
    pub resource_type: Option<String>,
    pub resource_key: Option<String>,
    pub step_code: String,
    pub state: StepState,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskEventLevel {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub event_id: String,
    pub local_task_id: String,
    pub operation_record_id: Option<String>,
    pub sequence: u64,
    pub local_project_id: String,
    pub domain_type: String,
    pub resource_type: Option<String>,
    pub resource_key: Option<String>,
    pub stage: String,
    pub status: String,
    pub progress_current: Option<u64>,
    pub progress_total: Option<u64>,
    pub level: TaskEventLevel,
    pub message_code: String,
    pub message_params: BTreeMap<String, String>,
    pub message: Option<String>,
    pub timestamp: String,
}
