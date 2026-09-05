use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::error::AppResult;
use crate::domain::aio::assets::{
    AioImportSession, ImportSelection, InventoryApplyResult, OperationRecordSummary,
    PlatformRecordIssue, ServiceVersionRecord,
};
use crate::domain::aio::inventory::{FieldConflict, PlatformNodeSnapshot};
use crate::domain::aio::service_check::NodeServiceCheckSnapshot;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAioNodesQuery {
    pub search: Option<String>,
    pub state: Option<String>,
    pub page: u32,
    pub page_size: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeStats {
    pub total: u32,
    pub online: u32,
    pub offline: u32,
    pub pending: u32,
    pub conflicts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeListItem {
    pub mac: String,
    pub mac_normalized: String,
    pub name: String,
    pub ip: String,
    pub location: String,
    pub management_state: String,
    pub deploy_label: String,
    pub platform_state: String,
    pub platform_updated_at: String,
    pub service_state: String,
    pub service_label: String,
    pub last_operation: String,
    pub last_operation_at: Option<String>,
    pub platform_id: Option<String>,
    pub source: String,
    pub version: u64,
    pub conflicts: Vec<FieldConflict>,
    pub versions: Vec<ServiceVersionRecord>,
    pub service_check: Option<NodeServiceCheckSnapshot>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeListPage {
    pub items: Vec<AioNodeListItem>,
    pub total: u32,
    pub page: u32,
    pub page_size: u32,
    pub stats: AioNodeStats,
    pub platform_issues: Vec<PlatformRecordIssue>,
    pub latest_import_session_id: Option<String>,
    pub refreshed_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalCheckRecord {
    pub step_code: String,
    pub state: String,
    pub message: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeDetail {
    pub node: AioNodeListItem,
    pub platform: Option<PlatformNodeSnapshot>,
    pub versions: Vec<ServiceVersionRecord>,
    pub last_operation: Option<OperationRecordSummary>,
    pub latest_ssh_check: Option<LocalCheckRecord>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryPreview {
    pub session: AioImportSession,
    pub platform_issues: Vec<PlatformRecordIssue>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryApplyOutcome {
    pub result: InventoryApplyResult,
    pub local_session_finalized: bool,
}

#[allow(async_fn_in_trait)]
pub trait AioAssetsPort: Send + Sync {
    async fn list_nodes(
        &self,
        local_project_id: &str,
        query: ListAioNodesQuery,
    ) -> AppResult<AioNodeListPage>;
    async fn node_detail(&self, local_project_id: &str, mac: &str) -> AppResult<AioNodeDetail>;
    async fn preview_import(
        &self,
        local_project_id: &str,
        file_path: &Path,
    ) -> AppResult<InventoryPreview>;
    async fn latest_import(&self, local_project_id: &str) -> AppResult<Option<AioImportSession>>;
    async fn update_selection(
        &self,
        local_project_id: &str,
        session_id: &str,
        selections: &[ImportSelection],
    ) -> AppResult<AioImportSession>;
    async fn discard_import(&self, local_project_id: &str, session_id: &str) -> AppResult<()>;
    async fn apply_import(
        &self,
        local_project_id: &str,
        session_id: &str,
    ) -> AppResult<InventoryApplyOutcome>;
}

pub async fn list_aio_nodes<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    query: ListAioNodesQuery,
) -> AppResult<AioNodeListPage> {
    port.list_nodes(local_project_id, query).await
}

pub async fn get_aio_node_detail<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    mac: &str,
) -> AppResult<AioNodeDetail> {
    port.node_detail(local_project_id, mac).await
}

pub async fn preview_inventory_import<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    file_path: &Path,
) -> AppResult<InventoryPreview> {
    port.preview_import(local_project_id, file_path).await
}

pub async fn latest_inventory_import<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
) -> AppResult<Option<AioImportSession>> {
    port.latest_import(local_project_id).await
}

pub async fn update_inventory_selection<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    session_id: &str,
    selections: &[ImportSelection],
) -> AppResult<AioImportSession> {
    port.update_selection(local_project_id, session_id, selections)
        .await
}

pub async fn discard_inventory_import<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    session_id: &str,
) -> AppResult<()> {
    port.discard_import(local_project_id, session_id).await
}

pub async fn apply_inventory_import<P: AioAssetsPort>(
    port: &P,
    local_project_id: &str,
    session_id: &str,
) -> AppResult<InventoryApplyOutcome> {
    port.apply_import(local_project_id, session_id).await
}
