use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug)]
pub struct UploadRequest {
    pub operation_id: String,
    pub local_path: PathBuf,
    pub remote_path: String,
    pub expected_sha256: Option<String>,
    pub overwrite: bool,
    pub chunk_size: usize,
    pub inactivity_timeout: Duration,
    pub minimum_bytes_per_second: u64,
    pub minimum_total_timeout: Duration,
}

impl UploadRequest {
    pub fn validate(&self) -> AppResult<()> {
        validate_operation_id(&self.operation_id)?;
        validate_remote_path(&self.remote_path)?;
        if !self.local_path.is_file()
            || self.chunk_size == 0
            || self.inactivity_timeout.is_zero()
            || self.minimum_bytes_per_second == 0
            || self.minimum_total_timeout.is_zero()
        {
            return Err(AppError::InvalidConfig("SFTP上传参数无效".into()));
        }
        validate_sha256(self.expected_sha256.as_deref())
    }

    pub fn temporary_remote_path(&self) -> String {
        format!("{}.part-{}", self.remote_path, self.operation_id)
    }
}

#[derive(Clone, Debug)]
pub struct DownloadRequest {
    pub operation_id: String,
    pub remote_path: String,
    pub local_path: PathBuf,
    pub expected_sha256: Option<String>,
    pub overwrite: bool,
    pub chunk_size: usize,
    pub inactivity_timeout: Duration,
    pub minimum_bytes_per_second: u64,
    pub minimum_total_timeout: Duration,
}

impl DownloadRequest {
    pub fn validate(&self) -> AppResult<()> {
        validate_operation_id(&self.operation_id)?;
        validate_remote_path(&self.remote_path)?;
        if self.local_path.as_os_str().is_empty()
            || self.chunk_size == 0
            || self.inactivity_timeout.is_zero()
            || self.minimum_bytes_per_second == 0
            || self.minimum_total_timeout.is_zero()
        {
            return Err(AppError::InvalidConfig("SFTP下载参数无效".into()));
        }
        validate_sha256(self.expected_sha256.as_deref())
    }

    pub fn temporary_local_path(&self) -> PathBuf {
        let mut value = self.local_path.as_os_str().to_os_string();
        value.push(format!(".part-{}", self.operation_id));
        PathBuf::from(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub transferred: u64,
    pub total: u64,
}

pub trait TransferProgressSink: Send + Sync {
    fn emit(&self, progress: TransferProgress) -> AppResult<()>;
}

#[derive(Default)]
pub struct NoopTransferProgressSink;

impl TransferProgressSink for NoopTransferProgressSink {
    fn emit(&self, _progress: TransferProgress) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteFileMetadata {
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub modified_at_epoch_seconds: Option<u32>,
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symlink: bool,
}

#[allow(async_fn_in_trait)]
pub trait FileTransferService: Send + Sync {
    async fn upload(
        &self,
        request: &UploadRequest,
        cancellation: &CancellationToken,
        progress: &dyn TransferProgressSink,
    ) -> AppResult<()>;

    async fn download(
        &self,
        request: &DownloadRequest,
        cancellation: &CancellationToken,
        progress: &dyn TransferProgressSink,
    ) -> AppResult<()>;

    async fn stat(&self, remote_path: &str) -> AppResult<RemoteFileMetadata>;
    async fn rename(&self, source: &str, destination: &str) -> AppResult<()>;
    async fn remove_file(&self, remote_path: &str) -> AppResult<()>;
}

pub fn validate_remote_path(value: &str) -> AppResult<()> {
    if !value.starts_with('/')
        || value.contains('\0')
        || value.split('/').any(|segment| segment == "..")
    {
        return Err(AppError::InvalidConfig(
            "远端路径必须是无穿越的绝对路径".into(),
        ));
    }
    Ok(())
}

fn validate_operation_id(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(AppError::InvalidConfig("传输操作ID无效".into()));
    }
    Ok(())
}

fn validate_sha256(value: Option<&str>) -> AppResult<()> {
    if value
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(AppError::InvalidConfig("SHA-256格式无效".into()));
    }
    Ok(())
}
