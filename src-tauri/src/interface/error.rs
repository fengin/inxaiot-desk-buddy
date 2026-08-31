use std::collections::BTreeMap;

use serde::Serialize;
use uuid::Uuid;

use crate::core::error::AppError;
use crate::infrastructure::logging::redactor::SensitiveValueRedactor;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandErrorDto {
    pub code: String,
    pub message_key: String,
    pub params: BTreeMap<String, String>,
    pub trace_id: String,
}

impl From<AppError> for CommandErrorDto {
    fn from(error: AppError) -> Self {
        let trace_id = Uuid::now_v7().to_string();
        let redactor = SensitiveValueRedactor::production();
        let summary = redactor.redact_text(&error.to_string());
        tracing::error!(trace_id, error = %summary, "tauri command failed");
        let (code, message_key) = match &error {
            AppError::InvalidConfig(_) => ("CONFIG_VALIDATION_FAILED", "error.invalid_config"),
            AppError::Conflict(message) if message.contains("发布配置") => {
                ("CONFIG_VERSION_CONFLICT", "error.config_version_conflict")
            }
            AppError::Conflict(_) => ("STATE_CONFLICT", "error.state_conflict"),
            AppError::NotFound(_) => ("NOT_FOUND", "error.not_found"),
            AppError::Database { .. } => ("LOCAL_DATABASE", "error.local_database"),
            AppError::PlatformHttp { .. } => ("PLATFORM_HTTP", "error.platform_http"),
            AppError::Authentication(_) => ("PLATFORM_AUTH", "error.platform_auth"),
            AppError::Ssh { .. } => ("SSH_OPERATION_FAILED", "error.ssh"),
            AppError::Sftp { .. } => ("SFTP_OPERATION_FAILED", "error.sftp"),
            AppError::Io { .. } => ("LOCAL_IO", "error.local_io"),
            AppError::Timeout { .. } => ("REMOTE_TIMEOUT", "error.remote_timeout"),
            AppError::HostKeyChanged { .. } => ("SSH_HOST_KEY_CHANGED", "error.host_key_changed"),
            AppError::Integrity { .. } => ("FILE_INTEGRITY_FAILED", "error.file_integrity"),
            AppError::Cancelled => ("OPERATION_CANCELLED", "error.cancelled"),
        };
        let mut params = BTreeMap::new();
        params.insert("summary".into(), summary);
        Self {
            code: code.into(),
            message_key: message_key.into(),
            params,
            trace_id,
        }
    }
}
