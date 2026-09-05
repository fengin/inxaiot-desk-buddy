use std::path::Path;

use crate::application::release_artifacts::{ReleaseArtifactsPort, ServiceImageInspection};
use crate::core::error::AppResult;
use crate::domain::aio::release::inspect_image_archive;

pub struct ReleaseArtifactsService;

impl ReleaseArtifactsPort for ReleaseArtifactsService {
    fn inspect_image(
        &self,
        path: &str,
        expected_image: Option<&str>,
    ) -> AppResult<ServiceImageInspection> {
        inspect_service_image(path, expected_image)
    }
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
    let expected_matches = expected_image.as_ref().is_none_or(|expected| {
        archive.repo_tags.is_empty() || archive.repo_tags.iter().any(|tag| tag == expected)
    });
    Ok(ServiceImageInspection {
        archive,
        expected_image,
        expected_matches,
    })
}
