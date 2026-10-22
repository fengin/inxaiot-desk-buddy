//! 小新提供的字段定义只用于限定的配置协议，不接受前端自报能力。
use crate::core::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldType {
    Boolean,
    Integer,
    Number,
    String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum RestartPolicy {
    None,
    AppRestart,
    CurrentEnvironmentRestart,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FieldGroup {
    General,
    Environment,
    Voice,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum StringFormat {
    HttpUrl,
    WebsocketUrl,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct FieldOption {
    pub label: String,
    pub value: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConfigFieldDefinition {
    pub key: String,
    #[serde(rename = "type")]
    pub field_type: FieldType,
    pub label: String,
    pub group: FieldGroup,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub section: Option<String>,
    pub order: i32,
    pub writable: bool,
    pub clearable: bool,
    pub default_value: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<FieldOption>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<StringFormat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub restart_policy: RestartPolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ConfigSchema {
    pub version: u32,
    pub fields: Vec<ConfigFieldDefinition>,
}

pub fn legacy_field(field: &str) -> bool {
    if [
        "customDeviceName",
        "environments.current",
        "autoStartVoiceEngine",
    ]
    .contains(&field)
    {
        return true;
    }
    let parts: Vec<_> = field.split('.').collect();
    parts.len() == 3
        && parts[0] == "environments"
        && ["test", "pre", "prod"].contains(&parts[1])
        && ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"].contains(&parts[2])
}

pub fn audio_field(field: &str) -> bool {
    let Some(leaf) = field.strip_prefix("audioConfig.") else {
        return false;
    };
    field.len() <= 160
        && !leaf.is_empty()
        && leaf.len() <= 64
        && leaf.as_bytes()[0].is_ascii_alphabetic()
        && leaf.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !["__proto__", "prototype", "constructor"].contains(&leaf)
}

pub fn safe_field_path(field: &str) -> bool {
    field.len() <= 160
        && !field
            .split('.')
            .any(|part| ["__proto__", "prototype", "constructor"].contains(&part))
        && (legacy_field(field) || audio_field(field))
}

fn text_ok(text: &str, limit: usize, allow_empty: bool) -> bool {
    (allow_empty || !text.trim().is_empty())
        && text.chars().count() <= limit
        && !text.chars().any(char::is_control)
}

impl ConfigFieldDefinition {
    pub fn shape_valid(&self) -> bool {
        if !safe_field_path(&self.key)
            || !text_ok(&self.label, 80, false)
            || self
                .section
                .as_ref()
                .is_some_and(|s| !text_ok(s, 80, false))
            || self.unit.as_ref().is_some_and(|s| !text_ok(s, 16, true))
            || self
                .description
                .as_ref()
                .is_some_and(|s| !text_ok(s, 400, true))
            || !(-10000..=10000).contains(&self.order)
            || self.min.is_some_and(|v| !v.is_finite())
            || self.max.is_some_and(|v| !v.is_finite())
            || self.step.is_some_and(|v| !v.is_finite() || v <= 0.0)
            || matches!((self.min, self.max), (Some(min), Some(max)) if min > max)
            || self.max_length.is_some_and(|v| v == 0 || v > 4096)
        {
            return false;
        }
        let numeric = matches!(self.field_type, FieldType::Integer | FieldType::Number);
        if (!numeric && (self.min.is_some() || self.max.is_some() || self.step.is_some()))
            || (self.field_type != FieldType::String && self.max_length.is_some())
            || (self.field_type != FieldType::String && self.format.is_some())
        {
            return false;
        }
        if audio_field(&self.key) {
            if self.group != FieldGroup::Voice
                || self.clearable
                || (self.field_type == FieldType::String && self.options.is_none())
                || self.restart_policy == RestartPolicy::CurrentEnvironmentRestart
                || self.format.is_some()
            {
                return false;
            }
        } else {
            let expected = if self.key == "autoStartVoiceEngine"
                || self.key.ends_with(".h5ReadyCheckEnabled")
            {
                FieldType::Boolean
            } else {
                FieldType::String
            };
            if self.field_type != expected
                || (self.clearable
                    && self.key != "customDeviceName"
                    && !self.key.ends_with(".wsUrl")
                    && !self.key.ends_with(".h5Url"))
                || (self.key.starts_with("environments.") && self.group != FieldGroup::Environment)
                || (self.key == "autoStartVoiceEngine" && self.group != FieldGroup::Voice)
                || (self.key == "customDeviceName" && self.group != FieldGroup::General)
            {
                return false;
            }
        }
        if let Some(format) = &self.format {
            let expected = if self.key.ends_with(".wsUrl") {
                Some(StringFormat::WebsocketUrl)
            } else if self.key.ends_with(".otaUrl") || self.key.ends_with(".h5Url") {
                Some(StringFormat::HttpUrl)
            } else {
                None
            };
            if expected.as_ref() != Some(format) {
                return false;
            }
        }
        if let Some(options) = &self.options {
            if options.is_empty() || options.len() > 64 {
                return false;
            }
            for (index, option) in options.iter().enumerate() {
                if !text_ok(&option.label, 80, false)
                    || !self.scalar_valid(&option.value)
                    || options[..index]
                        .iter()
                        .any(|other| values_equal(&other.value, &option.value))
                {
                    return false;
                }
            }
        }
        self.accepts(&self.default_value)
    }

    fn scalar_valid(&self, value: &Value) -> bool {
        if value.is_null() {
            return self.clearable;
        }
        match self.field_type {
            FieldType::Boolean => value.is_boolean(),
            FieldType::Integer => {
                (value.as_i64().is_some() || value.as_u64().is_some())
                    && self.numeric_in_range(value)
            }
            FieldType::Number => value.is_number() && self.numeric_in_range(value),
            FieldType::String => value.as_str().is_some_and(|v| {
                text_ok(v, self.max_length.unwrap_or(4096), true)
                    && self.format.as_ref().is_none_or(|format| {
                        // 存量可空手动地址曾保存为空串；读取保留它，写入仍要求明确clear。
                        if v.is_empty() && self.clearable {
                            return true;
                        }
                        reqwest::Url::parse(v).ok().is_some_and(|url| {
                            url.host_str().is_some()
                                && match format {
                                    StringFormat::HttpUrl => {
                                        ["http", "https"].contains(&url.scheme())
                                    }
                                    StringFormat::WebsocketUrl => {
                                        ["ws", "wss"].contains(&url.scheme())
                                    }
                                }
                        })
                    })
            }),
        }
    }

    fn numeric_in_range(&self, value: &Value) -> bool {
        value.as_f64().is_some_and(|n| {
            n.is_finite()
                && self.min.is_none_or(|min| n >= min)
                && self.max.is_none_or(|max| n <= max)
        })
    }

    pub fn accepts(&self, value: &Value) -> bool {
        self.scalar_valid(value)
            && (value.is_null()
                || self
                    .options
                    .as_ref()
                    .is_none_or(|options| options.iter().any(|o| values_equal(&o.value, value))))
    }

    /// 存量值可能不符合新范围，仍保留可信的实际标量以便用户修正，不用默认值替换。
    pub fn readable_value(&self, value: &Value) -> bool {
        if value.is_null() {
            return self.clearable;
        }
        match self.field_type {
            FieldType::Boolean => value.is_boolean(),
            FieldType::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            FieldType::Number => value.as_f64().is_some_and(f64::is_finite),
            FieldType::String => value
                .as_str()
                .is_some_and(|v| v.chars().count() <= 4096 && !v.chars().any(char::is_control)),
        }
    }

    pub fn requires_restart(&self, before: &Value, after_value: &Value) -> bool {
        if super::app_config::field_value(before, &self.key)
            .is_some_and(|v| values_equal(v, after_value))
        {
            return false;
        }
        match self.restart_policy {
            RestartPolicy::None => false,
            RestartPolicy::AppRestart => true,
            RestartPolicy::CurrentEnvironmentRestart => {
                let current = before["environments"]["current"].as_str().unwrap_or("");
                self.key.starts_with(&format!("environments.{current}."))
            }
        }
    }
}

impl ConfigSchema {
    pub fn from_capabilities(cap: &Value) -> Result<Option<Self>, &'static str> {
        let Some(raw) = cap.get("configSchema") else {
            return Ok(None);
        };
        if raw["version"].as_u64() != Some(1) {
            return Err("当前小新的字段定义版本暂不支持，语音参数暂不可修改");
        }
        let schema: Self = serde_json::from_value(raw.clone())
            .map_err(|_| "小新字段定义不完整，语音参数暂不可修改，请重新读取")?;
        let invalid = "小新字段定义无效，语音参数暂不可修改，请重新读取";
        if schema.fields.is_empty() || schema.fields.len() > 64 {
            return Err(invalid);
        }
        let supported = cap["supportedFields"].as_array().ok_or(invalid)?;
        let supported: BTreeSet<&str> = supported
            .iter()
            .map(|v| v.as_str().filter(|key| safe_field_path(key)).ok_or(invalid))
            .collect::<Result<_, _>>()?;
        if cap["supportedFields"].as_array().unwrap().len() != supported.len()
            || supported.len() > 64
        {
            return Err(invalid);
        }
        let mut keys = BTreeSet::new();
        for field in &schema.fields {
            if !keys.insert(field.key.as_str())
                || !field.shape_valid()
                || (field.writable && !supported.contains(field.key.as_str()))
            {
                return Err(invalid);
            }
        }
        if !supported.iter().all(|key| keys.contains(key)) {
            return Err(invalid);
        }
        Ok(Some(schema))
    }

    pub fn field(&self, key: &str) -> Option<&ConfigFieldDefinition> {
        self.fields.iter().find(|field| field.key == key)
    }

    pub fn validate_patch(
        &self,
        patch: &super::app_config::AppConfigPatch,
        current: &Value,
    ) -> AppResult<()> {
        for key in patch.fields() {
            let field = self.field(&key).filter(|f| f.writable).ok_or_else(|| {
                AppError::Conflict("当前小新未开放所选配置字段，请重新读取或升级应用".into())
            })?;
            let before =
                super::app_config::field_value(current, &key).filter(|v| field.readable_value(v));
            if before.is_none() {
                return Err(AppError::Conflict(format!(
                    "{}当前值未确认，未修改配置，请重新读取",
                    field.label
                )));
            }
            if let Some(value) = patch.set.get(&key) {
                if value.is_null() || !field.accepts(value) {
                    return Err(AppError::InvalidConfig(format!(
                        "{}不符合屏端字段定义，请检查类型、范围或选项",
                        field.label
                    )));
                }
            } else if !field.clearable {
                return Err(AppError::InvalidConfig(format!(
                    "{}不允许清空",
                    field.label
                )));
            }
        }
        Ok(())
    }

    pub fn restart_required(
        &self,
        patch: &super::app_config::AppConfigPatch,
        before: &Value,
    ) -> bool {
        patch.fields().iter().any(|key| {
            self.field(key).is_some_and(|field| {
                field.requires_restart(before, patch.set.get(key).unwrap_or(&Value::Null))
            })
        })
    }

    pub fn selected_definitions(&self, fields: &[String]) -> Vec<ConfigFieldDefinition> {
        self.fields
            .iter()
            .filter(|field| fields.contains(&field.key))
            .cloned()
            .collect()
    }
}

/// 不用f64转换两个整数，避免相邻大整数被错误判为同值。
pub fn values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(a), Value::Number(b)) => {
            fn integer(n: &serde_json::Number) -> Option<i128> {
                n.as_i64()
                    .map(i128::from)
                    .or_else(|| n.as_u64().map(i128::from))
            }
            match (integer(a), integer(b)) {
                (Some(a), Some(b)) => a == b,
                (Some(i), None) => b
                    .as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0 && f as i128 == i),
                (None, Some(i)) => a
                    .as_f64()
                    .is_some_and(|f| f.is_finite() && f.fract() == 0.0 && f as i128 == i),
                (None, None) => a.as_f64() == b.as_f64(),
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(a, b)| values_equal(a, b))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|other| values_equal(v, other)))
        }
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::smart_screen::app_config::{
        AppConfigPatch, shared_changes, shared_field_labels,
    };
    use serde_json::json;

    fn definition(key: &str, kind: &str, default: Value, restart: &str) -> Value {
        json!({"key":key,"type":kind,"label":"测试字段","group":if key.starts_with("environments.") {"environment"} else if key=="customDeviceName" {"general"} else {"voice"},
            "order":10,"writable":true,"clearable":key=="customDeviceName" || key.ends_with(".wsUrl"),"defaultValue":default,"restartPolicy":restart})
    }
    fn cap(fields: Vec<Value>) -> Value {
        json!({"supportedFields":fields.iter().map(|f|f["key"].clone()).collect::<Vec<_>>(),"configSchema":{"version":1,"fields":fields}})
    }
    fn patch(value: Value) -> AppConfigPatch {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn only_safe_legacy_or_audio_leaf_namespaces_are_supported() {
        for field in [
            "audioConfig",
            "audioConfig.__proto__",
            "audioConfig.constructor",
            "audioConfig.prototype",
            "audioConfig.x.y",
            "authCode",
            "deviceId",
            "extendParams.site",
            "environments.prod.privateKey",
        ] {
            assert!(!safe_field_path(field), "{field}");
        }
        assert!(safe_field_path("audioConfig.futureSensitivity"));
        assert!(safe_field_path("environments.prod.h5ReadyCheckEnabled"));
    }

    #[test]
    fn definitions_reject_bad_versions_duplicates_ranges_and_unbounded_metadata() {
        let base = cap(vec![definition(
            "audioConfig.futureSensitivity",
            "number",
            json!(0.6),
            "appRestart",
        )]);
        assert!(ConfigSchema::from_capabilities(&base).unwrap().is_some());
        let mut cases = Vec::new();
        let mut version = base.clone();
        version["configSchema"]["version"] = json!(2);
        cases.push(version);
        let mut duplicate = base.clone();
        duplicate["configSchema"]["fields"]
            .as_array_mut()
            .unwrap()
            .push(base["configSchema"]["fields"][0].clone());
        cases.push(duplicate);
        let mut private = base.clone();
        private["configSchema"]["fields"][0]["key"] = json!("authCode");
        cases.push(private);
        let mut limits = base.clone();
        limits["configSchema"]["fields"][0]["min"] = json!(2);
        limits["configSchema"]["fields"][0]["max"] = json!(1);
        cases.push(limits);
        let mut label = base.clone();
        label["configSchema"]["fields"][0]["label"] = json!("x".repeat(81));
        cases.push(label);
        let mut group = base.clone();
        group["configSchema"]["fields"][0]["group"] = json!("general");
        cases.push(group);
        let mut options = base.clone();
        options["configSchema"]["fields"][0]["options"] =
            json!([{"label":"a","value":0.6},{"label":"b","value":0.60}]);
        cases.push(options);
        let mut unsupported = base.clone();
        unsupported["supportedFields"] = json!([]);
        cases.push(unsupported);
        for malformed in cases {
            assert!(
                ConfigSchema::from_capabilities(&malformed).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn step_is_a_ui_hint_but_integer_and_options_are_strict() {
        let mut duration = definition(
            "audioConfig.minSilenceDuration",
            "number",
            json!(0.6),
            "appRestart",
        );
        duration["min"] = json!(0.3);
        duration["max"] = json!(2);
        duration["step"] = json!(0.1);
        let mut integer = definition(
            "audioConfig.micSampleRate",
            "integer",
            json!(16000),
            "appRestart",
        );
        integer["options"] =
            json!([{"label":"11025","value":11025},{"label":"16000","value":16000}]);
        let schema = ConfigSchema::from_capabilities(&cap(vec![duration, integer]))
            .unwrap()
            .unwrap();
        let current = json!({"audioConfig":{"minSilenceDuration":0.6,"micSampleRate":16000}});
        assert!(
            schema
                .validate_patch(
                    &patch(json!({"set":{"audioConfig.minSilenceDuration":0.62}})),
                    &current
                )
                .is_ok()
        );
        assert!(
            schema
                .validate_patch(
                    &patch(json!({"set":{"audioConfig.micSampleRate":11025}})),
                    &current
                )
                .is_ok()
        );
        for value in [
            json!(11025.0),
            json!(44100),
            json!("16000"),
            json!(true),
            Value::Null,
        ] {
            assert!(
                schema
                    .validate_patch(
                        &patch(json!({"set":{"audioConfig.micSampleRate":value}})),
                        &current
                    )
                    .is_err()
            );
        }
        assert!(
            schema
                .validate_patch(
                    &patch(json!({"clear":["audioConfig.minSilenceDuration"]})),
                    &current
                )
                .is_err()
        );
    }

    #[test]
    fn actual_current_value_is_required_and_default_never_authorizes_a_missing_value() {
        let schema = ConfigSchema::from_capabilities(&cap(vec![definition(
            "audioConfig.vadThreshold",
            "number",
            json!(0.8),
            "appRestart",
        )]))
        .unwrap()
        .unwrap();
        let change = patch(json!({"set":{"audioConfig.vadThreshold":0.7}}));
        for current in [
            json!({}),
            json!({"audioConfig":{}}),
            json!({"audioConfig":{"vadThreshold":null}}),
            json!({"audioConfig":{"vadThreshold":"0.8"}}),
        ] {
            assert!(schema.validate_patch(&change, &current).is_err());
        }
        let mut readonly = cap(vec![definition(
            "audioConfig.vadThreshold",
            "number",
            json!(0.8),
            "appRestart",
        )]);
        readonly["configSchema"]["fields"][0]["writable"] = json!(false);
        assert!(
            ConfigSchema::from_capabilities(&readonly)
                .unwrap()
                .unwrap()
                .validate_patch(&change, &json!({"audioConfig":{"vadThreshold":0.8}}))
                .is_err()
        );
    }

    #[test]
    fn restart_is_driven_by_metadata_and_only_actual_changes() {
        let capability = cap(vec![
            definition(
                "audioConfig.futureSensitivity",
                "number",
                json!(2.0),
                "none",
            ),
            definition(
                "audioConfig.minSilenceDuration",
                "number",
                json!(0.6),
                "appRestart",
            ),
            definition(
                "environments.test.wsUrl",
                "string",
                Value::Null,
                "currentEnvironmentRestart",
            ),
            definition(
                "environments.prod.wsUrl",
                "string",
                Value::Null,
                "currentEnvironmentRestart",
            ),
        ]);
        let before = json!({"audioConfig":{"futureSensitivity":2.0,"minSilenceDuration":0.6},"environments":{"current":"prod","test":{"wsUrl":null},"prod":{"wsUrl":null}}});
        let same = patch(json!({"set":{"audioConfig.futureSensitivity":2}}));
        assert!(same.matches(&before));
        assert!(!same.restart_required_with_capabilities(&before, &capability));
        assert!(
            !patch(json!({"set":{"audioConfig.futureSensitivity":3}}))
                .restart_required_with_capabilities(&before, &capability)
        );
        assert!(
            patch(json!({"set":{"audioConfig.minSilenceDuration":0.7}}))
                .restart_required_with_capabilities(&before, &capability)
        );
        assert!(
            !patch(json!({"set":{"environments.test.wsUrl":"wss://example.invalid"}}))
                .restart_required_with_capabilities(&before, &capability)
        );
        assert!(
            patch(json!({"set":{"environments.prod.wsUrl":"wss://example.invalid"}}))
                .restart_required_with_capabilities(&before, &capability)
        );
    }

    #[test]
    fn numeric_equality_never_rounds_distinct_large_integers() {
        assert!(values_equal(&json!(2), &json!(2.0)));
        assert!(!values_equal(
            &json!(9007199254740992_i64),
            &json!(9007199254740993_i64)
        ));
        assert!(!values_equal(
            &json!(9007199254740993_i64),
            &json!(9007199254740992_f64)
        ));
        assert!(!values_equal(
            &json!(u64::MAX),
            &json!(18446744073709551616_f64)
        ));
        assert!(!values_equal(&json!(false), &json!(0)));
    }

    #[test]
    fn shared_audio_changes_have_safe_labels_without_private_or_address_values() {
        let mut field = definition(
            "audioConfig.minSilenceDuration",
            "number",
            json!(0.6),
            "appRestart",
        );
        field["label"] = json!("静音判定时长");
        field["unit"] = json!("秒");
        let mut evidence = json!({"fields":["audioConfig.minSilenceDuration","authCode","environments.prod.wsUrl"],"fieldDefinitions":[field],
            "before":{"audioConfig":{"minSilenceDuration":0.6},"authCode":"private-before","environments":{"prod":{"wsUrl":"wss://private-before.invalid"}}},
            "after":{"audioConfig":{"minSilenceDuration":0.7},"authCode":"private-after","environments":{"prod":{"wsUrl":"wss://private-after.invalid"}}}});
        assert_eq!(
            shared_changes(&evidence),
            vec![
                json!({"field":"audioConfig.minSilenceDuration","label":"静音判定时长","unit":"秒","before":0.6,"after":0.7})
            ]
        );
        evidence["fieldDefinitions"] = json!([]);
        assert!(shared_changes(&evidence).is_empty());
    }

    #[test]
    fn shared_failure_labels_do_not_require_value_changes_or_export_definition_defaults() {
        let mut field = definition(
            "audioConfig.minSilenceDuration",
            "number",
            json!(0.6),
            "appRestart",
        );
        field["label"] = json!("静音判定时长");
        let mut evidence = json!({"fields":["audioConfig.minSilenceDuration"],"fieldDefinitions":[field],"save":"failed"});
        assert!(shared_changes(&evidence).is_empty());
        assert_eq!(
            shared_field_labels(&evidence),
            std::collections::BTreeMap::from([(
                "audioConfig.minSilenceDuration".into(),
                "静音判定时长".into()
            )])
        );
        evidence["fieldDefinitions"][0]["label"] = json!("https://user:password@example.invalid");
        assert!(shared_field_labels(&evidence).is_empty());
    }
}
