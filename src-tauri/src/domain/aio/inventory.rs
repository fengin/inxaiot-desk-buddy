use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;

use serde::{Deserialize, Serialize};

use super::mac::MacAddress;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InventoryValues {
    pub name: String,
    pub ip: String,
    pub mac: String,
    pub building_id: Option<String>,
    pub region_id: Option<String>,
    pub addr_alias: Option<String>,
    pub floor: Option<String>,
    pub location: Option<String>,
    pub remark: Option<String>,
}

impl InventoryValues {
    pub fn normalize_text(&mut self) {
        self.name = self.name.trim().to_string();
        self.ip = self.ip.trim().to_string();
        self.mac = self.mac.trim().to_string();
        for value in [
            &mut self.building_id,
            &mut self.region_id,
            &mut self.addr_alias,
            &mut self.floor,
            &mut self.location,
            &mut self.remark,
        ] {
            *value = value
                .take()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty());
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedInventoryRow {
    pub row_number: u32,
    pub values: InventoryValues,
    pub mac_normalized: Option<String>,
    pub errors: Vec<String>,
}

/// CSV 导入与单台新增使用相同的字段规范和校验。
pub fn validate_inventory_values(row_number: u32, mut values: InventoryValues) -> ParsedInventoryRow {
    values.normalize_text();
    let mut errors = Vec::new();
    if values.name.is_empty() {
        errors.push(format!("第 {row_number} 行：名称不能为空"));
    }
    if values.ip.is_empty() {
        errors.push(format!("第 {row_number} 行：IP 不能为空"));
    } else if values.ip.parse::<IpAddr>().is_err() {
        errors.push(format!("第 {row_number} 行：IP 格式无效"));
    }
    let mac_normalized = if values.mac.is_empty() {
        errors.push(format!("第 {row_number} 行：MAC 不能为空"));
        None
    } else {
        match MacAddress::parse(&values.mac) {
            Ok(mac) => Some(mac.normalized().to_string()),
            Err(_) => {
                errors.push(format!("第 {row_number} 行：MAC 格式无效"));
                None
            }
        }
    };
    ParsedInventoryRow { row_number, values, mac_normalized, errors }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchNodeSnapshot {
    pub mac_normalized: String,
    pub name: String,
    pub ip: String,
    pub building_id: Option<String>,
    pub region_id: Option<String>,
    pub addr_alias: Option<String>,
    pub floor: Option<String>,
    pub location: Option<String>,
    pub remark: Option<String>,
    pub platform_aio_id: Option<String>,
    pub management_state: String,
    pub source: String,
    pub last_operation_id: Option<String>,
    pub version: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformNodeSnapshot {
    pub id: String,
    pub name: String,
    pub ip: String,
    pub mac_raw: String,
    pub mac_normalized: String,
    pub building_id: Option<String>,
    pub addr_alias: Option<String>,
    pub status: Option<i32>,
    pub last_beat_time: Option<i64>,
    pub last_sync_time: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportClassification {
    NewPending,
    ExistingUnchanged,
    ExistingChanged,
    PlatformExisting,
    Conflict,
    Invalid,
}

impl ImportClassification {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::NewPending => "new_pending",
            Self::ExistingUnchanged => "existing_unchanged",
            Self::ExistingChanged => "existing_changed",
            Self::PlatformExisting => "platform_existing",
            Self::Conflict => "conflict",
            Self::Invalid => "invalid",
        }
    }

    pub fn can_apply(&self) -> bool {
        matches!(
            self,
            Self::NewPending | Self::ExistingChanged | Self::PlatformExisting
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldConflict {
    pub code: String,
    pub field: String,
    pub message: String,
    pub import_value: Option<String>,
    pub workbench_value: Option<String>,
    pub platform_value: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciledImportItem {
    pub row_number: u32,
    pub values: InventoryValues,
    pub mac_normalized: Option<String>,
    pub display_mac: Option<String>,
    pub classification: ImportClassification,
    pub selected: bool,
    pub errors: Vec<String>,
    pub conflicts: Vec<FieldConflict>,
    pub workbench_version: Option<u64>,
    pub platform_aio_id: Option<String>,
    pub platform_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportCounts {
    pub total: u32,
    pub new_pending: u32,
    pub existing_unchanged: u32,
    pub existing_changed: u32,
    pub platform_existing: u32,
    pub conflicts: u32,
    pub invalid: u32,
    pub selected: u32,
}

impl ImportCounts {
    pub fn from_items(items: &[ReconciledImportItem]) -> Self {
        let mut counts = Self {
            total: u32::try_from(items.len()).unwrap_or(u32::MAX),
            ..Self::default()
        };
        for item in items {
            match item.classification {
                ImportClassification::NewPending => counts.new_pending += 1,
                ImportClassification::ExistingUnchanged => counts.existing_unchanged += 1,
                ImportClassification::ExistingChanged => counts.existing_changed += 1,
                ImportClassification::PlatformExisting => counts.platform_existing += 1,
                ImportClassification::Conflict => counts.conflicts += 1,
                ImportClassification::Invalid => counts.invalid += 1,
            }
            if item.selected {
                counts.selected += 1;
            }
        }
        counts
    }
}

pub fn reconcile_inventory(
    mut rows: Vec<ParsedInventoryRow>,
    workbench_nodes: &[WorkbenchNodeSnapshot],
    platform_nodes: &[PlatformNodeSnapshot],
) -> Vec<ReconciledImportItem> {
    apply_import_duplicates(&mut rows);
    let workbench = workbench_nodes
        .iter()
        .map(|node| (node.mac_normalized.clone(), node))
        .collect::<HashMap<_, _>>();
    let mut platform = HashMap::<String, Vec<&PlatformNodeSnapshot>>::new();
    for node in platform_nodes {
        platform
            .entry(node.mac_normalized.clone())
            .or_default()
            .push(node);
    }

    rows.into_iter()
        .map(|row| {
            let display_mac = row.mac_normalized.as_ref().map(|mac| {
                mac.as_bytes()
                    .chunks(2)
                    .map(|chunk| std::str::from_utf8(chunk).expect("validated MAC"))
                    .collect::<Vec<_>>()
                    .join(":")
            });
            if !row.errors.is_empty() || row.mac_normalized.is_none() {
                return ReconciledImportItem {
                    row_number: row.row_number,
                    values: row.values,
                    mac_normalized: row.mac_normalized,
                    display_mac,
                    classification: ImportClassification::Invalid,
                    selected: false,
                    errors: row.errors,
                    conflicts: Vec::new(),
                    workbench_version: None,
                    platform_aio_id: None,
                    platform_fingerprint: None,
                };
            }

            let mac = row.mac_normalized.clone().expect("checked MAC");
            let workbench_node = workbench.get(&mac).copied();
            let platform_matches = platform.get(&mac).cloned().unwrap_or_default();
            if platform_matches.len() > 1 {
                let ids = platform_matches
                    .iter()
                    .map(|node| node.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return reconciled_conflict(
                    row,
                    display_mac,
                    workbench_node,
                    None,
                    FieldConflict {
                        code: "PLATFORM_DUPLICATE_MAC".into(),
                        field: "mac".into(),
                        message: format!("平台存在多条相同规范化 MAC 记录：{ids}"),
                        import_value: Some(mac),
                        workbench_value: None,
                        platform_value: Some(ids),
                    },
                );
            }
            let platform_node = platform_matches.first().copied();

            if let (Some(workbench_node), Some(platform_node)) = (workbench_node, platform_node) {
                if let Some(linked_id) = workbench_node.platform_aio_id.as_deref()
                    && linked_id != platform_node.id
                {
                    return reconciled_conflict(
                        row,
                        display_mac,
                        Some(workbench_node),
                        Some(platform_node),
                        FieldConflict {
                            code: "PLATFORM_LINK_MISMATCH".into(),
                            field: "platformAioId".into(),
                            message: "工作台平台关联 ID 与当前 MAC 匹配对象不一致".into(),
                            import_value: None,
                            workbench_value: Some(linked_id.into()),
                            platform_value: Some(platform_node.id.clone()),
                        },
                    );
                }
                let identity_conflicts =
                    workbench_platform_identity_conflicts(workbench_node, platform_node);
                if !identity_conflicts.is_empty() {
                    let mut item = reconciled_conflict(
                        row,
                        display_mac,
                        Some(workbench_node),
                        Some(platform_node),
                        identity_conflicts[0].clone(),
                    );
                    item.conflicts = identity_conflicts;
                    return item;
                }
            } else if let Some(workbench_node) = workbench_node
                && workbench_node.platform_aio_id.is_some()
            {
                return reconciled_conflict(
                    row,
                    display_mac,
                    Some(workbench_node),
                    None,
                    FieldConflict {
                        code: "LINKED_PLATFORM_NODE_MISSING".into(),
                        field: "platformAioId".into(),
                        message: "工作台记录已关联平台对象，但平台当前无法按 MAC 找到该对象".into(),
                        import_value: None,
                        workbench_value: workbench_node.platform_aio_id.clone(),
                        platform_value: None,
                    },
                );
            }

            let (classification, selected) = match (workbench_node, platform_node) {
                (None, None) => (ImportClassification::NewPending, true),
                (None, Some(_)) => (ImportClassification::PlatformExisting, true),
                (Some(node), _) if inventory_matches_workbench(&row.values, node) => {
                    (ImportClassification::ExistingUnchanged, false)
                }
                (Some(_), _) => (ImportClassification::ExistingChanged, true),
            };
            ReconciledImportItem {
                row_number: row.row_number,
                values: row.values,
                mac_normalized: row.mac_normalized,
                display_mac,
                classification,
                selected,
                errors: Vec::new(),
                conflicts: Vec::new(),
                workbench_version: workbench_node.map(|node| node.version),
                platform_aio_id: platform_node.map(|node| node.id.clone()),
                platform_fingerprint: platform_node.map(platform_fingerprint),
            }
        })
        .collect()
}

fn reconciled_conflict(
    row: ParsedInventoryRow,
    display_mac: Option<String>,
    workbench: Option<&WorkbenchNodeSnapshot>,
    platform: Option<&PlatformNodeSnapshot>,
    conflict: FieldConflict,
) -> ReconciledImportItem {
    ReconciledImportItem {
        row_number: row.row_number,
        values: row.values,
        mac_normalized: row.mac_normalized,
        display_mac,
        classification: ImportClassification::Conflict,
        selected: false,
        errors: row.errors,
        conflicts: vec![conflict],
        workbench_version: workbench.map(|node| node.version),
        platform_aio_id: platform.map(|node| node.id.clone()),
        platform_fingerprint: platform.map(platform_fingerprint),
    }
}

pub fn platform_fingerprint(node: &PlatformNodeSnapshot) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}",
        node.id,
        node.name,
        node.ip,
        node.mac_raw,
        node.building_id.as_deref().unwrap_or(""),
        node.addr_alias.as_deref().unwrap_or(""),
        node.status
            .map(|value| value.to_string())
            .unwrap_or_default(),
        node.last_beat_time
            .map(|value| value.to_string())
            .unwrap_or_default(),
        node.last_sync_time.as_deref().unwrap_or("")
    )
}

fn apply_import_duplicates(rows: &mut [ParsedInventoryRow]) {
    let mut mac_rows = BTreeMap::<String, Vec<u32>>::new();
    let mut ip_rows = BTreeMap::<String, Vec<u32>>::new();
    for row in rows.iter() {
        if let Some(mac) = &row.mac_normalized {
            mac_rows
                .entry(mac.clone())
                .or_default()
                .push(row.row_number);
        }
        if !row.values.ip.is_empty() {
            ip_rows
                .entry(row.values.ip.clone())
                .or_default()
                .push(row.row_number);
        }
    }
    for row in rows {
        if let Some(mac) = &row.mac_normalized
            && let Some(duplicates) = mac_rows.get(mac)
            && duplicates.len() > 1
        {
            row.errors.push(format!(
                "第 {} 行：MAC 与第 {} 行重复",
                row.row_number,
                duplicates
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
        if let Some(duplicates) = ip_rows.get(&row.values.ip)
            && duplicates.len() > 1
        {
            row.errors.push(format!(
                "第 {} 行：IP 与第 {} 行重复",
                row.row_number,
                duplicates
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            ));
        }
    }
}

fn inventory_matches_workbench(values: &InventoryValues, node: &WorkbenchNodeSnapshot) -> bool {
    values.name.trim() == node.name.trim()
        && values.ip.trim() == node.ip.trim()
        && option_eq(&values.building_id, &node.building_id)
        && option_eq(&values.region_id, &node.region_id)
        && option_eq(&values.addr_alias, &node.addr_alias)
        && option_eq(&values.floor, &node.floor)
        && option_eq(&values.location, &node.location)
        && option_eq(&values.remark, &node.remark)
}

fn option_eq(left: &Option<String>, right: &Option<String>) -> bool {
    left.as_deref().unwrap_or("").trim() == right.as_deref().unwrap_or("").trim()
}

fn workbench_platform_identity_conflicts(
    workbench: &WorkbenchNodeSnapshot,
    platform: &PlatformNodeSnapshot,
) -> Vec<FieldConflict> {
    let mut conflicts = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(row_number: u32, name: &str, ip: &str, mac: &str) -> ParsedInventoryRow {
        ParsedInventoryRow {
            row_number,
            values: InventoryValues {
                name: name.into(),
                ip: ip.into(),
                mac: mac.into(),
                ..InventoryValues::default()
            },
            mac_normalized: Some(mac.into()),
            errors: Vec::new(),
        }
    }

    fn workbench(mac: &str) -> WorkbenchNodeSnapshot {
        WorkbenchNodeSnapshot {
            mac_normalized: mac.into(),
            name: "node-1".into(),
            ip: "192.168.3.101".into(),
            building_id: None,
            region_id: None,
            addr_alias: None,
            floor: None,
            location: None,
            remark: None,
            platform_aio_id: None,
            management_state: "pending".into(),
            source: "import".into(),
            last_operation_id: None,
            version: 3,
        }
    }

    fn platform(mac: &str) -> PlatformNodeSnapshot {
        PlatformNodeSnapshot {
            id: "1001".into(),
            name: "node-1".into(),
            ip: "192.168.3.101".into(),
            mac_raw: mac.into(),
            mac_normalized: mac.into(),
            building_id: None,
            addr_alias: None,
            status: Some(1),
            last_beat_time: None,
            last_sync_time: None,
        }
    }

    #[test]
    fn reconciliation_covers_new_existing_changed_platform_and_duplicates() {
        let mac_new = "001122334401";
        let mac_existing = "001122334402";
        let mac_changed = "001122334403";
        let mac_platform = "001122334404";
        let mut existing = workbench(mac_existing);
        existing.name = "same".into();
        existing.ip = "192.168.3.102".into();
        let mut changed = workbench(mac_changed);
        changed.ip = "192.168.3.103".into();
        let mut platform_only = platform(mac_platform);
        platform_only.ip = "192.168.3.104".into();
        let items = reconcile_inventory(
            vec![
                row(2, "new", "192.168.3.101", mac_new),
                row(3, "same", "192.168.3.102", mac_existing),
                row(4, "changed-name", "192.168.3.103", mac_changed),
                row(5, "platform-import", "192.168.3.104", mac_platform),
            ],
            &[existing, changed],
            &[platform_only],
        );
        assert_eq!(items[0].classification, ImportClassification::NewPending);
        assert_eq!(
            items[1].classification,
            ImportClassification::ExistingUnchanged
        );
        assert_eq!(
            items[2].classification,
            ImportClassification::ExistingChanged
        );
        assert_eq!(
            items[3].classification,
            ImportClassification::PlatformExisting
        );
        assert_eq!(ImportCounts::from_items(&items).selected, 3);
    }

    #[test]
    fn duplicate_import_and_duplicate_platform_mac_are_blocked() {
        let duplicate_mac = "001122334405";
        let duplicated_rows = reconcile_inventory(
            vec![
                row(2, "one", "192.168.3.105", duplicate_mac),
                row(3, "two", "192.168.3.106", duplicate_mac),
            ],
            &[],
            &[],
        );
        assert!(
            duplicated_rows
                .iter()
                .all(|item| item.classification == ImportClassification::Invalid)
        );

        let duplicate_platform = reconcile_inventory(
            vec![row(2, "one", "192.168.3.105", duplicate_mac)],
            &[],
            &[
                platform(duplicate_mac),
                PlatformNodeSnapshot {
                    id: "1002".into(),
                    ..platform(duplicate_mac)
                },
            ],
        );
        assert_eq!(
            duplicate_platform[0].classification,
            ImportClassification::Conflict
        );
        assert_eq!(
            duplicate_platform[0].conflicts[0].code,
            "PLATFORM_DUPLICATE_MAC"
        );
    }
}
