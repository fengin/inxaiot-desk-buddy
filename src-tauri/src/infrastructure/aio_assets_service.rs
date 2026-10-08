use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use crate::infrastructure::client_instance::application_instance_id;
use sqlx::Row;
use time::OffsetDateTime;

use crate::application::aio_assets::{
    AioAssetsPort, AioNodeDetail, AioNodeListItem, AioNodeListPage, AioNodeStats,
    InventoryApplyOutcome, InventoryPreview, ListAioNodesQuery, LocalCheckRecord,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::assets::{
    AioImportSession, ImportSelection, OperationRecordSummary, ServiceVersionRecord,
};
use crate::domain::aio::inventory::{
    FieldConflict, ImportClassification, InventoryValues, ParsedInventoryRow, PlatformNodeSnapshot,
    ReconciledImportItem, WorkbenchNodeSnapshot, reconcile_inventory, validate_inventory_values,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::service_check::NodeServiceCheckSnapshot;
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::csv_inventory::parse_inventory_path;
use crate::infrastructure::local_sqlite::aio_import_repository::AioImportRepository;
use crate::infrastructure::platform_aio::PlatformAioRepository;
use crate::infrastructure::project_context::project_aio_database as project_database;
use crate::infrastructure::service_check_repository::ServiceCheckRepository;
use crate::infrastructure::workbench_aio::{
    ApplyInventoryWrite, InventoryAssetWrite, WorkbenchAioRepository,
};

pub struct AioAssetsService<'a> {
    state: &'a FormalAppState,
}

impl<'a> AioAssetsService<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl AioAssetsPort for AioAssetsService<'_> {
    async fn list_nodes(
        &self,
        local_project_id: &str,
        query: ListAioNodesQuery,
    ) -> AppResult<AioNodeListPage> {
        list_aio_nodes(self.state, local_project_id, query).await
    }

    async fn node_detail(&self, local_project_id: &str, mac: &str) -> AppResult<AioNodeDetail> {
        get_aio_node_detail(self.state, local_project_id, mac).await
    }

    async fn preview_import(
        &self,
        local_project_id: &str,
        file_path: &Path,
    ) -> AppResult<InventoryPreview> {
        preview_inventory_import(self.state, local_project_id, file_path).await
    }

    async fn latest_import(&self, local_project_id: &str) -> AppResult<Option<AioImportSession>> {
        latest_inventory_import(self.state, local_project_id).await
    }

    async fn preview_create(
        &self,
        local_project_id: &str,
        values: InventoryValues,
    ) -> AppResult<InventoryPreview> {
        preview_aio_node_create(self.state, local_project_id, values).await
    }

    async fn update_selection(
        &self,
        local_project_id: &str,
        session_id: &str,
        selections: &[ImportSelection],
    ) -> AppResult<AioImportSession> {
        update_inventory_selection(self.state, local_project_id, session_id, selections).await
    }

    async fn discard_import(&self, local_project_id: &str, session_id: &str) -> AppResult<()> {
        discard_inventory_import(self.state, local_project_id, session_id).await
    }

    async fn apply_import(
        &self,
        local_project_id: &str,
        session_id: &str,
    ) -> AppResult<InventoryApplyOutcome> {
        apply_inventory_import(self.state, local_project_id, session_id).await
    }
}

pub async fn list_aio_nodes(
    state: &FormalAppState,
    local_project_id: &str,
    query: ListAioNodesQuery,
) -> AppResult<AioNodeListPage> {
    let page = query.page.max(1);
    let page_size = query.page_size.clamp(1, 100);
    let pools = project_database(state, local_project_id).await?;
    let workbench_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let check_repository =
        ServiceCheckRepository::new(state.local_store.pool().clone(), local_project_id);
    let (workbench, platform, versions, operations, checks) = tokio::try_join!(
        workbench_repository.list_snapshots(),
        platform_repository.list_all(),
        workbench_repository.list_service_versions(),
        workbench_repository.list_last_operations(),
        check_repository.list(),
    )?;
    let mut items = build_node_items(&workbench, &platform.nodes, &versions, &operations, &checks);
    let stats = calculate_stats(&items);
    let keyword = query.search.unwrap_or_default().trim().to_lowercase();
    let state_filter = query.state.unwrap_or_else(|| "all".into());
    items.retain(|item| {
        let keyword_matches = keyword.is_empty()
            || [&item.name, &item.ip, &item.mac, &item.location]
                .iter()
                .any(|value| value.to_lowercase().contains(&keyword));
        let state_matches = state_filter == "all"
            || item.management_state == state_filter
            || item.platform_state == state_filter;
        keyword_matches && state_matches
    });
    let total = u32::try_from(items.len()).unwrap_or(u32::MAX);
    let start = usize::try_from((page - 1).saturating_mul(page_size)).unwrap_or(usize::MAX);
    let paged = if start >= items.len() {
        Vec::new()
    } else {
        items
            .into_iter()
            .skip(start)
            .take(usize::try_from(page_size).unwrap_or(100))
            .collect()
    };
    let import_repository = AioImportRepository::new(state.local_store.pool().clone());
    let latest_import_session_id = import_repository
        .latest_open_for_project(local_project_id)
        .await?
        .map(|session| session.id);
    Ok(AioNodeListPage {
        items: paged,
        total,
        page,
        page_size,
        stats,
        platform_issues: platform.issues,
        latest_import_session_id,
        refreshed_at: timestamp(),
    })
}

pub async fn get_aio_node_detail(
    state: &FormalAppState,
    local_project_id: &str,
    mac: &str,
) -> AppResult<AioNodeDetail> {
    let mac = MacAddress::parse(mac)?;
    let pools = project_database(state, local_project_id).await?;
    let workbench_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let check_repository =
        ServiceCheckRepository::new(state.local_store.pool().clone(), local_project_id);
    let (workbench, platform, versions, operations, check) = tokio::try_join!(
        workbench_repository.list_snapshots(),
        platform_repository.list_all(),
        workbench_repository.list_service_versions(),
        workbench_repository.list_last_operations(),
        check_repository.get(mac.normalized()),
    )?;
    let service_checks = check
        .into_iter()
        .map(|check| (mac.normalized().to_string(), check))
        .collect();
    let node = build_node_items(
        &workbench,
        &platform.nodes,
        &versions,
        &operations,
        &service_checks,
    )
    .into_iter()
    .find(|item| item.mac_normalized == mac.normalized())
    .ok_or_else(|| AppError::NotFound(format!("一体机不存在：{}", mac.display())))?;
    let platform_node = platform
        .nodes
        .into_iter()
        .find(|item| item.mac_normalized == mac.normalized());
    let node_versions = node.versions.clone();
    let last_operation = operations.get(mac.normalized()).cloned();
    let checks = latest_local_checks(state, local_project_id, mac.normalized()).await?;
    Ok(AioNodeDetail {
        node,
        platform: platform_node,
        versions: node_versions,
        last_operation,
        latest_ssh_check: checks
            .iter()
            .find(|check| check.step_code.to_ascii_lowercase().contains("ssh"))
            .cloned(),
    })
}

pub async fn preview_inventory_import(
    state: &FormalAppState,
    local_project_id: &str,
    file_path: &Path,
) -> AppResult<InventoryPreview> {
    let repository = AioImportRepository::new(state.local_store.pool().clone());
    if repository
        .latest_open_for_project(local_project_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "当前项目已有未处理导入预览，请继续处理或先放弃".into(),
        ));
    }
    let rows = parse_inventory_path(file_path)?;
    let pools = project_database(state, local_project_id).await?;
    let workbench_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (workbench, platform) = tokio::try_join!(
        workbench_repository.list_snapshots(),
        platform_repository.list_all(),
    )?;
    let items = reconcile_inventory(rows, &workbench, &platform.nodes);
    let file_name = file_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| AppError::InvalidConfig("无法读取导入文件名".into()))?;
    let session = repository
        .create_preview(
            local_project_id,
            file_name,
            &file_path.to_string_lossy(),
            &items,
        )
        .await?;
    Ok(InventoryPreview {
        session,
        platform_issues: platform.issues,
    })
}

