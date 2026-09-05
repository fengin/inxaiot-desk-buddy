use crate::application::release_artifacts::{
    ServiceImageInspection, inspect_service_image as inspect_image,
};
use crate::infrastructure::release_artifacts_service::ReleaseArtifactsService;
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn inspect_service_image(
    path: String,
    expected_image: Option<String>,
) -> Result<ServiceImageInspection, CommandErrorDto> {
    tauri::async_runtime::spawn_blocking(move || {
        inspect_image(&ReleaseArtifactsService, &path, expected_image.as_deref())
    })
    .await
    .map_err(|_| {
        CommandErrorDto::from(crate::core::error::AppError::Io {
            operation: "等待镜像检查任务",
        })
    })?
    .map_err(CommandErrorDto::from)
}
