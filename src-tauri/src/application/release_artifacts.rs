use serde::{Deserialize, Serialize};

use crate::core::error::AppResult;
use crate::domain::aio::deployment::{DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::release::{ImageArchiveInfo, ReleaseValidation};
use crate::domain::aio::release_render::{ReleaseRenderContext, RenderedReleaseFiles};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceImageInspection {
    pub archive: ImageArchiveInfo,
    pub expected_image: Option<String>,
    pub expected_matches: bool,
}

pub trait ReleaseArtifactsPort: Send + Sync {
    fn validate_release(&self, path: &str) -> AppResult<ReleaseValidation>;
    fn inspect_image(
        &self,
        path: &str,
        expected_image: Option<&str>,
    ) -> AppResult<ServiceImageInspection>;
    fn render_preview(
        &self,
        release_dir: &str,
        context: &ReleaseRenderContext,
    ) -> AppResult<RenderedReleaseFiles>;
    fn build_plan(&self, input: DeploymentPlanInput) -> AppResult<DeploymentPlan>;
}

pub fn validate_release_package<P: ReleaseArtifactsPort>(
    port: &P,
    path: &str,
) -> AppResult<ReleaseValidation> {
    port.validate_release(path)
}

pub fn inspect_service_image<P: ReleaseArtifactsPort>(
    port: &P,
    path: &str,
    expected_image: Option<&str>,
) -> AppResult<ServiceImageInspection> {
    port.inspect_image(path, expected_image)
}

pub fn render_release_preview<P: ReleaseArtifactsPort>(
    port: &P,
    release_dir: &str,
    context: &ReleaseRenderContext,
) -> AppResult<RenderedReleaseFiles> {
    port.render_preview(release_dir, context)
}

pub fn build_deployment_plan<P: ReleaseArtifactsPort>(
    port: &P,
    input: DeploymentPlanInput,
) -> AppResult<DeploymentPlan> {
    port.build_plan(input)
}
