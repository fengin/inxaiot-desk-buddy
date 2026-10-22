use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use super::app_config_schema::{self as schema, ConfigFieldDefinition, ConfigSchema, FieldType, values_equal};

/// 配置与安装一样按明确尺寸分批；与应用版本和单个字段能力相互独立。
pub fn validate_batch_sizes<'a>(sizes: impl IntoIterator<Item = &'a str>) -> AppResult<()> {
    let sizes = sizes.into_iter().collect::<BTreeSet<_>>();
    if sizes.is_empty() || sizes.iter().any(|size| !["4", "10"].contains(size)) {
        return Err(AppError::InvalidConfig("请先确认所选屏尺寸".into()));
    }
    if sizes.len() != 1 {
        return Err(AppError::InvalidConfig("4 寸屏和 10 寸屏不能同时修改配置，请分批选择。".into()));
    }
    Ok(())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppConfigPatch {
    #[serde(default)]
    pub set: BTreeMap<String, Value>,
    #[serde(default)]
    pub clear: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_enabled: Option<bool>,
}
impl AppConfigPatch {
    pub fn is_empty(&self) -> bool {
        self.set.is_empty() && self.clear.is_empty() && self.home_enabled.is_none()
    }
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
            let invalid = || {
                AppError::InvalidConfig(format!(
                    "{}内容无效，请检查填写格式或选择明确清空",
                    field_label(&field)
                ))
            };
            let value = self.set.get(&field);
            let parts: Vec<_> = field.split('.').collect();
            if parts.len() == 3 && parts[0] == "environments" {
                environments.insert(parts[1].to_string());
            }
            match value {
                Some(v) if schema::audio_field(&field) => {
                    // 类型、范围、选项和权限必须在实际屏重新读取schema后复核。
                    if !(v.is_boolean() || v.as_f64().is_some_and(f64::is_finite)
                        || v.as_str().is_some_and(|s| !s.trim().is_empty() && s.chars().count() <= 4096 && !s.chars().any(char::is_control))) {
                        return Err(invalid());
                    }
                }
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
                Some(v) if field == "autoStartVoiceEngine" || field.ends_with(".h5ReadyCheckEnabled") => {
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
            .all(|(key, value)| field_value(config, key).is_some_and(|before| values_equal(before, value)))
            && self
                .clear
                .iter()
                .all(|key| field_value(config, key).is_some_and(Value::is_null))
    }
    pub fn restart_required(&self, before: &Value) -> bool {
        let current = before["environments"]["current"].as_str().unwrap_or("");
        self.fields().iter().any(|field| {
            let value = self.set.get(field).unwrap_or(&Value::Null);
            !field_value(before, field).is_some_and(|before| values_equal(before, value))
                && (field == "environments.current"
                    || field == "autoStartVoiceEngine"
                    || field.starts_with(&format!("environments.{current}.")))
        })
    }
    pub fn restart_required_with_capabilities(&self, before: &Value, cap: &Value) -> bool {
        match ConfigSchema::from_capabilities(cap) {
            Ok(Some(schema)) => schema.restart_required(self, before),
            _ => self.restart_required(before),
        }
    }
}
pub fn supported_field(field: &str) -> bool {
    schema::safe_field_path(field)
}
fn field_label(field: &str) -> &str {
    match field.rsplit('.').next().unwrap_or(field) {
        "customDeviceName" => "小新自定义名称",
        "autoStartVoiceEngine" => "语音引擎随应用启动",
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

/// 共享结果保留旧安全字段及schema确认的语音数值/布尔值，不共享地址和私有字段。
pub fn shared_changes(evidence: &Value) -> Vec<Value> {
    evidence["fields"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let field = item.as_str()?;
            if schema::audio_field(field) {
                let definition: ConfigFieldDefinition = evidence["fieldDefinitions"].as_array()?.iter()
                    .find(|definition| definition["key"] == field)
                    .and_then(|value| serde_json::from_value(value.clone()).ok())?;
                if !definition.shape_valid() || !matches!(definition.field_type, FieldType::Boolean | FieldType::Integer | FieldType::Number) { return None; }
                let before = field_value(&evidence["before"], field)?;
                let after = field_value(&evidence["after"], field)?;
                if before.is_null() || after.is_null() || !definition.readable_value(before) || !definition.readable_value(after) || values_equal(before, after) { return None; }
                let mut change = serde_json::json!({"field":field,"before":before,"after":after});
                if safe_shared_label(&definition.label) { change["label"] = serde_json::json!(definition.label); }
                if let Some(unit) = definition.unit.filter(|unit| safe_shared_label(unit)) { change["unit"] = serde_json::json!(unit); }
                return Some(change);
            }
            if field != "customDeviceName"
                && field != "environments.current"
                && field != "autoStartVoiceEngine"
                && !field.ends_with(".h5ReadyCheckEnabled")
            {
                return None;
            }
            let before = field_value(&evidence["before"], field)?;
            let after = field_value(&evidence["after"], field)?;
            if field == "autoStartVoiceEngine" && (!before.is_boolean() || !after.is_boolean()) {
                return None;
            }
            if values_equal(before, after)
                || !(before.is_null() || before.is_string() || before.is_boolean())
                || !(after.is_null() || after.is_string() || after.is_boolean())
            {
                return None;
            }
            Some(serde_json::json!({"field":field,"before":before,"after":after}))
        })
        .collect()
}

/// 失败或同值结果也能显示语音参数名称；只共享定义中的短标签，不复制默认值或地址。
pub fn shared_field_labels(evidence: &Value) -> BTreeMap<String, String> {
    evidence["fieldDefinitions"].as_array().into_iter().flatten().take(64)
        .filter_map(|raw| serde_json::from_value::<ConfigFieldDefinition>(raw.clone()).ok())
        .filter(|definition| definition.shape_valid()
            && evidence["fields"].as_array().is_some_and(|fields| fields.contains(&Value::String(definition.key.clone())))
            && safe_shared_label(&definition.label))
        .map(|definition| (definition.key, definition.label)).collect()
}

fn safe_shared_label(label: &str) -> bool {
    !label.contains("://") && !["password=", "token="].iter().any(|key| label.to_ascii_lowercase().contains(key))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfigRead {
    pub screen_id: String,
    pub read_at: String,
    pub config: Option<Value>,
    pub capabilities: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<AppHomeRead>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHomeRead {
    pub supported: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn configuration_batch_accepts_only_one_known_size_independent_of_app_version() {
        for sizes in [vec!["4"], vec!["10"], vec!["4", "4"], vec!["10", "10", "10"]] {
            assert!(validate_batch_sizes(sizes).is_ok());
        }
        for sizes in [vec![], vec!["unknown"], vec![""], vec!["7"], vec!["4", "unknown"], vec!["10", "unknown"]] {
            assert!(matches!(validate_batch_sizes(sizes), Err(AppError::InvalidConfig(message)) if message == "请先确认所选屏尺寸"));
        }
        for sizes in [vec!["4", "10"], vec!["10", "4"], vec!["4", "4", "10"]] {
            assert!(matches!(validate_batch_sizes(sizes), Err(AppError::InvalidConfig(message)) if message == "4 寸屏和 10 寸屏不能同时修改配置，请分批选择。"));
        }
    }

    #[test]
    fn home_patch_is_explicit_and_separate_from_dart_fields_and_restart() {
        for enabled in [true, false] {
            let patch: AppConfigPatch =
                serde_json::from_value(json!({"homeEnabled":enabled})).unwrap();
            patch.validate().unwrap();
            assert!(!patch.is_empty());
            assert!(patch.fields().is_empty());
            assert!(!patch.restart_required(&json!({})));
            assert_eq!(
                serde_json::to_value(&patch).unwrap()["homeEnabled"],
                enabled
            );
            let restored: AppConfigPatch =
                serde_json::from_value(serde_json::to_value(&patch).unwrap()).unwrap();
            assert_eq!(restored.home_enabled, Some(enabled));
        }
        let unchanged: AppConfigPatch =
            serde_json::from_value(json!({"set":{"customDeviceName":"小新"}})).unwrap();
        assert_eq!(unchanged.home_enabled, None);
        assert!(
            serde_json::to_value(unchanged)
                .unwrap()
                .get("homeEnabled")
                .is_none()
        );
        let invalid: AppConfigPatch =
            serde_json::from_value(json!({"set":{"homeEnabled":true}})).unwrap();
        assert!(invalid.validate().is_err());
        assert!(serde_json::from_value::<AppConfigPatch>(json!({"homeEnabled":"true"})).is_err());
    }
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
        let evidence = json!({"fields":["customDeviceName","environments.pre.wsUrl"],"before":{"customDeviceName":"旧名称","environments":{"pre":{"wsUrl":"wss://private.example"}}},"after":{"customDeviceName":"新名称","environments":{"pre":{"wsUrl":"wss://another.example"}}}});
        let changes = shared_changes(&evidence);
        assert_eq!(
            changes,
            vec![json!({"field":"customDeviceName","before":"旧名称","after":"新名称"})]
        );
    }

    #[test]
    fn voice_auto_start_is_a_strict_boolean_set_and_cannot_be_cleared() {
        assert!(supported_field("autoStartVoiceEngine"));
        for value in [json!(true), json!(false)] {
            let patch: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":value}})).unwrap();
            patch.validate().unwrap();
            assert_eq!(patch.fields(), vec!["autoStartVoiceEngine"]);
            assert!(patch.clear.is_empty());
            assert!(patch.matches(&json!({"autoStartVoiceEngine":value})));
            assert!(!patch.matches(&json!({})));
            assert!(!patch.matches(&json!({"autoStartVoiceEngine":null})));
            assert!(!patch.restart_required(&json!({"autoStartVoiceEngine":value})));
            assert!(patch.restart_required(&json!({"autoStartVoiceEngine":!value.as_bool().unwrap()})));
        }
        for value in [Value::Null, json!("false"), json!("true"), json!(0), json!(1), json!([]), json!({})] {
            let patch: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":value}})).unwrap();
            assert!(patch.validate().is_err());
        }
        let clear: AppConfigPatch = serde_json::from_value(json!({"clear":["autoStartVoiceEngine"]})).unwrap();
        assert!(clear.validate().is_err());
        assert!(!supported_field("environments.prod.autoStartVoiceEngine"));
    }

    #[test]
    fn voice_auto_start_combines_with_environment_changes_in_one_restart_decision() {
        let before = json!({"autoStartVoiceEngine":true,"environments":{"current":"prod","prod":{"h5ReadyCheckEnabled":true}}});
        let changed: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false,"environments.prod.h5ReadyCheckEnabled":false}})).unwrap();
        changed.validate().unwrap();
        assert!(changed.restart_required(&before));
        let same: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":true,"environments.prod.h5ReadyCheckEnabled":true}})).unwrap();
        same.validate().unwrap();
        assert!(same.matches(&before));
        assert!(!same.restart_required(&before));
        let evidence = json!({"fields":["autoStartVoiceEngine","environments.prod.otaUrl"],"before":{"autoStartVoiceEngine":true,"environments":{"prod":{"otaUrl":"https://private.example"}}},"after":{"autoStartVoiceEngine":false,"environments":{"prod":{"otaUrl":"https://another.example"}}}});
        assert_eq!(shared_changes(&evidence), vec![json!({"field":"autoStartVoiceEngine","before":true,"after":false})]);
        for (before, after) in [(Value::Null, json!(false)), (json!("true"), json!(false)), (json!(false), json!(false))] {
            assert!(shared_changes(&json!({"fields":["autoStartVoiceEngine"],"before":{"autoStartVoiceEngine":before},"after":{"autoStartVoiceEngine":after}})).is_empty());
        }
    }
}
