use std::collections::{BTreeMap, BTreeSet};

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileValues {
    pub env_template: String,
    pub compose_template: String,
    pub host_info_template: String,
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
#[serde(rename_all = "snake_case")]
pub enum ReleaseAgentScriptSource {
    BuiltIn,
    Project,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAgentScriptView {
    pub file_name: String,
    pub version: String,
    pub protocol_version: String,
    pub sha256: String,
    pub source: ReleaseAgentScriptSource,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseAgentScriptReplaceRequest {
    pub file_path: String,
    pub expected_version: u64,
}

impl ReleaseAgentScriptReplaceRequest {
    pub fn validate(&self) -> AppResult<()> {
        if self.file_path.trim().is_empty() || self.expected_version == 0 {
            return Err(AppError::InvalidConfig("更换一体机脚本参数不完整".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileView {
    pub profile_key: String,
    pub values: ReleaseProfileValues,
    pub credentials: ReleaseProfileCredentials,
    pub credentials_reset_required: bool,
    pub agent_script: ReleaseAgentScriptView,
    pub compose_services: Vec<ReleaseComposeService>,
    pub version: u64,
    pub updated_by: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseProfileValidation {
    pub valid: bool,
    pub recognized_placeholder_count: usize,
    pub compose_services: Vec<ReleaseComposeService>,
    pub published_ports: Vec<u16>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseComposeService {
    pub name: String,
    pub configured_image: String,
    pub image_environment_variable: String,
}

impl ReleaseProfileDraft {
    pub fn validate(&self) -> AppResult<ReleaseProfileValidation> {
        let values = &self.values;
        let credentials = &self.credentials;
        let mut fields = BTreeMap::new();
        for (field, value, label) in [
            (
                "values.envTemplate",
                values.env_template.trim(),
                ".env 模板",
            ),
            (
                "values.composeTemplate",
                values.compose_template.trim(),
                "docker-compose.yml 模板",
            ),
            (
                "values.hostInfoTemplate",
                values.host_info_template.trim(),
                "host-info.json 模板",
            ),
            (
                "values.platformHost",
                values.platform_host.trim(),
                "平台主机",
            ),
            (
                "values.platformMqttHost",
                values.platform_mqtt_host.trim(),
                "平台 MQTT 主机",
            ),
            (
                "credentials.platformAuthKey",
                credentials.platform_auth_key.trim(),
                "平台 API AuthKey",
            ),
            (
                "credentials.platformMqttUser",
                credentials.platform_mqtt_user.trim(),
                "平台 MQTT 账号",
            ),
            (
                "credentials.platformMqttPassword",
                credentials.platform_mqtt_password.as_str(),
                "平台 MQTT 密码",
            ),
            (
                "credentials.aioMqttUser",
                credentials.aio_mqtt_user.trim(),
                "一体机 MQTT 账号",
            ),
            (
                "credentials.aioMqttPassword",
                credentials.aio_mqtt_password.as_str(),
                "一体机 MQTT 密码",
            ),
            (
                "credentials.sshUser",
                credentials.ssh_user.trim(),
                "SSH 用户名",
            ),
        ] {
            if value.is_empty() {
                fields.insert(field.into(), format!("请填写{label}"));
            }
        }
        for (field, port) in [
            ("values.platformApiPort", values.platform_api_port),
            ("values.platformMqttPort", values.platform_mqtt_port),
            ("values.sshPort", values.ssh_port),
        ] {
            if port == 0 {
                fields.insert(field.into(), "端口必须是 1～65535 的整数".into());
            }
        }
        if !(1..=3600).contains(&values.ssh_timeout_seconds) {
            fields.insert(
                "values.sshTimeoutSeconds".into(),
                "连接超时必须是 1～3600 秒的整数".into(),
            );
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
            for field in ["credentials.sshPassword", "credentials.sshPrivateKey"] {
                fields.insert(field.into(), "SSH 密码和私钥至少填写一项".into());
            }
        }
        for (field, value) in [
            (
                "credentials.platformAuthKey",
                credentials.platform_auth_key.as_str(),
            ),
            (
                "credentials.platformMqttPassword",
                credentials.platform_mqtt_password.as_str(),
            ),
            (
                "credentials.aioMqttPassword",
                credentials.aio_mqtt_password.as_str(),
            ),
            (
                "credentials.sshPassword",
                credentials.ssh_password.as_deref().unwrap_or_default(),
            ),
            (
                "credentials.sshPrivateKey",
                credentials.ssh_private_key.as_deref().unwrap_or_default(),
            ),
        ] {
            if !value.is_empty() && value.len() < 3 {
                fields
                    .entry(field.into())
                    .or_insert_with(|| "至少需要 3 个字节".into());
            }
        }
        for (field, value, label) in [
            (
                "values.aioDataRoot",
                values.aio_data_root.as_str(),
                "数据根目录",
            ),
            (
                "values.aioDeployRoot",
                values.aio_deploy_root.as_str(),
                "部署根目录",
            ),
        ] {
            if let Err(AppError::InvalidConfig(message)) = validate_remote_root(value, label) {
                fields.insert(field.into(), message);
            }
        }
        let mut placeholder_count = 0;
        let mut compose_services = Vec::new();
        if !values.env_template.trim().is_empty() {
            let env_result = validate_env_template(&values.env_template).and_then(|names| {
                validate_placeholders(&values.env_template, ".env模板").map(|count| (names, count))
            });
            match env_result {
                Ok((names, count)) => {
                    placeholder_count = count;
                    if !values.compose_template.trim().is_empty() {
                        match validate_compose(&values.compose_template, &names) {
                            Ok(services) => compose_services = services,
                            Err(AppError::InvalidConfig(message)) => {
                                fields.insert("values.composeTemplate".into(), message);
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }
                Err(AppError::InvalidConfig(message)) => {
                    fields.insert("values.envTemplate".into(), message);
                }
                Err(error) => return Err(error),
            }
        }
        let mut published_ports = Vec::new();
        if !values.compose_template.trim().is_empty() && !values.env_template.trim().is_empty() {
            match inspect_compose_published_ports(&values.compose_template, &values.env_template) {
                Ok(ports) => published_ports = ports,
                Err(AppError::InvalidConfig(message)) => {
                    fields.insert("values.composeTemplate".into(), message);
                }
                Err(error) => return Err(error),
            }
            if !compose_services.is_empty()
                && let Err(AppError::InvalidConfig(message)) =
                    validate_image_placeholders(&values.env_template, &compose_services)
            {
                fields.insert("values.envTemplate".into(), message);
            }
        }
        if !values.host_info_template.trim().is_empty() {
            match validate_host_info_template(&values.host_info_template) {
                Ok(count) => placeholder_count += count,
                Err(AppError::InvalidConfig(message)) => {
                    fields.insert("values.hostInfoTemplate".into(), message);
                }
                Err(error) => return Err(error),
            }
        }
        if !fields.is_empty() {
            return Err(AppError::InvalidFields(fields));
        }
        Ok(ReleaseProfileValidation {
            valid: true,
            recognized_placeholder_count: placeholder_count,
            compose_services,
            published_ports,
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
        if !names.insert(name.to_string()) {
            return Err(AppError::InvalidConfig(format!(
                ".env模板第{}行变量重复：{name}",
                index + 1
            )));
        }
    }
    Ok(names)
}

fn validate_placeholders(content: &str, label: &str) -> AppResult<usize> {
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
        return Err(AppError::InvalidConfig(format!("{label}占位符括号不匹配")));
    }
    if !unknown.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "存在未知模板变量：{}",
            unknown.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(count)
}

fn validate_compose(
    content: &str,
    env_names: &BTreeSet<String>,
) -> AppResult<Vec<ReleaseComposeService>> {
    let services = inspect_compose_services(content)?;
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
    Ok(services)
}

pub fn inspect_compose_services(content: &str) -> AppResult<Vec<ReleaseComposeService>> {
    let document: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|error| AppError::InvalidConfig(format!("docker-compose.yml格式无效：{error}")))?;
    let root = document
        .as_mapping()
        .ok_or_else(|| AppError::InvalidConfig("docker-compose.yml根节点必须是对象".into()))?;
    let services = root
        .get(serde_yaml::Value::String("services".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| AppError::InvalidConfig("docker-compose.yml缺少services根节点".into()))?;
    if services.is_empty() {
        return Err(AppError::InvalidConfig(
            "docker-compose.yml至少需要配置一个服务".into(),
        ));
    }
    let service_name =
        Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]*$").expect("static compose service regex");
    let image_variable = Regex::new(r"^\$\{([A-Za-z_][A-Za-z0-9_]*)\}$")
        .expect("static compose image variable regex");
    let mut result = Vec::with_capacity(services.len());
    let mut image_variable_owners = BTreeMap::new();
    for (name, config) in services {
        let name = name.as_str().ok_or_else(|| {
            AppError::InvalidConfig("docker-compose.yml服务名必须是字符串".into())
        })?;
        if !service_name.is_match(name) {
            return Err(AppError::InvalidConfig(format!(
                "Compose服务名不安全：{name}"
            )));
        }
        let config = config.as_mapping().ok_or_else(|| {
            AppError::InvalidConfig(format!("Compose服务 {name} 的配置必须是对象"))
        })?;
        let image = config
            .get(serde_yaml::Value::String("image".into()))
            .and_then(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                AppError::InvalidConfig(format!(
                    "Compose服务 {name} 缺少可用的image配置，当前工作台不支持仅build的服务"
                ))
            })?;
        let variable = image_variable
            .captures(image)
            .and_then(|capture| capture.get(1))
            .map(|value| value.as_str().to_string())
            .ok_or_else(|| {
                AppError::InvalidConfig(format!(
                    "Compose服务 {name} 的image必须使用${{变量名}}引用.env，部署时才能安全替换镜像"
                ))
            })?;
        if let Some(existing_service) = image_variable_owners.insert(variable.clone(), name) {
            return Err(AppError::InvalidConfig(format!(
                "Compose服务 {existing_service} 和 {name} 共用了镜像变量 {variable}；每个服务必须使用独立的镜像变量"
            )));
        }
        result.push(ReleaseComposeService {
            name: name.to_string(),
            configured_image: image.to_string(),
            image_environment_variable: variable,
        });
    }
    Ok(result)
}

pub fn inspect_compose_published_ports(content: &str, env_template: &str) -> AppResult<Vec<u16>> {
    let document: serde_yaml::Value = serde_yaml::from_str(content)
        .map_err(|error| AppError::InvalidConfig(format!("docker-compose.yml格式无效：{error}")))?;
    let services = document
        .as_mapping()
        .and_then(|root| root.get(serde_yaml::Value::String("services".into())))
        .and_then(serde_yaml::Value::as_mapping)
        .ok_or_else(|| AppError::InvalidConfig("docker-compose.yml缺少services根节点".into()))?;
    let environment = env_template
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                None
            } else {
                line.split_once('=')
                    .map(|(name, value)| (name.trim().to_string(), value.trim().to_string()))
            }
        })
        .collect::<BTreeMap<_, _>>();
    let variable =
        Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}").expect("static compose port variable regex");
    let mut result = BTreeSet::new();
    for (service, config) in services {
        let service = service.as_str().unwrap_or("未知服务");
        let Some(ports) = config
            .as_mapping()
            .and_then(|mapping| mapping.get(serde_yaml::Value::String("ports".into())))
        else {
            continue;
        };
        let ports = ports.as_sequence().ok_or_else(|| {
            AppError::InvalidConfig(format!("Compose服务 {service} 的ports必须是数组"))
        })?;
        for port in ports {
            let published = if let Some(value) = port.as_str() {
                let expanded = variable.replace_all(value, |captures: &regex::Captures<'_>| {
                    environment.get(&captures[1]).cloned().unwrap_or_default()
                });
                if expanded.contains("{{") || expanded.contains("${") {
                    return Err(AppError::InvalidConfig(format!(
                        "Compose服务 {service} 的宿主机端口无法从.env确定：{value}"
                    )));
                }
                let value = expanded.split('/').next().unwrap_or_default();
                let parts = value.rsplit(':').collect::<Vec<_>>();
                if parts.len() < 2 {
                    None
                } else {
                    Some(parts[1].to_string())
                }
            } else if let Some(mapping) = port.as_mapping() {
                mapping
                    .get(serde_yaml::Value::String("published".into()))
                    .and_then(|value| match value {
                        serde_yaml::Value::Number(number) => Some(number.to_string()),
                        serde_yaml::Value::String(value) => Some(value.clone()),
                        _ => None,
                    })
            } else {
                return Err(AppError::InvalidConfig(format!(
                    "Compose服务 {service} 包含不支持的ports配置"
                )));
            };
            if let Some(published) = published {
                let published = variable
                    .replace_all(&published, |captures: &regex::Captures<'_>| {
                        environment.get(&captures[1]).cloned().unwrap_or_default()
                    })
                    .into_owned();
                if published.contains("{{") || published.contains("${") {
                    return Err(AppError::InvalidConfig(format!(
                        "Compose服务 {service} 的宿主机端口无法从.env确定：{published}"
                    )));
                }
                let published = published.trim_matches(['"', '\'']);
                let port = published.parse::<u16>().map_err(|_| {
                    AppError::InvalidConfig(format!(
                        "Compose服务 {service} 的宿主机端口无效：{published}"
                    ))
                })?;
                if port == 0 {
                    return Err(AppError::InvalidConfig(format!(
                        "Compose服务 {service} 的宿主机端口不能为0"
                    )));
                }
                result.insert(port);
            }
        }
    }
    Ok(result.into_iter().collect())
}

fn validate_host_info_template(content: &str) -> AppResult<usize> {
    let count = validate_placeholders(content, "host-info.json模板")?;
    let source: serde_json::Value = serde_json::from_str(content)
        .map_err(|error| AppError::InvalidConfig(format!("host-info.json模板格式无效：{error}")))?;
    let source = source
        .as_object()
        .ok_or_else(|| AppError::InvalidConfig("host-info.json模板根节点必须是JSON对象".into()))?;
    for (field, placeholders) in [
        ("mac", &["{{ node.mac }}", "{{node.mac}}"] as &[&str]),
        ("ip", &["{{ node.ip }}", "{{node.ip}}"]),
        ("hostname", &["{{ node.name }}", "{{node.name}}"]),
        (
            "authKey",
            &[
                "{{ authKey }}",
                "{{authKey}}",
                "{{ platform.authKey }}",
                "{{platform.authKey}}",
            ],
        ),
    ] {
        let value = source
            .get(field)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !placeholders
            .iter()
            .any(|placeholder| value.contains(placeholder))
        {
            return Err(AppError::InvalidConfig(format!(
                "host-info.json模板字段 {field} 必须引用对应的一体机或项目变量"
            )));
        }
    }
    let pattern = Regex::new(r"\{\{([^{}]+)\}\}").expect("static placeholder regex");
    let rendered = pattern.replace_all(content, "sample");
    let document: serde_json::Value = serde_json::from_str(&rendered)
        .map_err(|error| AppError::InvalidConfig(format!("host-info.json模板格式无效：{error}")))?;
    let object = document
        .as_object()
        .ok_or_else(|| AppError::InvalidConfig("host-info.json模板根节点必须是JSON对象".into()))?;
    for field in ["mac", "ip", "hostname", "authKey"] {
        if !object.get(field).is_some_and(serde_json::Value::is_string) {
            return Err(AppError::InvalidConfig(format!(
                "host-info.json模板缺少字符串字段：{field}"
            )));
        }
    }
    Ok(count)
}

fn validate_image_placeholders(content: &str, services: &[ReleaseComposeService]) -> AppResult<()> {
    let configured = services
        .iter()
        .map(|service| service.name.as_str())
        .collect::<BTreeSet<_>>();
    let pattern = Regex::new(r"\{\{\s*(?:images|image|\.Images)\.([^{}\s]+)\s*\}\}")
        .expect("static image placeholder regex");
    let unknown = pattern
        .captures_iter(content)
        .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
        .filter(|service| !configured.contains(service))
        .collect::<BTreeSet<_>>();
    if !unknown.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            ".env模板引用了Compose中不存在的镜像服务：{}",
            unknown.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        ReleaseAgentScriptSource, ReleaseAgentScriptView, ReleaseComposeService,
        ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues, ReleaseProfileView,
    };

    fn valid_draft() -> ReleaseProfileDraft {
        ReleaseProfileDraft {
            values: ReleaseProfileValues {
                env_template: "APP_IMAGE={{ images.app }}\nAPP_PORT=8080\n".into(),
                compose_template:
                    "services:\n  app:\n    image: ${APP_IMAGE}\n    ports:\n      - ${APP_PORT}:80\n"
                        .into(),
                host_info_template: r#"{"mac":"{{ node.mac }}","ip":"{{ node.ip }}","hostname":"{{ node.name }}","authKey":"{{ authKey }}"}"#.into(),
                platform_host: "platform.test".into(),
                platform_api_port: 8055,
                platform_mqtt_host: "mqtt.test".into(),
                platform_mqtt_port: 1883,
                ssh_port: 22,
                ssh_timeout_seconds: 15,
                aio_data_root: "/opt/data".into(),
                aio_deploy_root: "/opt/data/deploy".into(),
            },
            credentials: ReleaseProfileCredentials {
                platform_auth_key: "auth".into(),
                platform_mqtt_user: "mqtt".into(),
                platform_mqtt_password: "password".into(),
                aio_mqtt_user: "local".into(),
                aio_mqtt_password: "password".into(),
                ssh_user: "root".into(),
                ssh_password: Some("password".into()),
                ssh_private_key: None,
            },
            expected_version: None,
        }
    }

    #[test]
    fn built_in_templates_pass_real_release_validation() {
        let mut draft = valid_draft();
        draft.values.env_template = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/shared/templates/aio-default.env"
        ))
        .into();
        draft.values.compose_template = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../src/shared/templates/aio-default-compose.yml"
        ))
        .into();
        let result = draft.validate().expect("内置三模板应通过正式校验");
        let services = result
            .compose_services
            .iter()
            .map(|service| service.name.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            services,
            ["emqx", "device-edge", "rule-engine", "device-edge-web"]
                .into_iter()
                .collect()
        );
        assert!(result.published_ports.contains(&6001));
        assert!(result.published_ports.contains(&6002));
        assert!(result.published_ports.contains(&7000));
    }

    #[test]
    fn validation_extracts_compose_services_and_ports_before_save() {
        let result = valid_draft().validate().expect("valid profile");
        assert_eq!(result.compose_services[0].name, "app");
        assert_eq!(
            result.compose_services[0].image_environment_variable,
            "APP_IMAGE"
        );
        assert_eq!(result.published_ports, vec![8080]);
    }

    #[test]
    fn validation_rejects_unreplaceable_images_and_incomplete_host_info() {
        let mut draft = valid_draft();
        draft.values.compose_template = "services:\n  app:\n    image: app:1\n".into();
        assert!(draft.validate().is_err());
        let mut draft = valid_draft();
        draft.values.host_info_template = r#"{"mac":"{{ node.mac }}"}"#.into();
        assert!(draft.validate().is_err());
        let mut draft = valid_draft();
        draft
            .values
            .env_template
            .push_str("UNKNOWN={{ images.missing }}\n");
        assert!(draft.validate().is_err());
        let mut draft = valid_draft();
        draft.values.env_template.push_str("APP_PORT=9090\n");
        assert!(draft.validate().is_err());
        let mut draft = valid_draft();
        draft.values.compose_template = concat!(
            "services:\n",
            "  app:\n    image: ${APP_IMAGE}\n",
            "  worker:\n    image: ${APP_IMAGE}\n"
        )
        .into();
        let error = super::inspect_compose_services(&draft.values.compose_template)
            .expect_err("shared image variable must be rejected");
        assert!(error.to_string().contains("共用了镜像变量 APP_IMAGE"));
    }

    #[test]
    fn credential_reset_requirement_uses_the_ipc_camel_case_field() {
        let view = ReleaseProfileView {
            profile_key: "default".into(),
            values: ReleaseProfileValues {
                env_template: "APP_IMAGE=app:1".into(),
                compose_template: "services:\n  app:\n    image: ${APP_IMAGE}".into(),
                host_info_template: r#"{"mac":"{{ node.mac }}","ip":"{{ node.ip }}","hostname":"{{ node.name }}","authKey":"{{ authKey }}"}"#.into(),
                platform_host: "platform.test".into(),
                platform_api_port: 8055,
                platform_mqtt_host: "mqtt.test".into(),
                platform_mqtt_port: 1883,
                ssh_port: 22,
                ssh_timeout_seconds: 15,
                aio_data_root: "/opt/data".into(),
                aio_deploy_root: "/opt/data/deploy".into(),
            },
            credentials: ReleaseProfileCredentials {
                platform_auth_key: String::new(),
                platform_mqtt_user: String::new(),
                platform_mqtt_password: String::new(),
                aio_mqtt_user: String::new(),
                aio_mqtt_password: String::new(),
                ssh_user: String::new(),
                ssh_password: None,
                ssh_private_key: None,
            },
            credentials_reset_required: true,
            agent_script: ReleaseAgentScriptView {
                file_name: "edge-node-agent.sh".into(),
                version: "0.1.9".into(),
                protocol_version: "1".into(),
                sha256: "a".repeat(64),
                source: ReleaseAgentScriptSource::BuiltIn,
            },
            compose_services: vec![ReleaseComposeService {
                name: "app".into(),
                configured_image: "${APP_IMAGE}".into(),
                image_environment_variable: "APP_IMAGE".into(),
            }],
            version: 1,
            updated_by: "operator".into(),
            updated_at: "2026-09-04T00:00:00Z".into(),
        };

        let value = serde_json::to_value(view).expect("serialize release profile view");
        assert_eq!(value["credentialsResetRequired"], true);
        assert_eq!(value["agentScript"]["source"], "built_in");
        assert!(value.get("credentials_reset_required").is_none());
    }
}
