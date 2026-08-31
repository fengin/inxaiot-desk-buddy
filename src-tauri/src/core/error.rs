use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("配置无效：{0}")]
    InvalidConfig(String),
    #[error("资源状态冲突：{0}")]
    Conflict(String),
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
        tracing::error!(operation, error = ?error, "database operation failed");
        Self::Database { operation }
    }

    pub fn platform_http(operation: &'static str, error: &reqwest::Error) -> Self {
        tracing::error!(operation, error = ?error, "platform http operation failed");
        Self::PlatformHttp { operation }
    }

    pub fn ssh(operation: &'static str, error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, error = ?error, "ssh operation failed");
        Self::Ssh { operation }
    }

    pub fn sftp(operation: &'static str, error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, error = ?error, "sftp operation failed");
        Self::Sftp { operation }
    }

    pub fn io(operation: &'static str, error: &std::io::Error) -> Self {
        tracing::error!(operation, error = ?error, "file operation failed");
        Self::Io { operation }
    }

    pub fn timeout(operation: &'static str) -> Self {
        Self::Timeout { operation }
    }

    pub fn integrity(operation: &'static str, error: impl std::fmt::Debug) -> Self {
        tracing::error!(operation, error = ?error, "file integrity check failed");
        Self::Integrity { operation }
    }
}
