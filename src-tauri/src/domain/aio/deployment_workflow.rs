use serde::{Deserialize, Serialize};

use crate::domain::aio::deployment::DeploymentPlanInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    Passed,
    Warning,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreflightRemediation {
    pub action: String,
    pub label: String,
    pub route: Option<String>,
    pub target: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPreflightCheck {
    pub code: String,
    pub label: String,
    pub status: PreflightStatus,
    pub blocking: bool,
    pub target_mac: Option<String>,
    pub message: String,
    pub remediation: Option<PreflightRemediation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentPreflightReport {
    pub ready: bool,
    pub checks: Vec<DeploymentPreflightCheck>,
    pub normalized_plan: DeploymentPlanInput,
    pub profile_version: Option<u64>,
    pub checked_at: String,
}

impl DeploymentPreflightReport {
    pub fn from_checks(
        checks: Vec<DeploymentPreflightCheck>,
        normalized_plan: DeploymentPlanInput,
        profile_version: Option<u64>,
        checked_at: String,
    ) -> Self {
        let ready = !checks
            .iter()
            .any(|check| check.blocking && check.status == PreflightStatus::Failed);
        Self {
            ready,
            checks,
            normalized_plan,
            profile_version,
            checked_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTaskSubmission {
    pub task_id: String,
    pub state: String,
    pub submitted_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTaskTargetView {
    pub mac: String,
    pub state: String,
    pub stage: String,
    pub progress: u32,
    pub message_code: Option<String>,
    pub message: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTaskStepView {
    pub mac: Option<String>,
    pub code: String,
    pub state: String,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTaskView {
    pub id: String,
    pub project_id: String,
    pub operation_id: Option<String>,
    pub operation_type: String,
    pub name: String,
    pub state: String,
    pub stage: String,
    pub progress: u32,
    pub target_count: u32,
    pub completed_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
    pub cancellable: bool,
    pub error_code: Option<String>,
    pub message: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub updated_at: String,
    pub targets: Vec<DeploymentTaskTargetView>,
    pub steps: Vec<DeploymentTaskStepView>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryQuery {
    pub page: u32,
    pub page_size: u32,
    pub operation_type: Option<String>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryItem {
    pub id: String,
    pub domain_type: String,
    pub operation_type: String,
    pub operation_name: String,
    pub operator_name: String,
    pub instance_id: String,
    pub state: String,
    pub target_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
    pub artifact_name: Option<String>,
    pub artifact_version: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryTarget {
    pub resource_type: String,
    pub resource_key: String,
    pub state: String,
    pub before_version: Option<String>,
    pub after_version: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryPage {
    pub items: Vec<OperationHistoryItem>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryDetail {
    pub operation: OperationHistoryItem,
    pub targets: Vec<OperationHistoryTarget>,
}
