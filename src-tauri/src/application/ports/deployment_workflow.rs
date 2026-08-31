use crate::core::error::AppResult;
use crate::domain::aio::deployment::DeploymentPlanInput;
use crate::domain::aio::deployment_workflow::{
    DeploymentPreflightReport, DeploymentTaskSubmission, DeploymentTaskView,
    OperationHistoryDetail, OperationHistoryPage, OperationHistoryQuery,
};

#[allow(async_fn_in_trait)]
pub trait DeploymentPreflightPort: Send + Sync {
    async fn preflight(
        &self,
        project_id: &str,
        input: &DeploymentPlanInput,
    ) -> AppResult<DeploymentPreflightReport>;
}

#[allow(async_fn_in_trait)]
pub trait DeploymentSubmissionPort: Send + Sync {
    async fn submit(
        &self,
        project_id: &str,
        input: &DeploymentPlanInput,
    ) -> AppResult<DeploymentTaskSubmission>;
}

#[allow(async_fn_in_trait)]
pub trait DeploymentTaskQueryPort: Send + Sync {
    async fn task(&self, project_id: &str, task_id: &str) -> AppResult<DeploymentTaskView>;
}

#[allow(async_fn_in_trait)]
pub trait OperationHistoryQueryPort: Send + Sync {
    async fn list_history(
        &self,
        project_id: &str,
        query: &OperationHistoryQuery,
    ) -> AppResult<OperationHistoryPage>;

    async fn history_detail(
        &self,
        project_id: &str,
        operation_id: &str,
    ) -> AppResult<OperationHistoryDetail>;
}
