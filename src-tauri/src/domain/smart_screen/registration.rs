use super::model::{ScreenAsset, ScreenDraft, ScreenFields, ScreenObservation};
use super::rules::normalize_mac;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationDiff {
    pub field: String,
    pub label: String,
    pub before: String,
    pub after: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationItem {
    pub screen_id: String,
    pub mode: String,
    pub state: String,
    pub reason: String,
    pub before: Option<ScreenFields>,
    pub after: ScreenFields,
    pub diffs: Vec<RegistrationDiff>,
    pub expected_revision: u64,
    pub needs_space_confirmation: bool,
    pub mac_source: String,
    pub mac_message: String,
    pub required_mac_confirmation: Option<String>,
    pub duplicate_ids: Vec<String>,
    #[serde(default)]
    pub platform_app_version: Option<String>,
    #[serde(default)]
    pub proposed_app_version: Option<String>,
    #[serde(default)]
    pub version_message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationPreview {
    pub id: String,
    pub project_id: String,
    pub created_at: String,
    pub items: Vec<RegistrationItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationSubmission {
    pub preview_id: String,
    pub screen_ids: Vec<String>,
    #[serde(default)]
    pub mac_confirmations: BTreeMap<String, String>,
    #[serde(default)]
    pub space_confirmations: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationPlan {
    pub preview: RegistrationPreview,
    pub original_assets: BTreeMap<String, ScreenAsset>,
    pub draft_revisions: BTreeMap<String, u64>,
    pub request_ids: BTreeMap<String, String>,
    pub platform_ids: BTreeMap<String, String>,
    pub confirmations: Option<RegistrationSubmission>,
    #[serde(default)]
    pub merge_versions: BTreeMap<String, VersionChange>,
    #[serde(default)]
    pub merge_sources: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub registration_versions: BTreeMap<String, RegistrationVersion>,
}
impl RegistrationPlan {
    pub fn version_change(&self, screen_id: &str) -> Option<&VersionChange> {
        self.registration_versions.get(screen_id).map(|v| &v.change)
            .or_else(|| self.merge_versions.get(screen_id))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationVersion {
    pub change: VersionChange,
    pub observation: ScreenObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionChange {
    pub before: Option<String>,
    pub after: Option<String>,
}
pub fn field_values(fields: &ScreenFields) -> BTreeMap<&'static str, String> {
    BTreeMap::from([
        ("name", fields.name.clone()),
        ("ip", fields.ip.clone()),
        ("mac", fields.mac.clone()),
        ("size", fields.size.clone()),
        ("spaceId", fields.space_id.clone().unwrap_or_default()),
        ("location", fields.location.clone()),
    ])
}

/// 仅用于资料意图和差异判断；平台写入的原值条件仍保留原始值。
pub fn field_semantically_equal(field: &str, left: &str, right: &str) -> bool {
    if field == "mac" {
        normalize_mac(left.trim()) == normalize_mac(right.trim())
    } else {
        left.trim() == right.trim()
    }
}

pub fn fields_semantically_equal(left: &ScreenFields, right: &ScreenFields) -> bool {
    let right = field_values(right);
    field_values(left)
        .iter()
        .all(|(field, value)| field_semantically_equal(field, value, &right[field]))
}

/// 未编辑字段跟随当前平台资料，不把旧草稿整份覆盖到新平台基准。
pub fn effective_draft_fields(current: &ScreenFields, draft: &ScreenDraft) -> ScreenFields {
    let mut values = current.clone();
    let base = field_values(&draft.base);
    for (field, value) in field_values(&draft.values) {
        if !field_semantically_equal(field, &value, &base[field]) {
            set_field(&mut values, field, value);
        }
    }
    values
}

pub fn draft_conflicting_fields(
    current: &ScreenFields,
    draft: &ScreenDraft,
) -> Vec<&'static str> {
    let base = field_values(&draft.base);
    let current = field_values(current);
    field_values(&draft.values)
        .into_iter()
        .filter_map(|(field, wanted)| {
            (!field_semantically_equal(field, &wanted, &base[field])
                && !field_semantically_equal(field, &current[field], &base[field])
                && !field_semantically_equal(field, &current[field], &wanted))
            .then_some(field)
        })
        .collect()
}
pub fn label(field: &str) -> &str {
    match field {
        "name" => "名称",
        "ip" => "IP 地址",
        "mac" => "MAC 地址",
        "size" => "屏尺寸",
        "spaceId" => "所在空间",
        "location" => "详细位置",
        _ => field,
    }
}
pub fn set_field(fields: &mut ScreenFields, key: &str, value: String) {
    match key {
        "name" => fields.name = value,
        "ip" => fields.ip = value,
        "mac" => fields.mac = value,
        "size" => fields.size = value,
        "spaceId" => fields.space_id = (!value.is_empty()).then_some(value),
        "location" => fields.location = value,
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields() -> ScreenFields {
        ScreenFields {
            name: "测试屏".into(),
            ip: "192.0.2.10".into(),
            mac: "02:AB:22:33:44:55".into(),
            size: "4".into(),
            space_id: None,
            location: "原位置".into(),
        }
    }

    fn draft(base: &ScreenFields, values: ScreenFields) -> ScreenDraft {
        ScreenDraft {
            screen_id: "screen".into(),
            base_revision: 1,
            revision: 2,
            base: base.clone(),
            values,
            updated_at: "time".into(),
        }
    }

    #[test]
    fn field_semantics_ignore_only_mac_format_and_outer_whitespace() {
        assert!(field_semantically_equal("mac", " 02-ab-22-33-44-55 ", "02:AB:22:33:44:55"));
        assert!(field_semantically_equal("mac", " ", ""));
        assert!(!field_semantically_equal("mac", "02:AB:22:33:44:55", "02:AB:22:33:44:56"));
        assert!(field_semantically_equal("name", " 测试屏 ", "测试屏"));
        assert!(!field_semantically_equal("name", "屏 A", "屏A"));
        let base = fields();
        let mut empty_space = base.clone();
        empty_space.space_id = Some(String::new());
        assert!(fields_semantically_equal(&base, &empty_space));
    }

    #[test]
    fn effective_draft_preserves_current_untouched_fields_and_raw_revision() {
        let base = fields();
        let mut wanted = base.clone();
        wanted.name = "待更新名称".into();
        wanted.mac = "02-ab-22-33-44-55".into();
        let pending = draft(&base, wanted);
        let mut current = base.clone();
        current.location = "平台新位置".into();
        current.mac = "02AB22334455".into();
        let effective = effective_draft_fields(&current, &pending);
        assert_eq!(effective.name, "待更新名称");
        assert_eq!(effective.location, "平台新位置");
        assert_eq!(effective.mac, current.mac);
        assert_eq!(pending.revision, 2);
        assert_eq!(pending.base.location, "原位置");
        assert!(draft_conflicting_fields(&current, &pending).is_empty());
    }

    #[test]
    fn three_way_conflict_preserves_a_real_third_value_but_not_an_applied_intent() {
        let base = fields();
        let mut wanted = base.clone();
        wanted.name = "待更新名称".into();
        let pending = draft(&base, wanted.clone());
        let mut current = base.clone();
        current.name = "平台另一名称".into();
        assert_eq!(draft_conflicting_fields(&current, &pending), vec!["name"]);
        current.name = " 待更新名称 ".into();
        assert!(draft_conflicting_fields(&current, &pending).is_empty());
        assert!(fields_semantically_equal(&current, &effective_draft_fields(&current, &pending)));
    }
}
