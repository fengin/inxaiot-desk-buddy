use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

pub const DEFAULT_WORKBENCH_DATABASE: &str = "inxaiot_desk_buddy";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInput {
    pub name: String,
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    #[serde(default)]
    pub db_tls_enabled: bool,
    #[serde(default)]
    pub db_password: Option<String>,
    pub business_db: String,
    #[serde(default = "default_workbench_database")]
    pub workbench_db: String,
}

impl ProjectInput {
    pub fn validate_for_create(&self) -> AppResult<()> {
        self.validate_common()?;
        if self.db_password.as_deref().is_none_or(str::is_empty) {
            return Err(AppError::InvalidConfig("数据库密码不能为空".into()));
        }
        Ok(())
    }

    pub fn validate_for_update(&self) -> AppResult<()> {
        self.validate_common()
    }

    pub fn validate_for_test(&self, existing_project_id: Option<&str>) -> AppResult<()> {
        self.validate_common()?;
        if existing_project_id.is_none() && self.db_password.as_deref().is_none_or(str::is_empty) {
            return Err(AppError::InvalidConfig(
                "测试未保存项目时必须填写数据库密码".into(),
            ));
        }
        Ok(())
    }

    fn validate_common(&self) -> AppResult<()> {
        if self.name.trim().is_empty()
            || self.platform_url.trim().is_empty()
            || self.db_host.trim().is_empty()
            || self.db_port == 0
            || self.db_user.trim().is_empty()
            || self.business_db.trim().is_empty()
            || self.workbench_db.trim().is_empty()
        {
            return Err(AppError::InvalidConfig("项目连接信息不完整".into()));
        }
        if !self.platform_url.starts_with("http://") && !self.platform_url.starts_with("https://") {
            return Err(AppError::InvalidConfig(
                "平台访问地址必须以 http:// 或 https:// 开头".into(),
            ));
        }
        if let Some(host) = self
            .platform_url
            .strip_prefix("http://")
            .and_then(|value| value.split(['/', ':']).next())
            && !is_private_network_host(host)
        {
            return Err(AppError::InvalidConfig(
                "非内网平台必须使用HTTPS；HTTP仅允许本机或RFC1918开发地址".into(),
            ));
        }
        if !valid_database_name(&self.business_db) || !valid_database_name(&self.workbench_db) {
            return Err(AppError::InvalidConfig("数据库名称包含非法字符".into()));
        }
        if self.business_db.eq_ignore_ascii_case(&self.workbench_db)
            || !self.workbench_db.starts_with("inxaiot_desk_buddy")
        {
            return Err(AppError::InvalidConfig(
                "工作台Schema必须使用inxaiot_desk_buddy命名空间，且不能与平台业务库相同".into(),
            ));
        }
        if self
            .db_password
            .as_deref()
            .is_some_and(|password| !password.is_empty() && password.len() < 3)
        {
            return Err(AppError::InvalidConfig(
                "数据库密码至少需要3个字节，以确保日志可安全脱敏".into(),
            ));
        }
        Ok(())
    }
}

pub fn is_private_network_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']).to_ascii_lowercase();
    if matches!(host.as_str(), "localhost" | "::1")
        || host.starts_with("127.")
        || host.ends_with(".test")
        || host.ends_with(".localhost")
    {
        return true;
    }
    if host.starts_with("10.") || host.starts_with("192.168.") {
        return true;
    }
    let Some(rest) = host.strip_prefix("172.") else {
        return false;
    };
    rest.split('.')
        .next()
        .and_then(|value| value.parse::<u8>().ok())
        .is_some_and(|octet| (16..=31).contains(&octet))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_tls_enabled: bool,
    pub business_db: String,
    pub workbench_db: String,
    pub last_opened_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectConnectionState {
    Disconnected,
    Connecting,
    LoginRequired,
    SessionExpired,
    SchemaRequired,
    Ready,
    ConnectionFailed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseConnectionState {
    Disconnected,
    Connected,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectOverview {
    #[serde(flatten)]
    pub project: ProjectRecord,
    pub connection_state: ProjectConnectionState,
    pub database_state: DatabaseConnectionState,
    pub schema_state: Option<String>,
    pub session: Option<ProjectSessionView>,
    pub connection_encrypted: bool,
    pub status_message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConnectionTestRequest {
    pub existing_project_id: Option<String>,
    pub project: ProjectInput,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConnectionTestResult {
    pub successful: bool,
    pub platform_database_connected: bool,
    pub workbench_database_connected: bool,
    pub platform_schema_compatible: bool,
    pub workbench_schema_state: String,
    pub workbench_schema_message: String,
    pub mysql_version: String,
    pub connection_encrypted: bool,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformLoginRequest {
    pub username: String,
    pub password: String,
    pub session_uuid: String,
    pub image_code: String,
}

impl PlatformLoginRequest {
    pub fn validate(&self) -> AppResult<()> {
        if self.username.trim().is_empty()
            || self.password.len() < 3
            || self.session_uuid.trim().is_empty()
            || self.image_code.trim().is_empty()
        {
            return Err(AppError::InvalidConfig(
                "平台登录参数不完整，且密码至少需要3个字节".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSessionState {
    Active,
    Missing,
    Expired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSessionView {
    pub local_project_id: String,
    pub username: Option<String>,
    pub state: ProjectSessionState,
    pub expires_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlatformLoginChallenge {
    pub session_uuid: String,
    pub captcha_image_data_url: Option<String>,
    pub requires_captcha: bool,
    pub expires_at_epoch_seconds: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyCaptureRequest {
    pub host: String,
    pub port: Option<u16>,
}

impl HostKeyCaptureRequest {
    pub fn validate(&self) -> AppResult<()> {
        if self.host.trim().is_empty() || self.port == Some(0) {
            return Err(AppError::InvalidConfig("SSH主机或端口无效".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HostKeyState {
    Unconfirmed,
    Confirmed,
    Changed,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostKeyObservation {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    pub state: HostKeyState,
    pub expected_fingerprint: Option<String>,
    pub accepted_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmHostKeyRequest {
    pub host: String,
    pub port: u16,
    pub algorithm: String,
    pub fingerprint: String,
    #[serde(default)]
    pub replace_changed: bool,
}

fn valid_database_name(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn default_workbench_database() -> String {
    DEFAULT_WORKBENCH_DATABASE.into()
}