pub async fn latest_inventory_import(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<Option<AioImportSession>> {
    AioImportRepository::new(state.local_store.pool().clone())
        .latest_open_for_project(local_project_id)
        .await
}

pub async fn preview_aio_node_create(
    state: &FormalAppState,
    local_project_id: &str,
    values: InventoryValues,
) -> AppResult<InventoryPreview> {
    let repository = AioImportRepository::new(state.local_store.pool().clone());
    if repository.latest_open_for_project(local_project_id).await?.is_some() {
        return Err(AppError::Conflict("当前项目有未处理的导入预览，请先处理或放弃后再新增一体机".into()));
    }
    let row = validate_inventory_values(1, values);
    if !row.errors.is_empty() {
        return Err(AppError::InvalidConfig(row.errors.join("；")));
    }
    let pools = project_database(state, local_project_id).await?;
    let workbench_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (workbench, platform) = tokio::try_join!(
        workbench_repository.list_snapshots(),
        platform_repository.list_all(),
    )?;
    let items = prepare_node_create(row, &workbench, &platform.nodes)?;
    // 与 CSV 共用一个未处理预览约束；创建失败也不会替换原有预览。
    let session = repository.create_preview(local_project_id, "单台新增一体机", "manual-entry", &items).await?;
    Ok(InventoryPreview { session, platform_issues: platform.issues })
}

fn prepare_node_create(
    row: ParsedInventoryRow,
    workbench: &[WorkbenchNodeSnapshot],
    platform: &[PlatformNodeSnapshot],
) -> AppResult<Vec<ReconciledImportItem>> {
    let items = reconcile_inventory(vec![row], workbench, platform);
    let item = &items[0];
    match item.classification {
        ImportClassification::NewPending => {}
        ImportClassification::PlatformExisting => {
            if let Some(current) = platform.iter().find(|node| Some(&node.id) == item.platform_aio_id.as_ref())
                && (current.name.trim() != item.values.name || current.ip.trim() != item.values.ip)
            {
                return Err(AppError::Conflict(format!(
                    "该 MAC 已在平台登记为“{}”（{}），请核对名称和 IP 后再接管，不能通过新增修改已有记录",
                    current.name, current.ip
                )));
            }
        }
        ImportClassification::ExistingUnchanged | ImportClassification::ExistingChanged => {
            return Err(AppError::Conflict("该 MAC 对应的一体机已在工作台中，无需重复新增；如需修改，请使用清单导入核对变更".into()));
        }
        ImportClassification::Conflict => {
            return Err(AppError::Conflict(item.conflicts.iter().map(|conflict| conflict.message.as_str()).collect::<Vec<_>>().join("；")));
        }
        ImportClassification::Invalid => return Err(AppError::InvalidConfig(item.errors.join("；"))),
    }
    Ok(items)
}

pub async fn update_inventory_selection(
    state: &FormalAppState,
    local_project_id: &str,
    session_id: &str,
    selections: &[ImportSelection],
) -> AppResult<AioImportSession> {
    let repository = AioImportRepository::new(state.local_store.pool().clone());
    let session = repository.get(session_id).await?;
    ensure_session_project(&session, local_project_id)?;
    repository.update_selection(session_id, selections).await
}

pub async fn discard_inventory_import(
    state: &FormalAppState,
    local_project_id: &str,
    session_id: &str,
) -> AppResult<()> {
    let repository = AioImportRepository::new(state.local_store.pool().clone());
    let session = repository.get(session_id).await?;
    ensure_session_project(&session, local_project_id)?;
    repository.discard(session_id).await
}

pub async fn apply_inventory_import(
    state: &FormalAppState,
    local_project_id: &str,
    session_id: &str,
) -> AppResult<InventoryApplyOutcome> {
    let local_repository = AioImportRepository::new(state.local_store.pool().clone());
    let session = local_repository.get(session_id).await?;
    ensure_session_project(&session, local_project_id)?;
    if session.state != "preview" {
        return Err(AppError::Conflict("导入会话已经结束".into()));
    }
    let selected = session
        .items
        .iter()
        .filter(|item| item.selected)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        return Err(AppError::InvalidConfig(
            "请至少选择一行可应用的一体机".into(),
        ));
    }
    let pools = project_database(state, local_project_id).await?;
    let workbench_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (current_workbench, current_platform) = tokio::try_join!(
        workbench_repository.list_snapshots(),
        platform_repository.list_all(),
    )?;
    let refreshed = reconcile_inventory(
        session
            .items
            .iter()
            .map(|item| ParsedInventoryRow {
                row_number: item.row_number,
                values: item.values.clone(),
                mac_normalized: item.mac_normalized.clone(),
                errors: item.errors.clone(),
            })
            .collect(),
        &current_workbench,
        &current_platform.nodes,
    );
    let refreshed_by_row = refreshed
        .iter()
        .map(|item| (item.row_number, item))
        .collect::<HashMap<_, _>>();
    let workbench_by_mac = current_workbench
        .iter()
        .map(|node| (node.mac_normalized.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut assets = Vec::with_capacity(selected.len());
    for original in selected {
        let item = refreshed_by_row.get(&original.row_number).ok_or_else(|| {
            AppError::Conflict(format!("第 {} 行在重新对账后不存在", original.row_number))
        })?;
        if !item.classification.can_apply() {
            return Err(AppError::Conflict(format!(
                "第 {} 行在预览后发生变化，请重新预览",
                item.row_number
            )));
        }
        if item.workbench_version != original.workbench_version
            || item.platform_aio_id != original.platform_aio_id
            || item.platform_fingerprint != original.platform_fingerprint
        {
            return Err(AppError::Conflict(format!(
                "第 {} 行对应的工作台或平台数据已变化，请重新预览",
                item.row_number
            )));
        }
        let mac = MacAddress::parse(
            item.mac_normalized
                .as_deref()
                .ok_or_else(|| AppError::InvalidConfig("导入行缺少规范化 MAC".into()))?,
        )?;
        let current = workbench_by_mac.get(mac.normalized()).copied();
        let (management_state, source) = asset_state(item, current);
        assets.push(InventoryAssetWrite {
            mac_normalized: mac.normalized().into(),
            display_mac: mac.display(),
            name: item.values.name.clone(),
            ip: item.values.ip.clone(),
            building_id: item.values.building_id.clone(),
            region_id: item.values.region_id.clone(),
            addr_alias: item.values.addr_alias.clone(),
            floor: item.values.floor.clone(),
            location: item.values.location.clone(),
            remark: item.values.remark.clone(),
            platform_aio_id: item.platform_aio_id.clone(),
            management_state,
            source,
            expected_version: item.workbench_version,
        });
    }
    let operator_name = project_operator(state, local_project_id).await?;
    let result = workbench_repository
        .apply_inventory(ApplyInventoryWrite {
            file_name: session.file_name.clone(),
            operator_name,
            instance_id: application_instance_id().into(),
            classification_counts: serde_json::to_value(&session.counts)
                .map_err(|_| AppError::InvalidConfig("导入统计无法序列化".into()))?,
            assets,
        })
        .await?;
    let local_session_finalized = match local_repository.mark_applied(session_id).await {
        Ok(_) => true,
        Err(error) => {
            tracing::error!(
                session_id,
                operation_id = result.operation_id,
                error = ?crate::core::log_safety::safe_error(&error),
                "project import committed but local session finalization failed"
            );
            false
        }
    };
    Ok(InventoryApplyOutcome {
        result,
        local_session_finalized,
    })
}

fn build_node_items(
    workbench_nodes: &[WorkbenchNodeSnapshot],
    platform_nodes: &[PlatformNodeSnapshot],
    versions: &[ServiceVersionRecord],
    operations: &HashMap<String, OperationRecordSummary>,
    checks: &HashMap<String, NodeServiceCheckSnapshot>,
) -> Vec<AioNodeListItem> {
    let workbench = workbench_nodes
        .iter()
        .map(|node| (node.mac_normalized.as_str(), node))
        .collect::<HashMap<_, _>>();
    let mut platform = HashMap::<&str, Vec<&PlatformNodeSnapshot>>::new();
    for node in platform_nodes {
        platform
            .entry(node.mac_normalized.as_str())
            .or_default()
            .push(node);
    }
    let mut versions_by_mac = HashMap::<&str, Vec<&ServiceVersionRecord>>::new();
    for version in versions {
        versions_by_mac
            .entry(version.mac_normalized.as_str())
            .or_default()
            .push(version);
    }
    let keys = workbench
        .keys()
        .copied()
        .chain(platform.keys().copied())
        .collect::<BTreeSet<_>>();
    let mut items = keys
        .into_iter()
        .map(|mac| {
            let workbench_node = workbench.get(mac).copied();
            let platform_matches = platform.get(mac).cloned().unwrap_or_default();
            let platform_node = platform_matches.first().copied();
            let conflicts = asset_conflicts(workbench_node, &platform_matches);
            let management_state = if !conflicts.is_empty() {
                "conflict".to_string()
            } else {
                workbench_node
                    .map(|node| node.management_state.clone())
                    .unwrap_or_else(|| "platform_existing".into())
            };
            let service_versions = versions_by_mac.get(mac).cloned().unwrap_or_default();
            let service_check = checks.get(mac).cloned();
            let (service_state, service_label) = service_check
                .as_ref()
                .map(NodeServiceCheckSnapshot::summary)
                .unwrap_or_else(|| ("unknown".into(), "暂无检查记录".into()));
            let platform_state = match platform_node.and_then(|node| node.status) {
                Some(1) => "online",
                Some(0) => "offline",
                _ => "unknown",
            }
            .to_string();
            let operation = operations.get(mac);
            AioNodeListItem {
                mac: MacAddress::parse(mac)
                    .map(|value| value.display())
                    .unwrap_or_else(|_| mac.into()),
                mac_normalized: mac.into(),
                name: workbench_node
                    .map(|node| node.name.clone())
                    .or_else(|| platform_node.map(|node| node.name.clone()))
                    .unwrap_or_default(),
                ip: workbench_node
                    .map(|node| node.ip.clone())
                    .or_else(|| platform_node.map(|node| node.ip.clone()))
                    .unwrap_or_default(),
                location: display_location(workbench_node, platform_node),
                deploy_label: deploy_label(&management_state).into(),
                management_state,
                platform_state,
                platform_updated_at: platform_node
                    .map(platform_updated_at)
                    .unwrap_or_else(|| "尚未注册".into()),
                service_state,
                service_label,
                last_operation: operation
                    .map(|operation| operation.operation_name.clone())
                    .unwrap_or_else(|| "无工作台历史".into()),
                last_operation_at: operation.and_then(|operation| operation.ended_at.clone()),
                platform_id: platform_node
                    .map(|node| node.id.clone())
                    .or_else(|| workbench_node.and_then(|node| node.platform_aio_id.clone())),
                source: workbench_node
                    .map(|node| node.source.clone())
                    .unwrap_or_else(|| "platform".into()),
                version: workbench_node.map(|node| node.version).unwrap_or(0),
                conflicts,
                // 旧 observed 字段曾由期望版本直接填充，不能再作为实测返回。
                // 真实镜像统一从 service_check.services 读取。
                versions: service_versions
                    .into_iter()
                    .map(|version| ServiceVersionRecord {
                        observed_image_name: None,
                        observed_version: None,
                        observed_at: None,
                        ..version.clone()
                    })
                    .collect(),
                service_check,
            }
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.mac_normalized.cmp(&right.mac_normalized))
    });
    items
}

fn asset_conflicts(
    workbench: Option<&WorkbenchNodeSnapshot>,
    platform: &[&PlatformNodeSnapshot],
) -> Vec<FieldConflict> {
    if platform.len() > 1 {
        return vec![FieldConflict {
            code: "PLATFORM_DUPLICATE_MAC".into(),
            field: "mac".into(),
            message: "平台存在多条相同规范化 MAC 记录".into(),
            import_value: None,
            workbench_value: workbench.map(|node| node.mac_normalized.clone()),
            platform_value: Some(
                platform
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect::<Vec<_>>()
                    .join("、"),
            ),
        }];
    }
    let Some(workbench) = workbench else {
        return Vec::new();
    };
    let Some(platform) = platform.first().copied() else {
        return workbench
            .platform_aio_id
            .as_ref()
            .map(|id| FieldConflict {
                code: "LINKED_PLATFORM_NODE_MISSING".into(),
                field: "platformAioId".into(),
                message: "工作台已关联平台对象，但平台当前无法按 MAC 找到".into(),
                import_value: None,
                workbench_value: Some(id.clone()),
                platform_value: None,
            })
            .into_iter()
            .collect();
    };
    let mut conflicts = Vec::new();
    if let Some(id) = &workbench.platform_aio_id
        && id != &platform.id
    {
        conflicts.push(FieldConflict {
            code: "PLATFORM_LINK_MISMATCH".into(),
            field: "platformAioId".into(),
            message: "工作台平台关联 ID 与 MAC 匹配对象不一致".into(),
            import_value: None,
            workbench_value: Some(id.clone()),
            platform_value: Some(platform.id.clone()),
        });
    }
    if workbench.name.trim() != platform.name.trim() {
        conflicts.push(FieldConflict {
            code: "WORKBENCH_PLATFORM_NAME_MISMATCH".into(),
            field: "name".into(),
            message: "工作台名称与平台名称不一致".into(),
            import_value: None,
            workbench_value: Some(workbench.name.clone()),
            platform_value: Some(platform.name.clone()),
        });
    }
    if workbench.ip.trim() != platform.ip.trim() {
        conflicts.push(FieldConflict {
            code: "WORKBENCH_PLATFORM_IP_MISMATCH".into(),
            field: "ip".into(),
            message: "工作台 IP 与平台 IP 不一致".into(),
            import_value: None,
            workbench_value: Some(workbench.ip.clone()),
            platform_value: Some(platform.ip.clone()),
        });
    }
    conflicts
}

fn calculate_stats(items: &[AioNodeListItem]) -> AioNodeStats {
    AioNodeStats {
        total: u32::try_from(items.len()).unwrap_or(u32::MAX),
        online: u32::try_from(
            items
                .iter()
                .filter(|item| item.platform_state == "online")
                .count(),
        )
        .unwrap_or(u32::MAX),
        offline: u32::try_from(
            items
                .iter()
                .filter(|item| item.platform_state == "offline")
                .count(),
        )
        .unwrap_or(u32::MAX),
        pending: u32::try_from(
            items
                .iter()
                .filter(|item| item.management_state == "pending")
                .count(),
        )
        .unwrap_or(u32::MAX),
        conflicts: u32::try_from(
            items
                .iter()
                .filter(|item| item.management_state == "conflict")
                .count(),
        )
        .unwrap_or(u32::MAX),
    }
}

fn deploy_label(state: &str) -> &'static str {
    match state {
        "managed" => "已管理",
        "pending" => "待实施",
        "platform_existing" => "平台已存在",
        "conflict" => "信息冲突",
        _ => "待确认",
    }
}

