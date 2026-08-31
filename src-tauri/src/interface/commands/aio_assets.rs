use std::path::Path;

use tauri::State;

use crate::application::aio_assets::{
    AioNodeDetail, AioNodeListPage, InventoryApplyOutcome, InventoryPreview, ListAioNodesQuery,
    apply_inventory_import as apply_import, discard_inventory_import as discard_import,
    get_aio_node_detail as get_detail, latest_inventory_import as latest_import, list_aio_nodes,
    preview_inventory_import as preview_import, update_inventory_selection as update_selection,
};
use crate::application::project_access::{ProjectAccessRequirement, require_project_access};
use crate::domain::aio::assets::{AioImportSession, ImportSelection};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::aio_assets_service::AioAssetsService;
use crate::infrastructure::stage75_adapter::Stage75Adapter;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn list_edge_nodes(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    query: ListAioNodesQuery,
) -> Result<AioNodeListPage, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    list_aio_nodes(&AioAssetsService::new(&state), &local_project_id, query)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_edge_node_detail(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    mac: String,
) -> Result<AioNodeDetail, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    get_detail(&AioAssetsService::new(&state), &local_project_id, &mac)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn preview_inventory_import(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    file_path: String,
) -> Result<InventoryPreview, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    preview_import(
        &AioAssetsService::new(&state),
        &local_project_id,
        Path::new(&file_path),
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_latest_inventory_import(
    state: State<'_, FormalAppState>,
    local_project_id: String,
) -> Result<Option<AioImportSession>, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    latest_import(&AioAssetsService::new(&state), &local_project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn update_inventory_import_selection(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    session_id: String,
    selections: Vec<ImportSelection>,
) -> Result<AioImportSession, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    update_selection(
        &AioAssetsService::new(&state),
        &local_project_id,
        &session_id,
        &selections,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn apply_inventory_import(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    session_id: String,
) -> Result<InventoryApplyOutcome, CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::Ready,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    apply_import(
        &AioAssetsService::new(&state),
        &local_project_id,
        &session_id,
    )
    .await
    .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn discard_inventory_import(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    session_id: String,
) -> Result<(), CommandErrorDto> {
    require_project_access(
        &Stage75Adapter::new(&state),
        &local_project_id,
        ProjectAccessRequirement::ActiveSession,
    )
    .await
    .map_err(CommandErrorDto::from)?;
    discard_import(
        &AioAssetsService::new(&state),
        &local_project_id,
        &session_id,
    )
    .await
    .map_err(CommandErrorDto::from)
}
