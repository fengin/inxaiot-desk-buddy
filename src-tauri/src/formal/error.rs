use std::collections::BTreeMap;

use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

pub type FormalResult<T> = Result<T, FormalError>;

#[derive(Debug, Error)]
pub enum FormalError {
    #[error("配置无效：{0}")]
    InvalidConfig(String),
    #[error("本地数据库操作失败：{0}")]
    LocalDatabase(&'static str),
    #[error("本机凭据存储操作失败：{0}")]
    SecretStore(&'static str),
    #[error("本地文件操作失败：{0}")]
    LocalIo(&'static str),
    #[error("资源状态冲突：{0}")]
    Conflict(String),
    #[error("未找到资源：{0}")]
    NotFound(String),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorDto {
    pub code: String,
    pub message_key: String,
    pub params: BTreeMap<String, String>,
    pub trace_id: String,
}

impl FormalError {
    pub fn to_dto(&self) -> ErrorDto {
        let (code, message_key) = match self {
            Self::InvalidConfig(_) => ("INVALID_CONFIG", "error.invalid_config"),
            Self::LocalDatabase(_) => ("LOCAL_DATABASE", "error.local_database"),
            Self::SecretStore(_) => ("SECRET_STORE", "error.secret_store"),
            Self::LocalIo(_) => ("LOCAL_IO", "error.local_io"),
            Self::Conflict(_) => ("CONFLICT", "error.conflict"),
            Self::NotFound(_) => ("NOT_FOUND", "error.not_found"),
        };
        let mut params = BTreeMap::new();
        params.insert("summary".into(), self.to_string());
        ErrorDto {
            code: code.into(),
            message_key: message_key.into(),
            params,
            trace_id: Uuid::now_v7().to_string(),
        }
    }
}