fn display_location(
    workbench: Option<&WorkbenchNodeSnapshot>,
    platform: Option<&PlatformNodeSnapshot>,
) -> String {
    workbench
        .and_then(|node| {
            node.location
                .clone()
                .or_else(|| node.addr_alias.clone())
                .or_else(|| node.floor.clone())
        })
        .or_else(|| platform.and_then(|node| node.addr_alias.clone()))
        .or_else(|| platform.and_then(|node| node.building_id.clone()))
        .unwrap_or_else(|| "未填写".into())
}

fn platform_updated_at(node: &PlatformNodeSnapshot) -> String {
    if let Some(value) = &node.last_sync_time {
        return value.clone();
    }
    let Some(raw) = node.last_beat_time else {
        return "无状态时间".into();
    };
    let seconds = if raw.unsigned_abs() > 10_000_000_000 {
        raw / 1000
    } else {
        raw
    };
    OffsetDateTime::from_unix_timestamp(seconds)
        .ok()
        .and_then(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| raw.to_string())
}

fn asset_state(
    item: &ReconciledImportItem,
    current: Option<&WorkbenchNodeSnapshot>,
) -> (String, String) {
    match item.classification {
        ImportClassification::NewPending => ("pending".into(), "import".into()),
        ImportClassification::PlatformExisting => ("platform_existing".into(), "merged".into()),
        ImportClassification::ExistingChanged => {
            if item.platform_aio_id.is_some() {
                (
                    current
                        .map(|node| node.management_state.clone())
                        .filter(|state| state == "managed")
                        .unwrap_or_else(|| "platform_existing".into()),
                    "merged".into(),
                )
            } else {
                (
                    current
                        .map(|node| node.management_state.clone())
                        .unwrap_or_else(|| "pending".into()),
                    current
                        .map(|node| node.source.clone())
                        .unwrap_or_else(|| "import".into()),
                )
            }
        }
        _ => ("pending".into(), "import".into()),
    }
}

