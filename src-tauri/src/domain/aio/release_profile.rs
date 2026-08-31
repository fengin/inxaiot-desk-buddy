use std::collections::BTreeSet;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileValues {
    pub env_template: String,
    pub compose_template: String,
    pub platform_host: String,
    pub platform_api_port: u16,
    pub platform_mqtt_host: String,
    pub platform_mqtt_port: u16,
    pub ssh_port: u16,
    pub ssh_timeout_seconds: u32,
    pub aio_data_root: String,
    pub aio_deploy_root: String,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileCredentials {
    pub platform_auth_key: String,
    pub platform_mqtt_user: String,
    pub platform_mqtt_password: String,
    pub aio_mqtt_user: String,
    pub aio_mqtt_password: String,
    pub ssh_user: String,
    pub ssh_password: Option<String>,
    pub ssh_private_key: Option<String>,
}

impl std::fmt::Debug for ReleaseProfileCredentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReleaseProfileCredentials([REDACTED])")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileDraft {
    pub values: ReleaseProfileValues,
    pub credentials: ReleaseProfileCredentials,
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileView {
    pub profile_key: String,
    pub values: ReleaseProfileValues,
    pub credentials: ReleaseProfileCredentials,
    pub version: u64,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileValidation {
    pub valid: bool,
    pub recognized_placeholder_count: usize,
    pub warnings: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseMasterKeyTransferRequest {
    pub file_path: String,
    pub passphrase: String,
}

impl std::fmt::Debug for ReleaseMasterKeyTransferRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReleaseMasterKeyTransferRequest")
            .field("file_path", &self.file_path)
            .field("passphrase", &"[REDACTED]")
            .finish()
    }
}

impl ReleaseMasterKeyTransferRequest {
    pub fn validate(&self) -> AppResult<()> {
        if self.file_path.trim().is_empty() {
            return Err(AppError::InvalidConfig("项目主密钥包路径不能为空".into()));
        }
        if !(12..=1024).contains(&self.passphrase.len()) {
            return Err(AppError::InvalidConfig(
                "密钥包口令长度必须为12到1024个字节".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseMasterKeyOperationResult {
    pub key_version: u32,
    pub message: String,
}

impl ReleaseProfileDraft {
    pub fn validate(&self) -> AppResult<ReleaseProfileValidation> {
        let values = &self.values;
        let credentials = &self.credentials;
        if values.env_template.trim().is_empty()
            || values.compose_template.trim().is_empty()
            || values.platform_host.trim().is_empty()
            || values.platform_api_port == 0
            || values.platform_mqtt_host.trim().is_empty()
            || values.platform_mqtt_port == 0
            || values.ssh_port == 0
            || !(1..=3600).contains(&values.ssh_timeout_seconds)
            || credentials.platform_auth_key.trim().is_empty()
            || credentials.platform_mqtt_user.trim().is_empty()
            || credentials.platform_mqtt_password.is_empty()
            || credentials.aio_mqtt_user.trim().is_empty()
            || credentials.aio_mqtt_password.is_empty()
            || credentials.ssh_user.trim().is_empty()
        {
            return Err(AppError::InvalidConfig("发布参数或凭据不完整".into()));
        }
        if credentials
            .ssh_password
            .as_deref()
            .is_none_or(str::is_empty)
            && credentials
                .ssh_private_key
                .as_deref()
                .is_none_or(str::is_empty)
        {
            return Err(AppError::InvalidConfig(
                "SSH密码和SSH私钥至少填写一项".into(),
            ));
        }
        if credentials.platform_auth_key.len() < 3
            || credentials.platform_mqtt_password.len() < 3
            || credentials.aio_mqtt_password.len() < 3
            || credentials
                .ssh_password
                .as_deref()
                .is_some_and(|value| !value.is_empty() && value.len() < 3)
            || credentials
                .ssh_private_key
                .as_deref()
                .is_some_and(|value| !value.is_empty() && value.len() < 3)
        {
            return Err(AppError::InvalidConfig(
                "AuthKey、MQTT密码和SSH凭据至少需要3个字节，以确保日志可安全脱敏".into(),
            ));
        }
        validate_remote_root(&values.aio_data_root, "一体机数据目录")?;
        validate_remote_root(&values.aio_deploy_root, "一体机部署目录")?;
        let env_names = validate_env_template(&values.env_template)?;
        let placeholder_count = validate_placeholders(&values.env_template)?;
        validate_compose(&values.compose_template, &env_names)?;
        Ok(ReleaseProfileValidation {
            valid: true,
            recognized_placeholder_count: placeholder_count,
            warnings: Vec::new(),
        })
    }
}

fn validate_remote_root(value: &str, label: &str) -> AppResult<()> {
    let value = value.trim();
    if !value.starts_with('/') || value == "/" || value.split('/').any(|segment| segment == "..") {
        return Err(AppError::InvalidConfig(format!(
            "{label}必须是非根目录的受控绝对路径"
        )));
    }
    Ok(())
}

fn validate_env_template(content: &str) -> AppResult<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for (index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _)) = line.split_once('=') else {
            return Err(AppError::InvalidConfig(format!(
                ".env模板第{}行缺少等号",
                index + 1
            )));
        };
        let name = name.trim();
        let mut bytes = name.bytes();
        if !matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
            || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(AppError::InvalidConfig(format!(
                ".env模板第{}行变量名无效",
                index + 1
            )));
        }
        names.insert(name.to_string());
    }
    Ok(names)
}

fn validate_placeholders(content: &str) -> AppResult<usize> {
    let pattern = Regex::new(r"\{\{([^{}]+)\}\}").expect("static placeholder regex");
    let allowed = BTreeSet::from([
        "release.version",
        "platform.host",
        "platform.apiPort",
        "platform.username",
        "platform.password",
        "mqtt.platformHost",
        "mqtt.platformPort",
        "mqtt.platformUser",
        "mqtt.platformPassword",
        "mqtt.localUser",
        "mqtt.localPassword",
        "authKey",
        "platform.authKey",
        "node.name",
        "node.ip",
        "node.mac",
        "node.buildingId",
        "node.regionId",
        "node.addrAlias",
        "node.floor",
        "node.location",
        "node.remark",
        "project.platform_host",
        "project.platform_api",
        "project.auth_key",
        "mqtt.platform_host",
        "mqtt.platform_port",
        "image.device_edge",
        "image.rule_engine",
        ".ReleaseVersion",
        ".PlatformHost",
        ".Node.Name",
        ".Node.IP",
        ".Node.MAC",
    ]);
    let mut unknown = BTreeSet::new();
    let mut count = 0;
    for capture in pattern.captures_iter(content) {
        count += 1;
        let key = capture[1].trim();
        if !allowed.contains(key)
            && !key.starts_with("image.")
            && !key.starts_with("images.")
            && !key.starts_with(".Images.")
        {
            unknown.insert(key.to_string());
        }
    }
    if content.matches("{{").count() != count || content.matches("}}").count() != count {
        return Err(AppError::InvalidConfig(".env模板占位符括号不匹配".into()));
    }
    if !unknown.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "存在未知模板变量：{}",
            unknown.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(count)
}

fn validate_compose(content: &str, env_names: &BTreeSet<String>) -> AppResult<()> {
    if !content.lines().any(|line| line.trim() == "services:") {
        return Err(AppError::InvalidConfig(
            "docker-compose.yml缺少services根节点".into(),
        ));
    }
    let pattern = Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?:(:-|-|:\?|\?)[^}]*)?\}")
        .expect("static compose variable regex");
    let mut missing = BTreeSet::new();
    for capture in pattern.captures_iter(content) {
        if !env_names.contains(&capture[1]) && capture.get(2).is_none() {
            missing.insert(capture[1].to_string());
        }
    }
    if !missing.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "Compose引用了.env未定义变量：{}",
            missing.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(())
}
