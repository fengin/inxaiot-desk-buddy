use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenFields {
    pub name: String,
    pub ip: String,
    pub mac: String,
    pub size: String,
    pub space_id: Option<String>,
    pub location: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenAsset {
    pub id: String,
    pub source: String,
    #[serde(flatten)]
    pub fields: ScreenFields,
    pub revision: u64,
    pub app_version: Option<String>,
    pub platform_status: String,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenDraft {
    pub screen_id: String,
    pub base_revision: u64,
    pub revision: u64,
    pub base: ScreenFields,
    pub values: ScreenFields,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenObservation {
    pub id: String,
    pub screen_id: String,
    pub operation_type: String,
    pub observed_ip: String,
    pub observed_at: String,
    pub task_id: Option<String>,
    pub ping: Option<String>,
    pub adb_available: bool,
    pub android: Option<String>,
    pub device_model: Option<String>,
    pub firmware: Option<String>,
    pub sdk: Option<u32>,
    pub abis: Vec<String>,
    pub observed_mac: Option<String>,
    pub mac_source: Option<String>,
    pub mac_candidates: Vec<String>,
    pub observed_app_version: Option<String>,
    pub app_version_code: Option<u64>,
    pub package_id: Option<String>,
    pub app_installed: Option<bool>,
    pub app_running: Option<bool>,
    pub free_space_mb: Option<u64>,
    pub clock_offset_seconds: Option<i64>,
    pub device_time: Option<String>,
    pub computer_time: Option<String>,
    pub timezone: Option<String>,
    pub automatic_time: Option<bool>,
    pub automatic_timezone: Option<bool>,
    pub persistent_adb: Option<bool>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenSnapshot {
    pub local_only: bool,
    pub screens: Vec<ScreenAsset>,
    pub spaces: Vec<SpaceNode>,
    pub observations: BTreeMap<String, Vec<ScreenObservation>>,
    pub platform_drafts: BTreeMap<String, ScreenDraft>,
    pub ignored_pairs: Vec<String>,
    pub platform_available: bool,
    pub spaces_available: bool,
    pub platform_read_at: Option<String>,
    pub business_project_id: Option<String>,
    pub available_projects: Vec<BusinessProject>,
    pub platform_message: Option<String>,
    pub tasks: Vec<super::operation::ScreenTaskView>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessProject {
    pub id: String,
    pub name: String,
}

/// 三类结果独立保存，记录写入失败不能重做已成功的设备动作。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultState {
    #[default]
    NotRequired,
    Pending,
    Succeeded,
    Failed,
    Unknown,
    Skipped,
    Cancelled,
}

pub fn target_succeeded(result:&ScreenTargetResult)->bool{
    matches!(result.device,ResultState::Succeeded|ResultState::Skipped|ResultState::NotRequired)
        && matches!(result.business,ResultState::Succeeded|ResultState::Skipped|ResultState::NotRequired)
}
pub fn target_cancelled(result:&ScreenTargetResult)->bool{result.device==ResultState::Cancelled||result.business==ResultState::Cancelled}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTargetResult {
    pub format_version: u32,
    pub screen_id: String,
    pub device: ResultState,
    pub business: ResultState,
    pub shared: ResultState,
    pub before_app_version: Option<String>,
    pub after_app_version: Option<String>,
    pub observation: Option<ScreenObservation>,
    pub message: String,
    pub evidence: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteIntent {
    pub request_id: String,
    pub business_project_id: String,
    pub screen_id: String,
    pub platform_screen_id: String,
    pub operation_type: String,
    pub payload: Value,
    pub state: String,
    pub result: Option<Value>,
}