fn ensure_session_project(session: &AioImportSession, local_project_id: &str) -> AppResult<()> {
    if session.local_project_id != local_project_id {
        return Err(AppError::Conflict("导入会话不属于当前项目".into()));
    }
    Ok(())
}

pub(crate) use crate::infrastructure::project_context::project_operator;
#[cfg(test)]
use crate::infrastructure::project_context::session_is_expired;

async fn latest_local_checks(
    state: &FormalAppState,
    local_project_id: &str,
    mac_normalized: &str,
) -> AppResult<Vec<LocalCheckRecord>> {
    let rows = sqlx::query(concat!(
        "SELECT s.step_code, s.state, s.message, s.updated_at FROM local_task_step s ",
        "JOIN local_task t ON t.id = s.local_task_id ",
        "WHERE t.local_project_id = ? AND s.resource_type = 'aio' AND s.resource_key = ? ",
        "ORDER BY s.updated_at DESC, s.id DESC LIMIT 50"
    ))
    .bind(local_project_id)
    .bind(mac_normalized)
    .fetch_all(state.local_store.pool())
    .await
    .map_err(|error| AppError::database("读取一体机本机检查记录", &error))?;
    rows.into_iter()
        .map(|row| -> AppResult<LocalCheckRecord> {
            Ok(LocalCheckRecord {
                step_code: row
                    .try_get("step_code")
                    .map_err(|error| AppError::database("解析本机检查步骤", &error))?,
                state: row
                    .try_get("state")
                    .map_err(|error| AppError::database("解析本机检查状态", &error))?,
                message: row
                    .try_get("message")
                    .map_err(|error| AppError::database("解析本机检查消息", &error))?,
                updated_at: row
                    .try_get("updated_at")
                    .map_err(|error| AppError::database("解析本机检查时间", &error))?,
            })
        })
        .collect::<AppResult<Vec<_>>>()
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 timestamp")
}

