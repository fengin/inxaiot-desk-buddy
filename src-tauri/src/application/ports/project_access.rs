use crate::application::project_access::ProjectAccessRequirement;
use crate::core::error::AppResult;

#[allow(async_fn_in_trait)]
pub trait ProjectAccessPort: Send + Sync {
    async fn require_project_access(
        &self,
        project_id: &str,
        requirement: ProjectAccessRequirement,
    ) -> AppResult<()>;
}
