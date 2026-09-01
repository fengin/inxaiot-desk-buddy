use tauri::State;

use crate::application::project_management::{
    capture_host_key as capture_host_key_use_case,
    check_project_session as check_project_session_use_case,
    confirm_host_key as confirm_host_key_use_case,
    create_login_challenge as create_login_challenge_use_case,
    create_project as create_project_use_case, delete_project as delete_project_use_case,
    export_release_master_key as export_release_master_key_use_case,
    get_project_session as get_project_session_use_case,
    get_release_profile as get_release_profile_use_case,
    import_release_master_key as import_release_master_key_use_case,
    list_host_keys as list_host_keys_use_case, list_projects as list_projects_use_case,
    login_project as login_project_use_case, logout_project as logout_project_use_case,
    save_release_profile as save_release_profile_use_case,
    switch_project as switch_project_use_case,
    test_project_connection as test_project_connection_use_case,
    update_project as update_project_use_case,
    validate_release_profile as validate_release_profile_use_case,
};
use crate::domain::aio::release_profile::{
    ReleaseMasterKeyOperationResult, ReleaseMasterKeyTransferRequest, ReleaseProfileDraft,
    ReleaseProfileValidation, ReleaseProfileView,
};
use crate::domain::common::project::{
    ConfirmHostKeyRequest, HostKeyCaptureRequest, HostKeyObservation, PlatformLoginChallenge,
    PlatformLoginRequest, ProjectConnectionTestRequest, ProjectConnectionTestResult, ProjectInput,
    ProjectOverview, ProjectSessionView,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::stage75_adapter::Stage75Adapter;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn list_local_projects(
    state: State<'_, FormalAppState>,
) -> Result<Vec<ProjectOverview>, CommandErrorDto> {
    list_projects_use_case(&Stage75Adapter::new(&state))
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn create_local_project(
    state: State<'_, FormalAppState>,
    input: ProjectInput,
) -> Result<ProjectOverview, CommandErrorDto> {
    create_project_use_case(&Stage75Adapter::new(&state), input)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn update_local_project(
    state: State<'_, FormalAppState>,
    project_id: String,
    input: ProjectInput,
) -> Result<ProjectOverview, CommandErrorDto> {
    update_project_use_case(&Stage75Adapter::new(&state), &project_id, input)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn delete_local_project(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<(), CommandErrorDto> {
    delete_project_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn test_project_connection(
    state: State<'_, FormalAppState>,
    request: ProjectConnectionTestRequest,
) -> Result<ProjectConnectionTestResult, CommandErrorDto> {
    test_project_connection_use_case(&Stage75Adapter::new(&state), request)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn switch_project(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<ProjectOverview, CommandErrorDto> {
    switch_project_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn create_project_login_challenge(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<PlatformLoginChallenge, CommandErrorDto> {
    create_login_challenge_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn login_project(
    state: State<'_, FormalAppState>,
    project_id: String,
    request: PlatformLoginRequest,
) -> Result<ProjectSessionView, CommandErrorDto> {
    login_project_use_case(&Stage75Adapter::new(&state), &project_id, request)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_project_session(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<ProjectSessionView, CommandErrorDto> {
    get_project_session_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn check_project_session(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<ProjectSessionView, CommandErrorDto> {
    check_project_session_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn logout_project(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<ProjectSessionView, CommandErrorDto> {
    logout_project_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn get_release_profile(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<Option<ReleaseProfileView>, CommandErrorDto> {
    get_release_profile_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub fn validate_release_profile(
    draft: ReleaseProfileDraft,
) -> Result<ReleaseProfileValidation, CommandErrorDto> {
    validate_release_profile_use_case(&draft).map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn save_release_profile(
    state: State<'_, FormalAppState>,
    project_id: String,
    draft: ReleaseProfileDraft,
) -> Result<ReleaseProfileView, CommandErrorDto> {
    save_release_profile_use_case(&Stage75Adapter::new(&state), &project_id, draft)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn export_release_master_key(
    state: State<'_, FormalAppState>,
    project_id: String,
    request: ReleaseMasterKeyTransferRequest,
) -> Result<ReleaseMasterKeyOperationResult, CommandErrorDto> {
    export_release_master_key_use_case(&Stage75Adapter::new(&state), &project_id, request)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn import_release_master_key(
    state: State<'_, FormalAppState>,
    project_id: String,
    request: ReleaseMasterKeyTransferRequest,
) -> Result<ReleaseMasterKeyOperationResult, CommandErrorDto> {
    import_release_master_key_use_case(&Stage75Adapter::new(&state), &project_id, request)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn list_host_keys(
    state: State<'_, FormalAppState>,
    project_id: String,
) -> Result<Vec<HostKeyObservation>, CommandErrorDto> {
    list_host_keys_use_case(&Stage75Adapter::new(&state), &project_id)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn capture_host_key(
    state: State<'_, FormalAppState>,
    project_id: String,
    request: HostKeyCaptureRequest,
) -> Result<HostKeyObservation, CommandErrorDto> {
    capture_host_key_use_case(&Stage75Adapter::new(&state), &project_id, request)
        .await
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn confirm_host_key(
    state: State<'_, FormalAppState>,
    project_id: String,
    request: ConfirmHostKeyRequest,
) -> Result<HostKeyObservation, CommandErrorDto> {
    confirm_host_key_use_case(&Stage75Adapter::new(&state), &project_id, request)
        .await
        .map_err(CommandErrorDto::from)
}