#[cfg(test)]
mod tests {
    use super::{asset_conflicts, build_node_items, platform_updated_at, prepare_node_create, session_is_expired};
    use crate::domain::aio::assets::ServiceVersionRecord;
    use crate::domain::aio::inventory::{ImportClassification, InventoryValues, PlatformNodeSnapshot, WorkbenchNodeSnapshot, validate_inventory_values};
    use std::collections::HashMap;

    fn workbench() -> WorkbenchNodeSnapshot {
        WorkbenchNodeSnapshot {
            mac_normalized: "001122334455".into(),
            name: "node".into(),
            ip: "192.0.2.1".into(),
            building_id: None,
            region_id: None,
            addr_alias: None,
            floor: None,
            location: None,
            remark: None,
            platform_aio_id: Some("1".into()),
            management_state: "managed".into(),
            source: "merged".into(),
            last_operation_id: None,
            version: 1,
        }
    }

    fn create_values() -> InventoryValues {
        InventoryValues { name: "node".into(), ip: "192.0.2.1".into(), mac: "00:11:22:33:44:55".into(), ..InventoryValues::default() }
    }

    #[test]
    fn manual_create_and_csv_share_field_validation() {
        let csv = crate::infrastructure::csv_inventory::parse_inventory_text("name,ip,mac\n node ,192.0.2.1,00:11:22:33:44:55\n").unwrap();
        let mut values = create_values();
        values.name = " node ".into();
        assert_eq!(validate_inventory_values(2, values), csv[0]);
        let mut invalid = create_values();
        invalid.name = " ".into(); invalid.ip = "192.0.2.999".into(); invalid.mac = "invalid".into();
        assert_eq!(validate_inventory_values(1, invalid).errors.len(), 3);
    }

