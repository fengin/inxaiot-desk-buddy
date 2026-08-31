use tauri::State;

use crate::application::deployment_workflow::{
    get_deployment_task as query_task, get_operation_history_detail as query_history_detail,
    list_operation_history as query_history, preflight_deployment as run_preflight,
    submit_deployment as submit_task,
};
use crate::application::project_access::{ProjectAccessRequirement, require_project_access};
use crate::domain::aio::deployment::DeploymentPlanInput;
use crate::domain::aio::deployment_workflow::{
    DeploymentPreflightReport, DeploymentTaskSubmission, DeploymentTaskView,
    OperationHistoryDetail, OperationHistoryPage, OperationHistoryQuery,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::stage75_adapter::Stage75Adapter;
use crate::infrastructure::stage75b_preflight_adapter::Stage75BPreflightAdapter;
use crate::infrastructure::stage75b_query_adapter::Stage75BQueryAdapter;
use crate::infrastructure::stage75b_submission_adapter::Stage75BSubmissionAdapter;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn preflight_deployment(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    input: DeploymentPlanInput,
) -> Result<DeploymentPreflightReport, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::Ready,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    run_preflight(
        &Stage75BPreflightAdapter::new(&state),
        &local_project_id,
        &input,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn submit_deployment(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    input: DeploymentPlanInput,
) -> Result<DeploymentTaskSubmission, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::Ready,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    submit_task(
        &Stage75BSubmissionAdapter::new(&state),
        &local_project_id,
        &input,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_deployment_task(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    task_id: String,
) -> Result<DeploymentTaskView, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    query_task(
        &Stage75BQueryAdapter::new(&state),
        &local_project_id,
        &task_id,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn list_operation_history(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    query: OperationHistoryQuery,
) -> Result<OperationHistoryPage, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    query_history(
        &Stage75BQueryAdapter::new(&state),
        &local_project_id,
        &query,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_operation_history_detail(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    operation_id: String,
) -> Result<OperationHistoryDetail, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    query_history_detail(
        &Stage75BQueryAdapter::new(&state),
        &local_project_id,
        &operation_id,
    )
    .await
    .map_err(CommandErrorDto::from)
}
