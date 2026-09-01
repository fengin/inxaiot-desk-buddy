use std::collections::{BTreeMap, BTreeSet};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::release_render::{ReleaseRenderContext, RenderedReleaseFiles};
use regex::Regex;

pub fn render_release_templates(
    env_template: &str,
    host_info_template: &str,
    compose_template: &str,
    context: &ReleaseRenderContext,
) -> AppResult<RenderedReleaseFiles> {
    let values = template_values(context);
    let mut env = render_placeholders(env_template, &values, false)?;
    if !env.ends_with('\n') {
        env.push('\n');
    }
    validate_env(&env)?;
    let host_info_json = render_placeholders(host_info_template, &values, true)?;
    serde_json::from_str::<serde_json::Value>(&host_info_json)
        .map_err(|_| AppError::InvalidConfig("渲染后的host-info不是有效JSON".into()))?;
    let compose_preview = render_compose(compose_template, &parse_env(&env))?;
    Ok(RenderedReleaseFiles {
        env,
        host_info_json,
        compose_preview,
    })
}

fn render_placeholders(
    template: &str,
    values: &BTreeMap<String, String>,
    escape_json: bool,
) -> AppResult<String> {
    let pattern = Regex::new(r"\{\{([^{}]+)\}\}").expect("static placeholder regex");
    let mut unknown = BTreeSet::new();
    let mut unsafe_values = BTreeSet::new();
    let rendered = pattern.replace_all(template, |captures: &regex::Captures<'_>| {
        let key = captures[1].trim();
        match values.get(key) {
            Some(value) if escape_json => serde_json::to_string(value)
                .unwrap_or_else(|_| "\"\"".into())
                .trim_matches('"')
                .to_string(),
            Some(value) if value.contains('\r') || value.contains('\n') || value.contains('\0') => {
                unsafe_values.insert(key.to_string());
                captures[0].to_string()
            }
            Some(value) => value.clone(),
            None => {
                unknown.insert(key.to_string());
                captures[0].to_string()
            }
        }
    });
    if !unknown.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "存在未知模板变量：{}",
            unknown.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    if !unsafe_values.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "模板变量包含换行或空字符：{}",
            unsafe_values.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(rendered.into_owned())
}

fn validate_env(content: &str) -> AppResult<()> {
    for (index, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, _value)) = line.split_once('=') else {
            return Err(AppError::InvalidConfig(format!(
                ".env第{}行缺少等号",
                index + 1
            )));
        };
        if !valid_env_name(name.trim()) {
            return Err(AppError::InvalidConfig(format!(
                ".env第{}行变量名无效",
                index + 1
            )));
        }
    }
    Ok(())
}

fn valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn parse_env(content: &str) -> BTreeMap<String, String> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            line.split_once('=')
                .map(|(name, value)| (name.trim().to_string(), value.to_string()))
        })
        .collect()
}

fn render_compose(template: &str, env: &BTreeMap<String, String>) -> AppResult<String> {
    let pattern = Regex::new(r"\$\{([^}]+)\}").expect("static compose regex");
    let mut missing = BTreeSet::new();
    let rendered = pattern.replace_all(template, |captures: &regex::Captures<'_>| {
        let expression = &captures[1];
        let (name, fallback, default_on_empty) = parse_compose_expression(expression);
        if let Some(value) = env.get(name)
            && (!default_on_empty || !value.is_empty())
        {
            return value.clone();
        }
        if let Some(fallback) = fallback {
            return fallback.to_string();
        }
        missing.insert(name.to_string());
        captures[0].to_string()
    });
    if !missing.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "渲染后的Compose仍有未定义变量：{}",
            missing.into_iter().collect::<Vec<_>>().join("、")
        )));
    }
    Ok(rendered.into_owned())
}

fn parse_compose_expression(expression: &str) -> (&str, Option<&str>, bool) {
    if let Some((name, fallback)) = expression.split_once(":-") {
        return (name, Some(fallback), true);
    }
    if let Some((name, fallback)) = expression.split_once('-') {
        return (name, Some(fallback), false);
    }
    if let Some((name, _message)) = expression.split_once(":?") {
        return (name, None, true);
    }
    if let Some((name, _message)) = expression.split_once('?') {
        return (name, None, false);
    }
    (expression, None, false)
}

