use tauri::State;

use crate::application::ports::service_inspection::{
    ServiceInspectionPort, ServiceInspectionSubmission,
};
use crate::application::project_access::{ProjectAccessRequirement, require_project_access};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::service_inspection::ServiceInspectionService;
use crate::infrastructure::stage75_adapter::Stage75Adapter;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn check_edge_node_services(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    mac: String,
) -> Result<ServiceInspectionSubmission, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    ServiceInspectionService::new(&state)
        .submit(&local_project_id, &mac)
        .await
        .map_err(CommandErrorDto::from)
}
