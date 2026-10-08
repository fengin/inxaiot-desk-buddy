use crate::application::{ports::smart_screen::ScreenAssetsPort, smart_screen::assets};
use crate::domain::smart_screen::model::{ScreenFields, ScreenSnapshot};
use crate::domain::smart_screen::operation::{ScreenOperationInput, ScreenPreflight};
use crate::domain::smart_screen::registration::{RegistrationPreview, RegistrationSubmission};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::smart_screen::apk::{self, ApkInfo};
use crate::infrastructure::smart_screen::assets_service::ScreenAssetsService;
use crate::infrastructure::smart_screen::merge::{self, MergeDecision, MergeResult};
use crate::infrastructure::smart_screen::registration;
use crate::infrastructure::smart_screen::tasks;
use crate::infrastructure::smart_screen::value_updates::{
    self, StatusChange, StatusResult, VersionPreview,
};
use crate::interface::error::CommandErrorDto;
use tauri::State;

#[tauri::command]
pub async fn screen_app_config_draft_load(state: State<'_, FormalAppState>, local_project_id: String) -> Result<Option<serde_json::Value>,CommandErrorDto> {
    crate::infrastructure::smart_screen::app_config::load_draft(&state,&local_project_id).await.map_err(Into::into)
}
#[tauri::command]
pub async fn screen_app_config_draft_save(state: State<'_, FormalAppState>, local_project_id: String, draft: Option<serde_json::Value>) -> Result<(),CommandErrorDto> {
    crate::infrastructure::smart_screen::app_config::save_draft(&state,&local_project_id,draft).await.map_err(Into::into)
}

#[tauri::command]
pub async fn screen_app_config_read(state: State<'_, FormalAppState>, local_project_id: String, screen_ids: Vec<String>) -> Result<Vec<crate::domain::smart_screen::app_config::AppConfigRead>,CommandErrorDto> {
    crate::infrastructure::smart_screen::app_config::read(&state,&local_project_id,&screen_ids).await.map_err(Into::into)
}
#[tauri::command]
pub async fn screen_app_config_preflight(state: State<'_, FormalAppState>, local_project_id: String, input: ScreenOperationInput, patches: std::collections::BTreeMap<String,crate::domain::smart_screen::app_config::AppConfigPatch>) -> Result<ScreenPreflight,CommandErrorDto> {
    crate::infrastructure::smart_screen::maintenance::preflight_with_config(&state,&local_project_id,input,Some(patches)).await.map_err(Into::into)
}

#[tauri::command]
pub async fn screen_takeover_release(state: State<'_, FormalAppState>, local_project_id: String, expected: Vec<crate::infrastructure::smart_screen::takeover::TakeoverConflict>, confirmed: bool) -> Result<(), CommandErrorDto> {
    crate::infrastructure::smart_screen::takeover::release(&state, &local_project_id, &expected, confirmed).await.map_err(Into::into)
}

#[tauri::command]
pub async fn screen_lock_preview(state: State<'_, FormalAppState>, local_project_id: String, operation_id: String) -> Result<Vec<crate::infrastructure::smart_screen::lock_release::ScreenLock>, CommandErrorDto> {
    crate::infrastructure::smart_screen::lock_release::preview(&state, &local_project_id, &operation_id).await.map_err(Into::into)
}
#[tauri::command]
pub async fn screen_lock_release(state: State<'_, FormalAppState>, local_project_id: String, operation_id: String, expected: Vec<crate::infrastructure::smart_screen::lock_release::ScreenLock>, confirmed: bool) -> Result<(), CommandErrorDto> {
    crate::infrastructure::smart_screen::lock_release::release(&state, &local_project_id, &operation_id, &expected, confirmed).await.map_err(Into::into)
}

