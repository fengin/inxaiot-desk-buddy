use super::model::{ScreenAsset, ScreenTargetResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const READ_ACTIONS: &[&str] = &["ping", "inspect", "mac", "diagnostics"];
pub const WRITE_ACTIONS: &[&str] = &["install", "time", "adb", "restart", "reboot", "app_config"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ScreenOperationInput {
    pub action: String,
    pub target_ids: Vec<String>,
    #[serde(default)]
    pub application_id: Option<String>,
    #[serde(default)]
    pub apk: Option<serde_json::Value>,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub abi: String,
    #[serde(default)]
    pub reinstall: bool,
    pub concurrency: u32,
    #[serde(default)]
    pub expected_targets: BTreeMap<String, String>,
    #[serde(default)]
    pub retry_of_operation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenPreflightItem {
    pub screen_id: String,
    pub name: String,
    pub ip: String,
    pub state: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observation: Option<super::model::ScreenObservation>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenPreflight {
    pub id: String,
    pub items: Vec<ScreenPreflightItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenPlan {
    pub project_id: String,
    pub input: ScreenOperationInput,
    pub targets: Vec<ScreenAsset>,
    pub business_project_id: Option<String>,
    pub data_source_id: Option<String>,
    pub operator: String,
    pub instance_id: String,
    pub created_at: String,
    #[serde(default)]
    pub detail: serde_json::Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenResults {
    pub targets: BTreeMap<String, ScreenTargetResult>,
    pub finished: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTaskTarget {
    pub screen_id: String,
    pub name: String,
    pub ip: String,
    pub state: String,
    pub progress: u32,
    pub message: String,
    pub result: Option<ScreenTargetResult>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTaskLog {
    pub time: String,
    pub level: String,
    pub message: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenTaskView {
    pub id: String,
    pub project_id: String,
    pub action: String,
    pub state: String,
    pub created_at: String,
    pub updated_at: String,
    pub targets: Vec<ScreenTaskTarget>,
    pub logs: Vec<ScreenTaskLog>,
    pub input: Option<ScreenOperationInput>,
}
pub fn action_label(action: &str) -> &str {
    match action {
        "ping" => "检查屏在离线",
        "inspect" => "检查设备",
        "mac" => "获取/核对 MAC",
        "diagnostics" => "采集诊断",
        "install" => "安装/升级小新",
        "time" => "校准时间",
        "adb" => "保持 ADB 端口",
        "restart" => "重启小新应用",
        "app_config" => "修改小新配置",
        "reboot" => "重启屏",
        "register" => "注册/更新到平台",
        "merge" => "合并屏记录",
        "version_sync" => "同步平台应用版本",
        "status" => "覆盖平台状态",
        _ => "智能屏操作",
    }
}
