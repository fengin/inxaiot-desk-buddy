use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use sqlx::Row;
use time::OffsetDateTime;

use crate::application::aio_assets::{
    AioAssetsPort, AioNodeDetail, AioNodeListItem, AioNodeListPage, AioNodeStats,
    InventoryApplyOutcome, InventoryPreview, ListAioNodesQuery, LocalCheckRecord,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::assets::{
    AioImportSession, ImportSelection, OperationRecordSummary, PlatformRecordIssue,
    ServiceVersionRecord,
};
use crate::domain::aio::inventory::{
    FieldConflict, ImportClassification, InventoryValues, ParsedInventoryRow, PlatformNodeSnapshot,
    ReconciledImportItem, WorkbenchNodeSnapshot, reconcile_inventory, validate_inventory_values,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::service_check::NodeServiceCheckSnapshot;
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::aio_inventory_source::merge_sources;
use crate::infrastructure::csv_inventory::parse_inventory_path;
use crate::infrastructure::local_sqlite::aio_import_repository::AioImportRepository;
use crate::infrastructure::local_sqlite::aio_node_repository::LocalAioRepository;
use crate::infrastructure::platform_aio::PlatformAioRepository;
use crate::infrastructure::project_context::project_pools as project_database;
use crate::infrastructure::service_check_repository::ServiceCheckRepository;
use crate::infrastructure::workbench_aio::WorkbenchAioRepository;

pub struct AioAssetsService<'a> {
    state: &'a FormalAppState,
}

impl<'a> AioAssetsService<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl AioAssetsPort for AioAssetsService<'_> {
    async fn update_node(
        &self,
        project: &str,
        input: crate::application::aio_assets::UpdateAioNodeInput,
    ) -> AppResult<()> {
        crate::infrastructure::aio_edit::update(self.state, project, input).await
    }
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
    let ((workbench, versions, operations, metadata_warning), platform, checks) = tokio::try_join!(
        async { Ok::<_, AppError>(shared_metadata(&workbench_repository).await) },
        platform_repository.list_all(),
        check_repository.list(),
    )?;
    let local = LocalAioRepository::new(state.local_store.pool().clone())
        .list(local_project_id)
        .await?;
    crate::infrastructure::aio_inventory_source::retire_registered(
        state,
        local_project_id,
        &local,
        &platform.nodes,
    )
    .await?;
    let workbench = merge_sources(&local, &workbench, &platform.nodes);
    let mut items = build_node_items(&workbench, &platform.nodes, &versions, &operations, &checks);
    apply_space_labels(&mut items, &pools.platform).await;
    let stats = calculate_stats(&items, &platform.issues);
    let keyword = query.search.unwrap_or_default().trim().to_lowercase();
    let state_filter = query.state.unwrap_or_else(|| "all".into());
    let (paged, page_platform_issues, total) = filter_node_page(
        items,
        &platform.issues,
        &keyword,
        &state_filter,
        page,
        page_size,
    );
    let import_repository = AioImportRepository::new(state.local_store.pool().clone());
    let latest_import_session_id = import_repository
        .latest_open_for_project(local_project_id)
        .await?
        .map(|session| session.id);
    Ok(AioNodeListPage {
        metadata_warning,
        items: paged,
        total,
        page,
        page_size,
        stats,
        platform_issues: platform.issues,
        page_platform_issues,
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
    let ((workbench, versions, operations, metadata_warning), platform, check) = tokio::try_join!(
        async { Ok::<_, AppError>(shared_metadata(&workbench_repository).await) },
        platform_repository.list_all(),
        check_repository.get(mac.normalized()),
    )?;
    let local = LocalAioRepository::new(state.local_store.pool().clone())
        .list(local_project_id)
        .await?;
    crate::infrastructure::aio_inventory_source::retire_registered(
        state,
        local_project_id,
        &local,
        &platform.nodes,
    )
    .await?;
    let workbench = merge_sources(&local, &workbench, &platform.nodes);
    let service_checks = check
        .into_iter()
        .map(|check| (mac.normalized().to_string(), check))
        .collect();
    let mut node = build_node_items(
        &workbench,
        &platform.nodes,
        &versions,
        &operations,
        &service_checks,
    )
    .into_iter()
    .find(|item| item.mac_normalized == mac.normalized())
    .ok_or_else(|| AppError::NotFound(format!("一体机不存在：{}", mac.display())))?;
    apply_space_labels(std::slice::from_mut(&mut node), &pools.platform).await;
    let platform_node = platform
        .nodes
        .into_iter()
        .find(|item| item.mac_normalized == mac.normalized());
    let node_versions = node.versions.clone();
    let last_operation = operations.get(mac.normalized()).cloned();
    let checks = latest_local_checks(state, local_project_id, mac.normalized()).await?;
    Ok(AioNodeDetail {
        metadata_warning,
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

async fn shared_metadata(
    repository: &WorkbenchAioRepository,
) -> (
    Vec<WorkbenchNodeSnapshot>,
    Vec<ServiceVersionRecord>,
    HashMap<String, OperationRecordSummary>,
    Option<String>,
) {
    match tokio::try_join!(repository.list_snapshots(), repository.list_service_versions(), repository.list_last_operations()) {
        Ok((nodes, versions, operations)) => (nodes, versions, operations, None),
        Err(_) => (Vec::new(), Vec::new(), HashMap::new(), Some("工作台部署记录暂未读取，当前显示本机清单和平台资料；请检查项目连接或工作台数据库结构。".into())),
    }
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
    let mut rows = parse_inventory_path(file_path)?;
    let pools = project_database(state, local_project_id).await?;
    resolve_import_spaces(&pools.platform, &mut rows).await?;
    let workbench_repository = LocalAioRepository::new(state.local_store.pool().clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (workbench, platform) = tokio::try_join!(
        workbench_repository.list(local_project_id),
        platform_repository.list_all(),
    )?;
    let items = reconcile_local_import(rows, &workbench, &platform.nodes);
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
    if repository
        .latest_open_for_project(local_project_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "当前项目有未处理的导入预览，请先处理或放弃后再新增一体机".into(),
        ));
    }
    let mut row = validate_inventory_values(1, values);
    if !row.errors.is_empty() {
        return Err(AppError::InvalidConfig(row.errors.join("；")));
    }
    let pools = project_database(state, local_project_id).await?;
    resolve_import_spaces(&pools.platform, std::slice::from_mut(&mut row)).await?;
    if !row.errors.is_empty() {
        return Err(AppError::InvalidConfig(row.errors.join("；")));
    }
    let workbench_repository = LocalAioRepository::new(state.local_store.pool().clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (workbench, platform) = tokio::try_join!(
        workbench_repository.list(local_project_id),
        platform_repository.list_all(),
    )?;
    let items = prepare_node_create(row, &workbench, &platform.nodes)?;
    // 与 CSV 共用一个未处理预览约束；创建失败也不会替换原有预览。
    let session = repository
        .create_preview(local_project_id, "单台新增一体机", "manual-entry", &items)
        .await?;
    Ok(InventoryPreview {
        session,
        platform_issues: platform.issues,
    })
}

async fn resolve_import_spaces(
    pool: &sqlx::MySqlPool,
    rows: &mut [ParsedInventoryRow],
) -> AppResult<()> {
    let needs_directory = rows.iter().any(|row| {
        row.values.space_path.is_some()
            || row
                .values
                .building_id
                .as_deref()
                .is_some_and(|id| !id.is_empty() && id != "0")
    });
    let spaces = if needs_directory {
        crate::infrastructure::project_spaces::read(pool, None).await?
    } else {
        Vec::new()
    };
    let directory = crate::domain::aio::space::InventorySpaceDirectory::new(&spaces);
    for row in rows {
        if let Err(error) =
            crate::domain::aio::space::resolve_inventory_space_in_directory(&mut row.values, &directory)
        {
            row.errors
                .push(format!("第 {} 行：{}", row.row_number, error));
        }
    }
    Ok(())
}

fn prepare_node_create(
    row: ParsedInventoryRow,
    workbench: &[WorkbenchNodeSnapshot],
    platform: &[PlatformNodeSnapshot],
) -> AppResult<Vec<ReconciledImportItem>> {
    if platform
        .iter()
        .any(|node| Some(&node.mac_normalized) == row.mac_normalized.as_ref())
    {
        return Err(AppError::Conflict(
            "该一体机已在平台注册，无需重复新增；请在详情中编辑平台资料".into(),
        ));
    }
    let items = reconcile_local_import(vec![row], workbench, platform);
    let item = &items[0];
    match item.classification {
        ImportClassification::NewPending => {}
        ImportClassification::PlatformExisting => {
            if let Some(current) = platform
                .iter()
                .find(|node| Some(&node.id) == item.platform_aio_id.as_ref())
                && (current.name.trim() != item.values.name || current.ip.trim() != item.values.ip)
            {
                return Err(AppError::Conflict(format!(
                    "该 MAC 已在平台登记为“{}”（{}），请核对名称和 IP 后再接管，不能通过新增修改已有记录",
                    current.name, current.ip
                )));
            }
        }
        ImportClassification::ExistingUnchanged | ImportClassification::ExistingChanged => {
            return Err(AppError::Conflict(
                "该 MAC 对应的一体机已在工作台中，无需重复新增；如需修改，请使用清单导入核对变更"
                    .into(),
            ));
        }
        ImportClassification::Conflict => {
            return Err(AppError::Conflict(
                item.conflicts
                    .iter()
                    .map(|conflict| conflict.message.as_str())
                    .collect::<Vec<_>>()
                    .join("；"),
            ));
        }
        ImportClassification::Invalid => {
            return Err(AppError::InvalidConfig(item.errors.join("；")));
        }
    }
    Ok(items)
}

fn reconcile_local_import(
    rows: Vec<ParsedInventoryRow>,
    local: &[WorkbenchNodeSnapshot],
    platform: &[PlatformNodeSnapshot],
) -> Vec<ReconciledImportItem> {
    let mut items = reconcile_inventory(rows, local, platform);
    for item in &mut items {
        if item.platform_aio_id.is_some() && item.errors.is_empty() {
            item.classification = ImportClassification::ExistingUnchanged;
            item.selected = false;
        }
    }
    items
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
    let sessions = AioImportRepository::new(state.local_store.pool().clone());
    let session = sessions.get(session_id).await?;
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
    let local = LocalAioRepository::new(state.local_store.pool().clone());
    let current = local.list(local_project_id).await?;
    let platform = PlatformAioRepository::new(pools.platform.clone())
        .list_all()
        .await?;
    let mut rows = selected
        .iter()
        .map(|item| ParsedInventoryRow {
            row_number: item.row_number,
            values: item.values.clone(),
            mac_normalized: item.mac_normalized.clone(),
            errors: item.errors.clone(),
        })
        .collect::<Vec<_>>();
    resolve_import_spaces(&pools.platform, &mut rows).await?;
    let checked = reconcile_local_import(rows, &current, &platform.nodes);
    let mut records = Vec::new();
    for item in checked {
        let original = selected
            .iter()
            .find(|old| old.row_number == item.row_number)
            .ok_or_else(|| AppError::Conflict("导入预览已经变化，请重新选择".into()))?;
        if !matches!(
            item.classification,
            ImportClassification::NewPending | ImportClassification::ExistingChanged
        ) || item.workbench_version != original.workbench_version
            || item.platform_aio_id.is_some()
        {
            return Err(AppError::Conflict(format!(
                "第 {} 行资料或注册状态已变化，请重新预览；已注册一体机请在详情中编辑",
                item.row_number
            )));
        }
        records.push((item.values, item.workbench_version));
    }
    local
        .apply_import(local_project_id, session_id, &records)
        .await?;
    Ok(InventoryApplyOutcome {
        result: crate::domain::aio::assets::InventoryApplyResult {
            operation_id: session_id.into(),
            applied_count: records.len() as u32,
        },
        local_session_finalized: true,
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
            let deployment_state = if management_state == "conflict" {
                "attention"
            } else if platform_node.is_some() {
                "deployed"
            } else if management_state == "pending" {
                "pending"
            } else {
                "unconfirmed"
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
                space_path: String::new(),
                building_id: platform_node
                    .and_then(|node| node.building_id.clone())
                    .or_else(|| workbench_node.and_then(|node| node.building_id.clone()))
                    .filter(|id| !id.is_empty() && id != "0"),
                deploy_label: deploy_label(deployment_state).into(),
                deployment_state: deployment_state.into(),
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

async fn apply_space_labels(items: &mut [AioNodeListItem], pool: &sqlx::MySqlPool) {
    if !items.iter().any(|item| item.building_id.is_some()) {
        return;
    }
    // 目录暂不可用不影响查看资产；实际选空间和保存仍需实时校验。
    if let Ok(spaces) = crate::infrastructure::project_spaces::read(pool, None).await {
        for item in items {
            if let Some(path) = item
                .building_id
                .as_deref()
                .and_then(|id| crate::domain::common::project_space::path_text(&spaces, id))
            {
                item.space_path = path;
            }
        }
    }
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

fn filter_node_page(
    items: Vec<AioNodeListItem>,
    platform_issues: &[PlatformRecordIssue],
    keyword: &str,
    state_filter: &str,
    page: u32,
    page_size: u32,
) -> (Vec<AioNodeListItem>, Vec<PlatformRecordIssue>, u32) {
    let nodes = items
        .into_iter()
        .filter(|item| {
            let keyword_matches = keyword.is_empty()
                || [
                    &item.name,
                    &item.ip,
                    &item.mac,
                    &item.location,
                    &item.space_path,
                ]
                .iter()
                .any(|value| value.to_lowercase().contains(keyword));
            let state_matches = state_filter == "all"
                || item.deployment_state == state_filter
                || item.platform_state == state_filter;
            keyword_matches && state_matches
        })
        .collect::<Vec<_>>();
    // 无法确认 MAC 身份的平台资料仅在待处理页合并，不能进入部署目标集合。
    let mut issues = platform_issues
        .iter()
        .filter(|issue| {
            state_filter == "attention"
                && (keyword.is_empty()
                    || [
                        &issue.name,
                        &issue.ip,
                        &issue.raw_mac,
                        &issue.platform_aio_id,
                        &issue.message,
                    ]
                    .iter()
                    .any(|value| value.to_lowercase().contains(keyword)))
        })
        .cloned()
        .collect::<Vec<_>>();
    issues.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.platform_aio_id.cmp(&right.platform_aio_id))
    });
    let total = u32::try_from(nodes.len().saturating_add(issues.len())).unwrap_or(u32::MAX);
    let start =
        usize::try_from(page.saturating_sub(1).saturating_mul(page_size)).unwrap_or(usize::MAX);
    let limit = usize::try_from(page_size).unwrap_or(100);
    let issue_start = start.saturating_sub(nodes.len());
    let paged = nodes
        .into_iter()
        .skip(start)
        .take(limit)
        .collect::<Vec<_>>();
    let paged_issues = issues
        .into_iter()
        .skip(issue_start)
        .take(limit.saturating_sub(paged.len()))
        .collect();
    (paged, paged_issues, total)
}

fn calculate_stats(items: &[AioNodeListItem], issues: &[PlatformRecordIssue]) -> AioNodeStats {
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
                .filter(|item| item.deployment_state == "pending")
                .count(),
        )
        .unwrap_or(u32::MAX),
        deployed: u32::try_from(
            items
                .iter()
                .filter(|item| item.deployment_state == "deployed")
                .count(),
        )
        .unwrap_or(u32::MAX),
        attention: u32::try_from(
            items
                .iter()
                .filter(|item| item.deployment_state == "attention")
                .count()
                .saturating_add(issues.len()),
        )
        .unwrap_or(u32::MAX),
        unconfirmed: u32::try_from(
            items
                .iter()
                .filter(|item| item.deployment_state == "unconfirmed")
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
        "deployed" => "已部署",
        "pending" => "待实施",
        "attention" => "待处理",
        _ => "待确认",
    }
}

fn display_location(
    workbench: Option<&WorkbenchNodeSnapshot>,
    platform: Option<&PlatformNodeSnapshot>,
) -> String {
    workbench
        .and_then(|node| node.location.clone().or_else(|| node.addr_alias.clone()))
        .or_else(|| platform.and_then(|node| node.addr_alias.clone()))
        .unwrap_or_default()
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
    use super::{
        asset_conflicts, build_node_items, calculate_stats, filter_node_page, platform_updated_at,
        prepare_node_create, session_is_expired,
    };
    use crate::domain::aio::assets::{PlatformRecordIssue, ServiceVersionRecord};
    use crate::domain::aio::inventory::{
        ImportClassification, InventoryValues, PlatformNodeSnapshot, WorkbenchNodeSnapshot,
        validate_inventory_values,
    };
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
        InventoryValues {
            name: "node".into(),
            ip: "192.0.2.1".into(),
            mac: "00:11:22:33:44:55".into(),
            ..InventoryValues::default()
        }
    }

    #[test]
    fn manual_create_and_csv_share_field_validation() {
        let csv = crate::infrastructure::csv_inventory::parse_inventory_text(
            "name,ip,mac\n node ,192.0.2.1,00:11:22:33:44:55\n",
        )
        .unwrap();
        let mut values = create_values();
        values.name = " node ".into();
        assert_eq!(validate_inventory_values(2, values), csv[0]);
        let mut invalid = create_values();
        invalid.name = " ".into();
        invalid.ip = "192.0.2.999".into();
        invalid.mac = "invalid".into();
        assert_eq!(validate_inventory_values(1, invalid).errors.len(), 3);
    }

    #[test]
    fn manual_create_only_allows_unregistered_local_nodes() {
        let row = || validate_inventory_values(1, create_values());
        assert_eq!(
            prepare_node_create(row(), &[], &[]).unwrap()[0].classification,
            ImportClassification::NewPending
        );
        assert!(prepare_node_create(row(), &[], &[platform()]).is_err());
        assert!(prepare_node_create(row(), &[workbench()], &[platform()]).is_err());
        let mut changed = create_values();
        changed.name = "changed".into();
        assert!(
            prepare_node_create(
                validate_inventory_values(1, changed.clone()),
                &[workbench()],
                &[platform()]
            )
            .is_err()
        );
        assert!(
            prepare_node_create(validate_inventory_values(1, changed), &[], &[platform()]).is_err()
        );
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
    fn platform_registration_defines_deployment_independent_of_shared_history() {
        for (shared, expected_management) in [
            (vec![workbench()], "managed"),
            (Vec::new(), "platform_existing"),
        ] {
            let platform = vec![platform()];
            let merged =
                crate::infrastructure::aio_inventory_source::merge_sources(&[], &shared, &platform);
            let nodes = build_node_items(&merged, &platform, &[], &HashMap::new(), &HashMap::new());
            assert_eq!(nodes[0].management_state, expected_management);
            assert_eq!(nodes[0].deployment_state, "deployed");
            assert_eq!(nodes[0].deploy_label, "已部署");
            let dto = serde_json::to_value(&nodes[0]).unwrap();
            assert_eq!(dto["deploymentState"], "deployed");
            assert_eq!(dto["managementState"], expected_management);
        }
    }

    #[test]
    fn deployment_projection_distinguishes_pending_unknown_and_conflict() {
        let mut local = workbench();
        local.platform_aio_id = None;
        for (raw, expected, label) in [
            ("pending", "pending", "待实施"),
            ("managed", "unconfirmed", "待确认"),
            ("unexpected", "unconfirmed", "待确认"),
            ("conflict", "attention", "待处理"),
        ] {
            local.management_state = raw.into();
            let nodes = build_node_items(
                std::slice::from_ref(&local),
                &[],
                &[],
                &HashMap::new(),
                &HashMap::new(),
            );
            assert_eq!(nodes[0].management_state, raw);
            assert_eq!(nodes[0].deployment_state, expected);
            assert_eq!(nodes[0].deploy_label, label);
        }
        let duplicate = build_node_items(
            &[],
            &[platform(), platform()],
            &[],
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(duplicate[0].deployment_state, "attention");
        assert_eq!(duplicate[0].conflicts[0].code, "PLATFORM_DUPLICATE_MAC");
        let missing = build_node_items(&[workbench()], &[], &[], &HashMap::new(), &HashMap::new());
        assert_eq!(missing[0].deployment_state, "attention");
        assert_eq!(missing[0].conflicts[0].code, "LINKED_PLATFORM_NODE_MISSING");
    }

    fn issue(id: &str, name: &str) -> PlatformRecordIssue {
        PlatformRecordIssue {
            platform_aio_id: id.into(),
            name: name.into(),
            ip: "192.0.2.9".into(),
            raw_mac: "bad-mac".into(),
            code: "PLATFORM_MAC_INVALID".into(),
            message: "MAC 地址格式无效".into(),
        }
    }

    #[test]
    fn attention_filter_paginates_nodes_and_platform_issues_without_polluting_selection() {
        let mut local = workbench();
        local.platform_aio_id = None;
        local.management_state = "pending".into();
        let mut pending =
            build_node_items(&[local], &[], &[], &HashMap::new(), &HashMap::new()).remove(0);
        pending.name = "pending".into();
        let mut deployed =
            build_node_items(&[], &[platform()], &[], &HashMap::new(), &HashMap::new()).remove(0);
        deployed.name = "deployed".into();
        let mut conflict_a = build_node_items(
            &[],
            &[platform(), platform()],
            &[],
            &HashMap::new(),
            &HashMap::new(),
        )
        .remove(0);
        conflict_a.name = "attention-a".into();
        let mut conflict_b = conflict_a.clone();
        conflict_b.name = "attention-b".into();
        conflict_b.mac_normalized = "001122334466".into();
        let mut unconfirmed = pending.clone();
        unconfirmed.name = "unconfirmed".into();
        unconfirmed.management_state = "unexpected".into();
        unconfirmed.deployment_state = "unconfirmed".into();
        let nodes = vec![conflict_a, conflict_b, deployed, pending, unconfirmed];
        let issues = vec![
            issue("3", "issue-c"),
            issue("1", "issue-a"),
            issue("2", "issue-b"),
        ];
        let stats = calculate_stats(&nodes, &issues);
        assert_eq!(
            (
                stats.total,
                stats.pending,
                stats.deployed,
                stats.attention,
                stats.unconfirmed,
                stats.conflicts
            ),
            (5, 1, 1, 5, 1, 2)
        );
        let (all, page_issues, total) = filter_node_page(nodes.clone(), &issues, "", "all", 1, 100);
        assert_eq!(all.len(), 5);
        assert_eq!(total, 5);
        assert!(page_issues.is_empty());
        let (first, first_issues, total) =
            filter_node_page(nodes.clone(), &issues, "", "attention", 1, 3);
        assert_eq!(total, 5);
        assert_eq!(
            first.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(),
            ["attention-a", "attention-b"]
        );
        assert_eq!(
            first_issues
                .iter()
                .map(|i| i.platform_aio_id.as_str())
                .collect::<Vec<_>>(),
            ["1"]
        );
        let (second, second_issues, total) =
            filter_node_page(nodes.clone(), &issues, "", "attention", 2, 3);
        assert_eq!(total, 5);
        assert!(second.is_empty());
        assert_eq!(
            second_issues
                .iter()
                .map(|i| i.platform_aio_id.as_str())
                .collect::<Vec<_>>(),
            ["2", "3"]
        );
        let (last, last_issues, total) =
            filter_node_page(nodes.clone(), &issues, "", "attention", 3, 3);
        assert_eq!(total, 5);
        assert!(last.is_empty() && last_issues.is_empty());
        let (matched, matched_issues, total) =
            filter_node_page(nodes.clone(), &issues, "issue-b", "attention", 1, 3);
        assert_eq!(total, 1);
        assert!(matched.is_empty());
        assert_eq!(matched_issues[0].platform_aio_id, "2");
        for state in ["deployed", "pending", "unconfirmed"] {
            let (matched, matched_issues, total) =
                filter_node_page(nodes.clone(), &issues, "", state, 1, 3);
            assert_eq!(total, 1);
            assert_eq!(matched[0].deployment_state, state);
            assert!(matched_issues.is_empty());
        }
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
