use crate::application::ports::project_management::{
    HostKeyManagementPort, ProjectManagementPort, ReleaseProfileManagementPort,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::release_profile::{
    ReleaseAgentScriptReplaceRequest, ReleaseProfileDraft, ReleaseProfileValidation,
    ReleaseProfileView,
};
use crate::domain::common::project::{
    ConfirmHostKeyRequest, HostKeyCaptureRequest, HostKeyObservation, PlatformLoginChallenge,
    PlatformLoginRequest, ProjectConnectionTestRequest, ProjectConnectionTestResult, ProjectInput,
    ProjectOverview, ProjectSessionView,
};

pub async fn list_projects<P: ProjectManagementPort>(port: &P) -> AppResult<Vec<ProjectOverview>> {
    port.list_projects().await
}

pub async fn create_project<P: ProjectManagementPort>(
    port: &P,
    input: ProjectInput,
) -> AppResult<ProjectOverview> {
    input.validate_for_create()?;
    port.create_project(input).await
}

pub async fn update_project<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
    input: ProjectInput,
) -> AppResult<ProjectOverview> {
    input.validate_for_update()?;
    port.update_project(project_id, input).await
}

pub async fn delete_project<P: ProjectManagementPort>(port: &P, project_id: &str) -> AppResult<()> {
    port.delete_project(project_id).await
}

pub async fn test_project_connection<P: ProjectManagementPort>(
    port: &P,
    request: ProjectConnectionTestRequest,
) -> AppResult<ProjectConnectionTestResult> {
    request
        .project
        .validate_for_test(request.existing_project_id.as_deref())?;
    port.test_project_connection(request).await
}

pub async fn switch_project<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<ProjectOverview> {
    port.switch_project(project_id).await
}

pub async fn create_login_challenge<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<PlatformLoginChallenge> {
    port.create_login_challenge(project_id).await
}

pub async fn login_project<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
    request: PlatformLoginRequest,
) -> AppResult<ProjectSessionView> {
    request.validate()?;
    port.login_project(project_id, request).await
}

pub async fn get_project_session<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<ProjectSessionView> {
    port.get_project_session(project_id).await
}

pub async fn check_project_session<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<ProjectSessionView> {
    port.check_project_session(project_id).await
}

pub async fn logout_project<P: ProjectManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<ProjectSessionView> {
    port.logout_project(project_id).await
}

pub async fn get_release_profile<P: ReleaseProfileManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<Option<ReleaseProfileView>> {
    port.get_release_profile(project_id).await
}

pub fn validate_release_profile(
    draft: &ReleaseProfileDraft,
) -> AppResult<ReleaseProfileValidation> {
    draft.validate()
}

pub async fn save_release_profile<P: ReleaseProfileManagementPort>(
    port: &P,
    project_id: &str,
    draft: ReleaseProfileDraft,
) -> AppResult<ReleaseProfileView> {
    draft.validate()?;
    port.save_release_profile(project_id, draft).await
}

pub async fn replace_release_agent_script<P: ReleaseProfileManagementPort>(
    port: &P,
    project_id: &str,
    request: ReleaseAgentScriptReplaceRequest,
) -> AppResult<ReleaseProfileView> {
    request.validate()?;
    port.replace_release_agent_script(project_id, request).await
}

pub async fn open_release_agent_script<P: ReleaseProfileManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<()> {
    port.open_release_agent_script(project_id).await
}

pub async fn list_host_keys<P: HostKeyManagementPort>(
    port: &P,
    project_id: &str,
) -> AppResult<Vec<HostKeyObservation>> {
    port.list_host_keys(project_id).await
}

pub async fn capture_host_key<P: HostKeyManagementPort>(
    port: &P,
    project_id: &str,
    request: HostKeyCaptureRequest,
) -> AppResult<HostKeyObservation> {
    request.validate()?;
    port.capture_host_key(project_id, request).await
}

pub async fn confirm_host_key<P: HostKeyManagementPort>(
    port: &P,
    project_id: &str,
    request: ConfirmHostKeyRequest,
) -> AppResult<HostKeyObservation> {
    if request.host.trim().is_empty()
        || request.port == 0
        || request.algorithm.trim().is_empty()
        || request.fingerprint.trim().is_empty()
    {
        return Err(AppError::InvalidConfig("主机密钥确认参数不完整".into()));
    }
    port.confirm_host_key(project_id, request).await
}
