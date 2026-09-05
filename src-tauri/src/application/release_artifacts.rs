use serde::{Deserialize, Serialize};

use crate::core::error::AppResult;
use crate::domain::aio::release::ImageArchiveInfo;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceImageInspection {
    pub archive: ImageArchiveInfo,
    pub expected_image: Option<String>,
    pub expected_matches: bool,
}

pub trait ReleaseArtifactsPort: Send + Sync {
    fn inspect_image(
        &self,
        path: &str,
        expected_image: Option<&str>,
    ) -> AppResult<ServiceImageInspection>;
}

pub fn inspect_service_image<P: ReleaseArtifactsPort>(
    port: &P,
    path: &str,
    expected_image: Option<&str>,
) -> AppResult<ServiceImageInspection> {
    port.inspect_image(path, expected_image)
}
