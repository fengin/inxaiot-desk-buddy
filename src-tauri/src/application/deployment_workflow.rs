use crate::application::ports::deployment_workflow::{
    DeploymentPreflightPort, DeploymentSubmissionPort, DeploymentTaskQueryPort,
    OperationHistoryQueryPort,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::DeploymentPlanInput;
use crate::domain::aio::deployment_workflow::{
    DeploymentExecutionSnapshot, DeploymentPreflightReport, DeploymentTaskSubmission,
    DeploymentTaskView, OperationHistoryDetail, OperationHistoryPage, OperationHistoryQuery,
};

pub async fn preflight_deployment(
    port: &impl DeploymentPreflightPort,
    project_id: &str,
    input: &DeploymentPlanInput,
) -> AppResult<DeploymentPreflightReport> {
    validate_id("项目ID", project_id)?;
    port.preflight(project_id, input).await
}

pub async fn submit_deployment(
    port: &impl DeploymentSubmissionPort,
    project_id: &str,
    preflight_task_id: &str,
    execution_snapshot: &DeploymentExecutionSnapshot,
) -> AppResult<DeploymentTaskSubmission> {
    validate_id("项目ID", project_id)?;
    validate_id("预检任务ID", preflight_task_id)?;
    port.submit(project_id, preflight_task_id, execution_snapshot)
        .await
}

pub async fn get_deployment_task(
    port: &impl DeploymentTaskQueryPort,
    project_id: &str,
    task_id: &str,
) -> AppResult<DeploymentTaskView> {
    validate_id("项目ID", project_id)?;
    validate_id("任务ID", task_id)?;
    port.task(project_id, task_id).await
}

pub async fn list_operation_history(
    port: &impl OperationHistoryQueryPort,
    project_id: &str,
    query: &OperationHistoryQuery,
) -> AppResult<OperationHistoryPage> {
    validate_id("项目ID", project_id)?;
    validate_history_query(query)?;
    port.list_history(project_id, query).await
}

pub async fn get_operation_history_detail(
    port: &impl OperationHistoryQueryPort,
    project_id: &str,
    operation_id: &str,
) -> AppResult<OperationHistoryDetail> {
    validate_id("项目ID", project_id)?;
    validate_id("操作ID", operation_id)?;
    port.history_detail(project_id, operation_id).await
}

fn validate_id(label: &str, value: &str) -> AppResult<()> {
    if value.trim().is_empty() {
        Err(AppError::InvalidConfig(format!("{label}不能为空")))
    } else {
        Ok(())
    }
}

fn validate_history_query(query: &OperationHistoryQuery) -> AppResult<()> {
    if query.page == 0 {
        return Err(AppError::InvalidConfig("历史页码必须从1开始".into()));
    }
    if !(1..=100).contains(&query.page_size) {
        return Err(AppError::InvalidConfig(
            "历史每页数量必须在1到100之间".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unbounded_history_page_size() {
        let error = validate_history_query(&OperationHistoryQuery {
            page: 1,
            page_size: 101,
            operation_type: None,
            state: None,
        })
        .expect_err("page size must be bounded");
        assert!(error.to_string().contains("1到100"));
    }
}