    #[test]
    fn manual_create_only_allows_new_node_or_matching_platform_takeover() {
        let row = || validate_inventory_values(1, create_values());
        assert_eq!(prepare_node_create(row(), &[], &[]).unwrap()[0].classification, ImportClassification::NewPending);
        assert_eq!(prepare_node_create(row(), &[], &[platform()]).unwrap()[0].classification, ImportClassification::PlatformExisting);
        assert!(prepare_node_create(row(), &[workbench()], &[platform()]).is_err());
        let mut changed = create_values(); changed.name = "changed".into();
        assert!(prepare_node_create(validate_inventory_values(1, changed.clone()), &[workbench()], &[platform()]).is_err());
        assert!(prepare_node_create(validate_inventory_values(1, changed), &[], &[platform()]).is_err());
        assert!(prepare_node_create(row(), &[], &[platform(), platform()]).is_err());
    }

    #[test]
    fn project_session_expiration_accepts_epoch_and_rfc3339() {
        assert!(session_is_expired("1"));
        assert!(session_is_expired("2000-01-01T00:00:00Z"));
        assert!(!session_is_expired("4102444800"));
    }

    fn platform() -> PlatformNodeSnapshot {
        PlatformNodeSnapshot {
            id: "1".into(),
            name: "node".into(),
            ip: "192.0.2.1".into(),
            mac_raw: "00:11:22:33:44:55".into(),
            mac_normalized: "001122334455".into(),
            building_id: None,
            addr_alias: None,
            status: Some(1),
            last_beat_time: Some(1_787_830_000_000),
            last_sync_time: None,
        }
    }

