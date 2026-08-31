use std::path::{Path, PathBuf};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::error::AppResult;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileDescriptor {
    pub path: PathBuf,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationWorkspace {
    pub root: PathBuf,
    pub artifacts_dir: PathBuf,
    pub temporary_dir: PathBuf,
    pub log_path: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileProcessingStage {
    Hashing,
    Copying,
    Verifying,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProcessingProgress {
    pub stage: FileProcessingStage,
    pub processed: u64,
    pub total: u64,
}

pub trait FileProcessingProgressSink: Send + Sync {
    fn emit(&self, progress: FileProcessingProgress) -> AppResult<()>;
}

#[derive(Default)]
pub struct NoopFileProcessingProgressSink;

impl FileProcessingProgressSink for NoopFileProcessingProgressSink {
    fn emit(&self, _progress: FileProcessingProgress) -> AppResult<()> {
        Ok(())
    }
}

#[allow(async_fn_in_trait)]
pub trait FileProcessingService: Send + Sync {
    async fn inspect_selected_file(
        &self,
        selected_root: &Path,
        path: &Path,
        cancellation: &CancellationToken,
        progress: &dyn FileProcessingProgressSink,
    ) -> AppResult<LocalFileDescriptor>;

    async fn create_workspace(
        &self,
        local_project_id: &str,
        local_task_id: &str,
    ) -> AppResult<OperationWorkspace>;

    async fn copy_to_workspace(
        &self,
        source: &LocalFileDescriptor,
        workspace: &OperationWorkspace,
        target_name: &str,
        cancellation: &CancellationToken,
        progress: &dyn FileProcessingProgressSink,
    ) -> AppResult<LocalFileDescriptor>;

    async fn cleanup_workspace(&self, workspace: &OperationWorkspace) -> AppResult<()>;
}