#[tauri::command]
pub async fn screen_load(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    refresh: bool,
) -> Result<ScreenSnapshot, CommandErrorDto> {
    assets::load(
        &ScreenAssetsService::new(&state),
        &local_project_id,
        refresh,
    )
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_select_project(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    business_project_id: String,
) -> Result<(), CommandErrorDto> {
    ScreenAssetsService::new(&state)
        .select_business_project(&local_project_id, &business_project_id)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_save_local(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    fields: ScreenFields,
    id: Option<String>,
    expected_revision: Option<u64>,
) -> Result<String, CommandErrorDto> {
    assets::save(
        &ScreenAssetsService::new(&state),
        &local_project_id,
        &fields,
        id.as_deref(),
        expected_revision,
    )
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_import_local(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    fields: Vec<ScreenFields>,
) -> Result<Vec<String>, CommandErrorDto> {
    assets::import(
        &ScreenAssetsService::new(&state),
        &local_project_id,
        &fields,
    )
    .await
    .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_remove_local(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    id: String,
) -> Result<(), CommandErrorDto> {
    ScreenAssetsService::new(&state)
        .remove_local(&local_project_id, &id)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_save_draft(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    id: String,
    fields: ScreenFields,
    expected_revision: u64,
    expected_asset_revision: u64,
) -> Result<(), CommandErrorDto> {
    ScreenAssetsService::new(&state)
        .save_draft(
            &local_project_id,
            &id,
            &fields,
            expected_revision,
            expected_asset_revision,
        )
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_discard_draft(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    id: String,
    expected_revision: u64,
) -> Result<(), CommandErrorDto> {
    ScreenAssetsService::new(&state)
        .discard_draft(&local_project_id, &id, expected_revision)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_preflight(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    input: ScreenOperationInput,
) -> Result<ScreenPreflight, CommandErrorDto> {
    tasks::preflight(&state, &local_project_id, input)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_execute(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    preflight_id: String,
    input: ScreenOperationInput,
) -> Result<String, CommandErrorDto> {
    tasks::submit(&state, &local_project_id, &preflight_id, input)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_cancel(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    task_id: String,
) -> Result<(), CommandErrorDto> {
    let task = state
        .task_repository
        .get(&task_id)
        .await
        .map_err(CommandErrorDto::from)?;
    if task.local_project_id != local_project_id || task.domain_type != tasks::DOMAIN {
        return Err(
            crate::core::error::AppError::Conflict("任务不属于当前智能屏项目".into()).into(),
        );
    }
    crate::interface::commands::task_activity::request_task_cancel(&state, &task_id).await?;
    Ok(())
}
#[tauri::command]
pub async fn screen_verify(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    task_id: String,
) -> Result<(), CommandErrorDto> {
    let task = state
        .task_repository
        .get(&task_id)
        .await
        .map_err(CommandErrorDto::from)?;
    if task.local_project_id != local_project_id || task.domain_type != tasks::DOMAIN {
        return Err(
            crate::core::error::AppError::Conflict("任务不属于当前智能屏项目".into()).into(),
        );
    }
    tasks::verify(&state, &local_project_id, &task_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn screen_registration_preview(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    screen_ids: Vec<String>,
) -> Result<RegistrationPreview, CommandErrorDto> {
    registration::preview(&state, &local_project_id, screen_ids)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_registration_submit(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    input: RegistrationSubmission,
) -> Result<String, CommandErrorDto> {
    registration::submit(&state, &local_project_id, input)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_merge(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    candidate: serde_json::Value,
    decision: MergeDecision,
) -> Result<Option<MergeResult>, CommandErrorDto> {
    merge::merge(&state, &local_project_id, candidate, decision)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_parse_apk(file_path: String) -> Result<ApkInfo, CommandErrorDto> {
    apk::inspect(
        std::path::Path::new(&file_path),
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_version_preview(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    screen_ids: Vec<String>,
) -> Result<VersionPreview, CommandErrorDto> {
    value_updates::version_preview(&state, &local_project_id, screen_ids)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_version_submit(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    preview_id: String,
    screen_ids: Vec<String>,
) -> Result<String, CommandErrorDto> {
    value_updates::version_submit(&state, &local_project_id, &preview_id, screen_ids)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_cover_status(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    changes: Vec<StatusChange>,
) -> Result<Vec<StatusResult>, CommandErrorDto> {
    value_updates::cover_status(&state, &local_project_id, changes)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn screen_read_diagnostics(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    task_id: String,
) -> Result<String, CommandErrorDto> {
    crate::infrastructure::smart_screen::diagnostics::read(&state, &local_project_id, &task_id)
        .await
        .map_err(Into::into)
}
#[tauri::command]
pub async fn screen_export_diagnostics(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    task_id: String,
    directory: String,
) -> Result<String, CommandErrorDto> {
    crate::infrastructure::smart_screen::diagnostics::export(
        &state,
        &local_project_id,
        &task_id,
        std::path::Path::new(&directory),
    )
    .await
    .map_err(Into::into)
}