fn template_values(context: &ReleaseRenderContext) -> BTreeMap<String, String> {
    let mut values = BTreeMap::from([
        ("release.version".into(), context.release_version.clone()),
        ("platform.host".into(), context.platform_host.clone()),
        (
            "platform.apiPort".into(),
            context.platform_api_port.to_string(),
        ),
        (
            "platform.username".into(),
            context.platform_username.clone(),
        ),
        (
            "platform.password".into(),
            context.platform_password.clone(),
        ),
        (
            "mqtt.platformHost".into(),
            context.platform_mqtt_host.clone(),
        ),
        (
            "mqtt.platformPort".into(),
            context.platform_mqtt_port.to_string(),
        ),
        (
            "mqtt.platformUser".into(),
            context.platform_mqtt_user.clone(),
        ),
        (
            "mqtt.platformPassword".into(),
            context.platform_mqtt_password.clone(),
        ),
        ("mqtt.localUser".into(), context.local_mqtt_user.clone()),
        (
            "mqtt.localPassword".into(),
            context.local_mqtt_password.clone(),
        ),
        ("authKey".into(), context.auth_key.clone()),
        ("node.name".into(), context.node_name.clone()),
        ("node.ip".into(), context.node_ip.clone()),
        ("node.mac".into(), context.node_mac.clone()),
        ("node.buildingId".into(), context.node_building_id.clone()),
        ("node.regionId".into(), context.node_region_id.clone()),
        ("node.addrAlias".into(), context.node_addr_alias.clone()),
        ("node.floor".into(), context.node_floor.clone()),
        ("node.location".into(), context.node_location.clone()),
        ("node.remark".into(), context.node_remark.clone()),
        (".ReleaseVersion".into(), context.release_version.clone()),
        (".PlatformHost".into(), context.platform_host.clone()),
        (".Node.Name".into(), context.node_name.clone()),
        (".Node.IP".into(), context.node_ip.clone()),
        (".Node.MAC".into(), context.node_mac.clone()),
    ]);
    for (service, image) in &context.images {
        values.insert(format!("images.{service}"), image.clone());
        values.insert(format!(".Images.{service}"), image.clone());
    }
    values
}

#[cfg(test)]
mod tests {
    use super::{ReleaseRenderContext, render_release_templates};
    use std::collections::BTreeMap;

    fn context() -> ReleaseRenderContext {
        ReleaseRenderContext {
            release_version: "2026.08.28".into(),
            platform_host: "10.20.1.18".into(),
            platform_api_port: 8055,
            platform_username: "user".into(),
            platform_password: "p\"ass".into(),
            platform_mqtt_host: "10.20.1.19".into(),
            platform_mqtt_port: 1883,
            platform_mqtt_user: "mqtt".into(),
            platform_mqtt_password: "mqtt-pass".into(),
            local_mqtt_user: "local".into(),
            local_mqtt_password: "local-pass".into(),
            auth_key: "key".into(),
            node_name: "AIO-1".into(),
            node_ip: "192.0.2.1".into(),
            node_mac: "00:11:22:33:44:55".into(),
            images: BTreeMap::from([("device-edge".into(), "device-edge:1".into())]),
            ..ReleaseRenderContext::default()
        }
    }

    #[test]
    fn renders_env_json_and_compose_with_defaults() {
        let output = render_release_templates(
            "RELEASE={{release.version}}\nAPP_IMAGE={{images.device-edge}}\nCONFIG_MARKER={{node.name}}\n",
            r#"{"name":"{{node.name}}","password":"{{platform.password}}"}"#,
            "services:\n  app:\n    image: ${APP_IMAGE}\n    labels:\n      config-marker: ${CONFIG_MARKER}\n  fallback:\n    image: ${OTHER:-busybox:latest}\n",
            &context(),
        )
        .expect("render");
        assert!(output.env.contains("APP_IMAGE=device-edge:1"));
        assert!(output.host_info_json.contains(r#"p\"ass"#));
        assert!(output.compose_preview.contains("config-marker: AIO-1"));
        assert!(!output.compose_preview.contains("${"));
    }

    #[test]
    fn rejects_unknown_placeholder_and_unresolved_compose_variable() {
        assert!(
            render_release_templates("A={{unknown}}\n", "{}", "services: {}", &context()).is_err()
        );
        assert!(render_release_templates("A=1\n", "{}", "image: ${MISSING}", &context()).is_err());

        let mut injected = context();
        injected.node_name = "safe\nINJECTED=value".into();
        assert!(
            render_release_templates("NODE_NAME={{node.name}}\n", "{}", "services: {}", &injected)
                .is_err()
        );
    }
}
