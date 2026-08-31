use std::path::Path;

use crate::application::release_artifacts::{ReleaseArtifactsPort, ServiceImageInspection};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::release::{
    ReleaseValidation, inspect_image_archive, inspect_release_directory,
};
use crate::domain::aio::release_render::{ReleaseRenderContext, RenderedReleaseFiles};
use crate::infrastructure::release_template::render_release_templates;

pub struct ReleaseArtifactsService;

impl ReleaseArtifactsPort for ReleaseArtifactsService {
    fn validate_release(&self, path: &str) -> AppResult<ReleaseValidation> {
        validate_release_package(path)
    }

    fn inspect_image(
        &self,
        path: &str,
        expected_image: Option<&str>,
    ) -> AppResult<ServiceImageInspection> {
        inspect_service_image(path, expected_image)
    }

    fn render_preview(
        &self,
        release_dir: &str,
        context: &ReleaseRenderContext,
    ) -> AppResult<RenderedReleaseFiles> {
        render_release_preview(release_dir, context)
    }

    fn build_plan(&self, input: DeploymentPlanInput) -> AppResult<DeploymentPlan> {
        build_deployment_plan(input)
    }
}

pub fn validate_release_package(path: &str) -> AppResult<ReleaseValidation> {
    inspect_release_directory(Path::new(path))
}

pub fn inspect_service_image(
    path: &str,
    expected_image: Option<&str>,
) -> AppResult<ServiceImageInspection> {
    let archive = inspect_image_archive(Path::new(path))?;
    let expected_image = expected_image
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let expected_matches = expected_image
        .as_ref()
        .is_none_or(|expected| archive.repo_tags.iter().any(|tag| tag == expected));
    Ok(ServiceImageInspection {
        archive,
        expected_image,
        expected_matches,
    })
}

pub fn render_release_preview(
    release_dir: &str,
    context: &ReleaseRenderContext,
) -> AppResult<RenderedReleaseFiles> {
    let validation = inspect_release_directory(Path::new(release_dir))?;
    if !validation.valid {
        return Err(AppError::InvalidConfig(format!(
            "Release校验失败：{}",
            validation.errors.join("；")
        )));
    }
    let manifest = validation
        .manifest
        .ok_or_else(|| AppError::InvalidConfig("Release缺少manifest".into()))?;
    let root = Path::new(&validation.package_dir);
    let env_template = std::fs::read_to_string(root.join(&manifest.templates.env))
        .map_err(|error| AppError::io("读取Release环境变量模板", &error))?;
    let host_info_template = std::fs::read_to_string(root.join(&manifest.templates.host_info))
        .map_err(|error| AppError::io("读取Release host-info模板", &error))?;
    let compose_template = std::fs::read_to_string(root.join(&manifest.compose_file))
        .map_err(|error| AppError::io("读取Release Compose", &error))?;
    render_release_templates(
        &env_template,
        &host_info_template,
        &compose_template,
        context,
    )
}

pub fn build_deployment_plan(input: DeploymentPlanInput) -> AppResult<DeploymentPlan> {
    DeploymentPlan::build(input)
}
