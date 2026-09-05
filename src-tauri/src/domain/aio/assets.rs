use serde::{Deserialize, Serialize};

use super::inventory::{ImportCounts, ReconciledImportItem};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioImportSession {
    pub id: String,
    pub local_project_id: String,
    pub file_name: String,
    pub file_path: String,
    pub state: String,
    pub counts: ImportCounts,
    pub created_at: String,
    pub updated_at: String,
    pub items: Vec<ReconciledImportItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSelection {
    pub row_number: u32,
    pub selected: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformRecordIssue {
    pub platform_aio_id: String,
    pub name: String,
    pub ip: String,
    pub code: String,
    pub message: String,
    pub raw_mac: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceVersionRecord {
    pub mac_normalized: String,
    pub service_name: String,
    pub expected_image_name: Option<String>,
    pub expected_version: Option<String>,
    pub observed_image_name: Option<String>,
    pub observed_version: Option<String>,
    pub observed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationRecordSummary {
    pub id: String,
    pub operation_type: String,
    pub operation_name: String,
    pub state: String,
    pub operator_name: String,
    pub ended_at: Option<String>,
    pub result_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryApplyResult {
    pub operation_id: String,
    pub applied_count: u32,
}
