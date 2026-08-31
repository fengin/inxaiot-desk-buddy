use tauri::State;

use crate::formal::app_state::FormalAppState;
use crate::formal::workbench_store::WorkbenchSchemaStatus;
use crate::infrastructure::project_context::{
    initialize_or_upgrade_workbench_schema as upgrade_schema, workbench_schema_status,
};
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn get_workbench_schema_status(
    state: State<'_, FormalAppState>,
    local_project_id: String,
) -> Result<WorkbenchSchemaStatus, CommandErrorDto> {
    workbench_schema_status(&state, &local_project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn initialize_or_upgrade_workbench_schema(
    state: State<'_, FormalAppState>,
    local_project_id: String,
) -> Result<WorkbenchSchemaStatus, CommandErrorDto> {
    upgrade_schema(&state, &local_project_id)
        .await
        .map_err(CommandErrorDto::from)
}
