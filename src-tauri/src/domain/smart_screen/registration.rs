use super::model::{ScreenAsset, ScreenFields};
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
