use crate::core::error::AppResult;
use crate::domain::aio::release_profile::{
    ReleaseAgentScriptReplaceRequest, ReleaseProfileDraft, ReleaseProfileView,
};
use crate::domain::common::project::{
    ConfirmHostKeyRequest, HostKeyCaptureRequest, HostKeyObservation, PlatformLoginChallenge,
    PlatformLoginRequest, ProjectConnectionTestRequest, ProjectConnectionTestResult, ProjectInput,
    ProjectOverview, ProjectSessionView,
};

#[allow(async_fn_in_trait)]
pub trait ProjectManagementPort: Send + Sync {
    async fn list_projects(&self) -> AppResult<Vec<ProjectOverview>>;
    async fn create_project(&self, input: ProjectInput) -> AppResult<ProjectOverview>;
    async fn update_project(
        &self,
        project_id: &str,
        input: ProjectInput,
    ) -> AppResult<ProjectOverview>;
    async fn delete_project(&self, project_id: &str) -> AppResult<()>;
    async fn test_project_connection(
        &self,
        request: ProjectConnectionTestRequest,
    ) -> AppResult<ProjectConnectionTestResult>;
    async fn switch_project(&self, project_id: &str) -> AppResult<ProjectOverview>;
    async fn create_login_challenge(&self, project_id: &str) -> AppResult<PlatformLoginChallenge>;
    async fn login_project(
        &self,
        project_id: &str,
        request: PlatformLoginRequest,
    ) -> AppResult<ProjectSessionView>;
    async fn get_project_session(&self, project_id: &str) -> AppResult<ProjectSessionView>;
    async fn check_project_session(&self, project_id: &str) -> AppResult<ProjectSessionView>;
    async fn logout_project(&self, project_id: &str) -> AppResult<ProjectSessionView>;
}

#[allow(async_fn_in_trait)]
pub trait ReleaseProfileManagementPort: Send + Sync {
    async fn get_release_profile(&self, project_id: &str) -> AppResult<Option<ReleaseProfileView>>;
    async fn save_release_profile(
        &self,
        project_id: &str,
        draft: ReleaseProfileDraft,
    ) -> AppResult<ReleaseProfileView>;
    async fn replace_release_agent_script(
        &self,
        project_id: &str,
        request: ReleaseAgentScriptReplaceRequest,
    ) -> AppResult<ReleaseProfileView>;
    async fn open_release_agent_script(&self, project_id: &str) -> AppResult<()>;
}

#[allow(async_fn_in_trait)]
pub trait HostKeyManagementPort: Send + Sync {
    async fn list_host_keys(&self, project_id: &str) -> AppResult<Vec<HostKeyObservation>>;
    async fn capture_host_key(
        &self,
        project_id: &str,
        request: HostKeyCaptureRequest,
    ) -> AppResult<HostKeyObservation>;
    async fn confirm_host_key(
        &self,
        project_id: &str,
        request: ConfirmHostKeyRequest,
    ) -> AppResult<HostKeyObservation>;
}
