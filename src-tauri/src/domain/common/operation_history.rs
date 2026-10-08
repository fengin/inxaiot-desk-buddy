use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryQuery {
    pub page: u32,
    pub page_size: u32,
    pub operation_type: Option<String>,
    pub state: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryItem {
    pub id: String,
    pub domain_type: String,
    pub operation_type: String,
    pub operation_name: String,
    pub operator_name: String,
    pub instance_id: String,
    pub state: String,
    pub target_count: u32,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
    pub artifact_name: Option<String>,
    pub artifact_version: Option<String>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryTarget {
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    pub resource_type: String,
    pub resource_key: String,
    pub state: String,
    pub before_version: Option<String>,
    pub after_version: Option<String>,
    pub result_summary: Option<String>,
    pub error_code: Option<String>,
    pub error_summary: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryPage {
    pub items: Vec<OperationHistoryItem>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationHistoryDetail {
    pub operation: OperationHistoryItem,
    pub targets: Vec<OperationHistoryTarget>,
}
