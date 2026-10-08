use tauri::State;
use crate::application::project_access::{ProjectAccessRequirement, require_project_access};
use crate::domain::common::project_space::SpaceNode;
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::{project_context::project_pools, stage75_adapter::Stage75Adapter};
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn list_project_spaces(state: State<'_, FormalAppState>, local_project_id: String) -> Result<Vec<SpaceNode>, CommandErrorDto> {
    require_project_access(&Stage75Adapter::new(&state), &local_project_id, ProjectAccessRequirement::ActiveSession)
        .await.map_err(CommandErrorDto::from)?;
    let pools = project_pools(&state, &local_project_id).await.map_err(CommandErrorDto::from)?;
    crate::infrastructure::project_spaces::read(&pools.platform, None).await.map_err(CommandErrorDto::from)
}
