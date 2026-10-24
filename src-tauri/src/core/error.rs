use std::collections::BTreeMap;

use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("配置无效：{0}")]
    InvalidConfig(String),
    #[error("参数校验失败，请修正标红的输入项")]
    InvalidFields(BTreeMap<String, String>),
    #[error("资源状态冲突：{0}")]
    Conflict(String),
    #[error("该智能屏正在由其他电脑操作，请确认是否接手")]
    ConfirmationRequired { code: &'static str, details: serde_json::Value },
    #[error("未找到资源：{0}")]
    NotFound(String),
    #[error("数据库操作失败：{operation}")]
    Database { operation: &'static str },
    #[error("平台请求失败：{operation}")]
    PlatformHttp { operation: &'static str },
    #[error("平台认证失败：{0}")]
    Authentication(String),
    #[error("SSH操作失败：{operation}")]
    Ssh { operation: &'static str },
    #[error("SFTP操作失败：{operation}")]
    Sftp { operation: &'static str },
    #[error("文件操作失败：{operation}")]
    Io { operation: &'static str },
    #[error("操作超时：{operation}")]
    Timeout { operation: &'static str },
    #[error("SSH主机密钥发生变化：期望 {expected}，实际 {actual}")]
    HostKeyChanged { expected: String, actual: String },
    #[error("文件完整性校验失败：{operation}")]
    Integrity { operation: &'static str },
    #[error("操作已取消")]
    Cancelled,
}

impl AppError {
    pub fn database(operation: &'static str, error: &sqlx::Error) -> Self {
        // 仅记录类别和标准错误码；原始错误可能带有 SQL、参数或连接凭据。
        let database_code = error
            .as_database_error()
            .and_then(|error| error.code())
            .filter(|code| safe_database_code(code));
        tracing::error!(
            operation,
            error_kind = database_error_kind(error),
            database_code = database_code.as_deref().unwrap_or("unavailable"),
            "database operation failed"
        );
        Self::Database { operation }
    }

    pub fn platform_http(operation: &'static str, _error: &reqwest::Error) -> Self {
        tracing::error!(operation, "platform http operation failed");
        Self::PlatformHttp { operation }
    }

    pub fn ssh(operation: &'static str, _error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, "ssh operation failed");
        Self::Ssh { operation }
    }

    pub fn sftp(operation: &'static str, _error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, "sftp operation failed");
        Self::Sftp { operation }
    }

    pub fn io(operation: &'static str, error: &std::io::Error) -> Self {
        tracing::error!(operation, error_kind = ?error.kind(), "file operation failed");
        Self::Io { operation }
    }

    pub fn timeout(operation: &'static str) -> Self {
        Self::Timeout { operation }
    }

    pub fn integrity(operation: &'static str, _error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, "file integrity check failed");
        Self::Integrity { operation }
    }
}

fn safe_database_code(code: &str) -> bool {
    // SQLite 扩展数字码或数据库驱动提供的五位 SQLSTATE。
    (!code.is_empty() && code.len() <= 10 && code.bytes().all(|byte| byte.is_ascii_digit()))
        || (code.len() == 5
            && code
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase()))
}

fn database_error_kind(error: &sqlx::Error) -> &'static str {
    match error {
        sqlx::Error::Configuration(_) => "configuration",
        sqlx::Error::InvalidArgument(_) => "invalid_argument",
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::RowNotFound => "row_not_found",
        sqlx::Error::TypeNotFound { .. } => "type_not_found",
        sqlx::Error::ColumnIndexOutOfBounds { .. } => "column_index_out_of_bounds",
        sqlx::Error::ColumnNotFound(_) => "column_not_found",
        sqlx::Error::ColumnDecode { .. } => "column_decode",
        sqlx::Error::Encode(_) => "encode",
        sqlx::Error::Decode(_) => "decode",
        sqlx::Error::AnyDriverError(_) => "driver",
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::WorkerCrashed => "worker_crashed",
        sqlx::Error::Migrate(_) => "migration",
        sqlx::Error::InvalidSavePointStatement => "invalid_savepoint",
        sqlx::Error::BeginFailed => "begin_failed",
        _ => "other",
    }
}
