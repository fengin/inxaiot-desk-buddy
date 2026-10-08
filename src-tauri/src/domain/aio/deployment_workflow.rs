use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::DeploymentPlanInput;
use crate::domain::aio::inventory::WorkbenchNodeSnapshot;

pub const DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTargetSnapshot {
    pub node: WorkbenchNodeSnapshot,
    pub ssh_host: String,
    pub ssh_port: u16,
    pub host_key_algorithm: String,
    pub host_key_fingerprint: String,
    pub host_key_accepted_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentExecutionSnapshot {
    pub schema_version: u32,
    pub local_project_id: String,
    pub checked_at: String,
    pub profile_version: u64,
    pub artifact_fingerprint: String,
    pub plan: DeploymentPlanInput,
    pub targets: Vec<DeploymentTargetSnapshot>,
}

impl DeploymentExecutionSnapshot {
    pub fn validate(&self, local_project_id: &str) -> AppResult<()> {
        if self.schema_version != DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION
            || self.local_project_id != local_project_id
            || self.checked_at.trim().is_empty()
            || self.profile_version == 0
            || self.artifact_fingerprint.len() != 64
            || !self
                .artifact_fingerprint
                .bytes()
                .all(|value| value.is_ascii_hexdigit())
            || self.targets.len() != self.plan.target_macs.len()
        {
            return Err(AppError::InvalidConfig(
                "部署任务执行快照无效或不属于当前项目".into(),
            ));
        }
        let mut plan_targets = self.plan.target_macs.clone();
        plan_targets.sort();
        plan_targets.dedup();
        let mut snapshot_targets = self
            .targets
            .iter()
            .map(|target| target.node.mac_normalized.clone())
            .collect::<Vec<_>>();
        snapshot_targets.sort();
        snapshot_targets.dedup();
        if plan_targets != snapshot_targets
            || self.targets.iter().any(|target| {
                target.ssh_host.trim().is_empty()
                    || target.ssh_port == 0
                    || target.host_key_algorithm.trim().is_empty()
                    || target.host_key_fingerprint.trim().is_empty()
                    || target.host_key_accepted_at.trim().is_empty()
                    || target.node.ip != target.ssh_host
            })
        {
            return Err(AppError::InvalidConfig(
                "部署任务目标快照与执行计划不一致".into(),
            ));
        }
        Ok(())
    }

    pub fn integrity_sha256(&self) -> AppResult<String> {
        let payload = serde_json::to_vec(self)
            .map_err(|_| AppError::InvalidConfig("序列化部署检查快照失败".into()))?;
        Ok(hex::encode(Sha256::digest(payload)))
    }
}

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
    pub execution_snapshot: Option<DeploymentExecutionSnapshot>,
    pub checked_at: String,
}

impl DeploymentPreflightReport {
    pub fn from_checks(
        checks: Vec<DeploymentPreflightCheck>,
        normalized_plan: DeploymentPlanInput,
        profile_version: Option<u64>,
        execution_snapshot: Option<DeploymentExecutionSnapshot>,
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
            execution_snapshot,
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

pub use crate::domain::common::operation_history::{
    OperationHistoryDetail, OperationHistoryItem, OperationHistoryPage, OperationHistoryQuery,
    OperationHistoryTarget,
};
