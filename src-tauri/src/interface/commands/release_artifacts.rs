use crate::application::release_artifacts::{
    ServiceImageInspection, build_deployment_plan as build_plan,
    inspect_service_image as inspect_image, render_release_preview as render_preview,
    validate_release_package as validate_release,
};
use crate::domain::aio::deployment::{DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::release::ReleaseValidation;
use crate::infrastructure::release_template::{ReleaseRenderContext, RenderedReleaseFiles};
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn validate_release_package(path: String) -> Result<ReleaseValidation, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || validate_release(&path))
        .await
        .map_err(|_| {
            CommandErrorDto::from(crate::core::error::AppError::Io {
                operation: "等待Release校验任务",
            })
        })?
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn inspect_service_image(
    path: String,
    expected_image: Option<String>,
) -> Result<ServiceImageInspection, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || inspect_image(&path, expected_image.as_deref()))
        .await
        .map_err(|_| {
            CommandErrorDto::from(crate::core::error::AppError::Io {
                operation: "等待镜像检查任务",
            })
        })?
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn render_release_preview(
    release_dir: String,
    context: ReleaseRenderContext,
) -> Result<RenderedReleaseFiles, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || render_preview(&release_dir, &context))
        .await
        .map_err(|_| {
            CommandErrorDto::from(crate::core::error::AppError::Io {
                operation: "等待模板预览任务",
            })
        })?
        .map_err(CommandErrorDto::from)
}

#[tauri::command]
pub async fn build_deployment_plan(
    input: DeploymentPlanInput,
) -> Result<DeploymentPlan, CommandErrorDto> {
    build_plan(input).map_err(CommandErrorDto::from)
}
