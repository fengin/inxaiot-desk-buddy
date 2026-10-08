use crate::application::project_access::{ProjectAccessRequirement, require_project_access};
use crate::domain::common::operation_history::{
    OperationHistoryDetail, OperationHistoryPage, OperationHistoryQuery,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::stage75_adapter::Stage75Adapter;
use crate::interface::error::CommandErrorDto;
use tauri::State;

#[tauri::command]
pub async fn list_business_operation_history(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    domain_type: String,
    query: OperationHistoryQuery,
) -> Result<OperationHistoryPage, CommandErrorDto> {
    if domain_type=="smart_screen" {return crate::infrastructure::smart_screen::history::list(&state,&local_project_id,&query).await.map_err(Into::into);}
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::Ready,
    )
    .await?;
    crate::infrastructure::operation_history::list_history(
        &state,
        &domain_type,
        &local_project_id,
        &query,
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn get_business_operation_history_detail(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    domain_type: String,
    operation_id: String,
) -> Result<OperationHistoryDetail, CommandErrorDto> {
    if domain_type=="smart_screen" {return crate::infrastructure::smart_screen::history::detail(&state,&local_project_id,&operation_id).await.map_err(Into::into);}
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::Ready,
    )
    .await?;
    crate::infrastructure::operation_history::history_detail(
        &state,
        &domain_type,
        &local_project_id,
        &operation_id,
    )
    .await
    .map_err(Into::into)
}