    #[test]
    fn list_conflicts_are_explicit() {
        let workbench = workbench();
        let mut platform = platform();
        assert!(asset_conflicts(Some(&workbench), &[&platform]).is_empty());
        platform.ip = "192.0.2.2".into();
        assert_eq!(asset_conflicts(Some(&workbench), &[&platform]).len(), 1);
    }

    #[test]
    fn legacy_observed_versions_cannot_claim_a_real_service_check() {
        let workbench = workbench();
        let nodes = build_node_items(
            std::slice::from_ref(&workbench),
            &[platform()],
            &[ServiceVersionRecord {
                mac_normalized: workbench.mac_normalized.clone(),
                service_name: "device-edge".into(),
                expected_image_name: None,
                expected_version: Some("1".into()),
                observed_image_name: None,
                observed_version: Some("1".into()),
                observed_at: Some("2099-01-01T00:00:00Z".into()),
            }],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(nodes[0].service_state, "unknown");
        assert_eq!(nodes[0].service_label, "暂无检查记录");
        assert!(nodes[0].service_check.is_none());
        assert!(nodes[0].versions[0].observed_version.is_none());
        assert!(nodes[0].versions[0].observed_at.is_none());
        assert_eq!(nodes[0].versions[0].expected_version.as_deref(), Some("1"));
    }

    #[test]
    fn heartbeat_accepts_millisecond_epoch() {
        let value = platform_updated_at(&platform());
        assert!(value.contains('T'));
    }
}
