use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfigPatch {
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
}
impl AppConfigPatch {
    pub fn fields(&self) -> Vec<String> {
        self.set.keys().chain(self.clear.iter()).cloned().collect()
    }
    pub fn validate(&self) -> AppResult<()> {
        let invalid = || AppError::InvalidConfig("小新配置字段或内容无效，请重新检查".into());
        if self.clear.iter().collect::<BTreeSet<_>>().len() != self.clear.len()
            || self.clear.iter().any(|key| self.set.contains_key(key))
        {
            return Err(invalid());
        }
        let mut environments = BTreeSet::new();
        for field in self.fields() {
            if !supported_field(&field) {
                return Err(invalid());
            }
            let invalid = || AppError::InvalidConfig(format!("{}内容无效，请检查填写格式或选择明确清空", field_label(&field)));
            let value = self.set.get(&field);
            let parts: Vec<_> = field.split('.').collect();
            if parts.len() == 3 {
                environments.insert(parts[1].to_string());
            }
            match value {
                None if field == "customDeviceName"
                    || field.ends_with(".wsUrl")
                    || field.ends_with(".h5Url") => {}
                None => return Err(invalid()),
                Some(v) if field == "environments.current" => {
                    let env = v
                        .as_str()
                        .filter(|s| ["test", "pre", "prod"].contains(s))
                        .ok_or_else(invalid)?;
                    environments.insert(env.into());
                }
                Some(v) if field.ends_with(".h5ReadyCheckEnabled") => {
                    if !v.is_boolean() {
                        return Err(invalid());
                    }
                }
                Some(v) if field == "customDeviceName" => {
                    let name = v.as_str().ok_or_else(invalid)?;
                    if name.trim().is_empty()
                        || name.chars().count() > 128
                        || name.chars().any(char::is_control)
                    {
                        return Err(invalid());
                    }
                }
                Some(v) => {
                    let text = v.as_str().ok_or_else(invalid)?;
                    let uri = reqwest::Url::parse(text.trim()).map_err(|_| invalid())?;
                    let protocols = if field.ends_with(".wsUrl") {
                        &["ws", "wss"][..]
                    } else {
                        &["http", "https"][..]
                    };
                    if text.len() > 4096
                        || text.chars().any(char::is_whitespace)
                        || uri.host_str().is_none()
                        || !protocols.contains(&uri.scheme())
                    {
                        return Err(invalid());
                    }
                }
            }
        }
        if environments.len() > 1
            || serde_json::to_vec(self).map_err(|_| invalid())?.len() > 15 * 1024
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub fn matches(&self, config: &Value) -> bool {
        self.set
            .iter()
            .all(|(key, value)| field_value(config, key) == Some(value))
            && self
                .clear
                .iter()
                .all(|key| field_value(config, key).is_some_and(Value::is_null))
    }
    pub fn restart_required(&self, before: &Value) -> bool {
        let current = before["environments"]["current"].as_str().unwrap_or("");
        self.fields().iter().any(|field| {
            let value = self.set.get(field).unwrap_or(&Value::Null);
            field_value(before, field) != Some(value)
                && (field == "environments.current"
                    || field.starts_with(&format!("environments.{current}.")))
        })
    }
}
pub fn supported_field(field: &str) -> bool {
    if ["customDeviceName", "environments.current"].contains(&field) {
        return true;
    }
    let p: Vec<_> = field.split('.').collect();
    p.len() == 3
        && p[0] == "environments"
        && ["test", "pre", "prod"].contains(&p[1])
        && ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"].contains(&p[2])
}
fn field_label(field: &str) -> &str {
    match field.rsplit('.').next().unwrap_or(field) {
        "customDeviceName" => "小新自定义名称",
        "current" => "运行环境",
        "otaUrl" => "授权／配置服务地址",
        "wsUrl" => "手动语音地址",
        "h5Url" => "手动网页地址",
        "h5ReadyCheckEnabled" => "网页启动检查",
        _ => "小新配置",
    }
}
pub fn field_value<'a>(config: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(config, |value, key| value.get(key))
}

/// 共享结果仅保留名称、运行环境和开关的前后值；地址可能含凭据，不共享地址值。
pub fn shared_changes(evidence: &Value) -> Vec<Value> {
    evidence["fields"].as_array().into_iter().flatten().filter_map(|item| {
        let field=item.as_str()?;
        if field!="customDeviceName" && field!="environments.current" && !field.ends_with(".h5ReadyCheckEnabled") {return None;}
        let before=field_value(&evidence["before"],field)?;
        let after=field_value(&evidence["after"],field)?;
        if before==after || !(before.is_null()||before.is_string()||before.is_boolean()) || !(after.is_null()||after.is_string()||after.is_boolean()) {return None;}
        Some(serde_json::json!({"field":field,"before":before,"after":after}))
    }).collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigRead {
    pub screen_id: String,
    pub read_at: String,
    pub config: Option<Value>,
    pub capabilities: Option<Value>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn whitelist_one_environment_and_explicit_clear() {
        for input in [
            json!({"set":{"authCode":"x"}}),
            json!({"clear":["environments.test.otaUrl"]}),
            json!({"set":{"environments.test.wsUrl":"https://example.org"}}),
            json!({"set":{"environments.current":"pre","environments.test.h5ReadyCheckEnabled":true}}),
            json!({"set":{"customDeviceName":"x"},"clear":["customDeviceName"]}),
        ] {
            assert!(
                serde_json::from_value::<AppConfigPatch>(input)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
        let patch: AppConfigPatch = serde_json::from_value(
            json!({"set":{"environments.current":"test"},"clear":["environments.test.wsUrl"]}),
        )
        .unwrap();
        assert!(patch.validate().is_ok());
    }
    #[test]
    fn compare_only_explicit_fields_and_restart_only_active_changes() {
        let before = json!({"customDeviceName":"one","environments":{"current":"pre","test":{"wsUrl":null},"pre":{"wsUrl":"wss://example.org"}}});
        let name: AppConfigPatch =
            serde_json::from_value(json!({"set":{"customDeviceName":"two"}})).unwrap();
        assert!(!name.restart_required(&before));
        let clear: AppConfigPatch =
            serde_json::from_value(json!({"clear":["environments.test.wsUrl"]})).unwrap();
        assert!(clear.matches(&before));
        assert!(!clear.restart_required(&before));
        let active: AppConfigPatch =
            serde_json::from_value(json!({"clear":["environments.pre.wsUrl"]})).unwrap();
        assert!(active.restart_required(&before));
    }
    #[test]
    fn shared_changes_exclude_all_address_values() {
        let evidence=json!({"fields":["customDeviceName","environments.pre.wsUrl"],"before":{"customDeviceName":"旧名称","environments":{"pre":{"wsUrl":"wss://private.example"}}},"after":{"customDeviceName":"新名称","environments":{"pre":{"wsUrl":"wss://another.example"}}}});
        let changes=shared_changes(&evidence);
        assert_eq!(changes,vec![json!({"field":"customDeviceName","before":"旧名称","after":"新名称"})]);
    }
}
