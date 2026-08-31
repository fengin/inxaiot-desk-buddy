use tauri::State;

use crate::application::data_directory::{
    DataDirectoryStatus, DataDirectorySwitchRequest, get_data_directory_status as get_status,
    schedule_data_directory_rollback as schedule_rollback,
    schedule_data_directory_switch as schedule_switch,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::data_directory::DataDirectoryManager;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub fn get_data_directory_status(
    manager: State<'_, DataDirectoryManager>,
) -> Result<DataDirectoryStatus, CommandErrorDto> {
    get_status(&*manager).map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn schedule_data_directory_switch(
    state: State<'_, FormalAppState>,
    manager: State<'_, DataDirectoryManager>,
    request: DataDirectorySwitchRequest,
) -> Result<DataDirectoryStatus, CommandErrorDto> {
    let active = state
        .task_repository
        .active_counts()
        .await
        .map_err(CommandErrorDto::from)?;
    schedule_switch(&*manager, active.total, request).map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn schedule_data_directory_rollback(
    state: State<'_, FormalAppState>,
    manager: State<'_, DataDirectoryManager>,
) -> Result<DataDirectoryStatus, CommandErrorDto> {
    let active = state
        .task_repository
        .active_counts()
        .await
        .map_err(CommandErrorDto::from)?;
    schedule_rollback(&*manager, active.total).map_err(CommandErrorDto::from)
}
