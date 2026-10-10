use crate::core::error::{AppError, AppResult};
use crate::domain::common::project_space::{SpaceNode, space_paths};
use super::inventory::InventoryValues;
use std::collections::BTreeMap;

/// 一次构建目录供整批导入使用，默认位置只使用有效父链中的楼栋、楼层。
pub struct InventorySpaceDirectory {
    paths: BTreeMap<String, String>,
    default_locations: BTreeMap<String, String>,
}

impl InventorySpaceDirectory {
    pub fn new(spaces: &[SpaceNode]) -> Self {
        let paths = space_paths(spaces);
        let nodes = spaces.iter().map(|node| (node.id.as_str(), node)).collect::<BTreeMap<_, _>>();
        let default_locations = paths.keys().filter_map(|id| {
            let mut current = id.as_str();
            let mut building = None;
            let mut floor = None;
            // paths 已排除重复 ID、缺失父级、循环及空名称。
            while let Some(node) = nodes.get(current) {
                match node.kind.as_str() {
                    "building" if building.is_none() => building = Some(node.name.trim()),
                    "floor" if floor.is_none() => floor = Some(node.name.trim()),
                    _ => {},
                }
                current = node.parent_id.as_deref().unwrap_or("0");
                if current == "0" || current.is_empty() { break; }
            }
            let names = [building, floor].into_iter().flatten().collect::<Vec<_>>();
            (!names.is_empty()).then(|| (id.clone(), names.join("_")))
        }).collect();
        Self { paths, default_locations }
    }
}

pub fn registration_building_id(value: Option<&str>) -> AppResult<i64> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(0),
        Some(value) => value.parse::<i64>().ok().filter(|value| *value >= 0)
            .ok_or_else(|| AppError::InvalidConfig("空间编号无效，请重新选择空间".into())),
    }
}

pub fn validate_location(building_id: Option<&str>, addr_alias: Option<&str>) -> AppResult<()> {
    let selected = registration_building_id(building_id)? > 0;
    let location = addr_alias.unwrap_or("").trim();
    if selected && location.is_empty() {
        return Err(AppError::InvalidConfig("选择所属空间后，具体位置不能为空；请在一体机详情中补齐后重新预检".into()));
    }
    if location.chars().count() > 128 {
        return Err(AppError::InvalidConfig("具体位置不能超过 128 个字符".into()));
    }
    Ok(())
}

fn normalized_path(value: &str) -> String {
    value.split('/').map(str::trim).collect::<Vec<_>>().join("/")
}

/// ID 是关联依据；导入路径必须唯一匹配，路径文字不能充当 ID。
pub fn resolve_inventory_space(values: &mut InventoryValues, spaces: &[SpaceNode]) -> AppResult<()> {
    resolve_inventory_space_in_directory(values, &InventorySpaceDirectory::new(spaces))
}

pub fn resolve_inventory_space_for_edit(values: &mut InventoryValues, spaces: &[SpaceNode]) -> AppResult<()> {
    resolve_in_directory(values, &InventorySpaceDirectory::new(spaces), false)
}

pub fn resolve_inventory_space_in_directory(values: &mut InventoryValues, directory: &InventorySpaceDirectory) -> AppResult<()> {
    resolve_in_directory(values, directory, true)
}

fn resolve_in_directory(values: &mut InventoryValues, directory: &InventorySpaceDirectory, fill_default: bool) -> AppResult<()> {
    values.normalize_text();
    if fill_default {
        values.addr_alias = values.addr_alias.clone().or_else(|| values.location.clone());
    }
    values.location = values.addr_alias.clone();
    let paths = &directory.paths;
    let requested_path = values.space_path.as_deref().map(normalized_path);
    let selected_id = values.building_id.as_deref().filter(|id| *id != "0");
    let id = if let Some(id) = selected_id {
        id.to_string()
    } else if let Some(requested) = &requested_path {
        let matches = paths.iter().filter(|(_, path)| normalized_path(path) == *requested).map(|(id, _)| id.clone()).collect::<Vec<_>>();
        match matches.as_slice() {
            [id] => id.clone(),
            [] => return Err(AppError::InvalidConfig("空间路径不存在，请填写完整路径或从空间树重新选择".into())),
            _ => return Err(AppError::InvalidConfig("空间路径对应多个节点，请核实后使用明确的空间编号".into())),
        }
    } else {
        values.building_id = None;
        return validate_location(None, values.addr_alias.as_deref());
    };
    let full_path = paths.get(&id).cloned().ok_or_else(|| AppError::InvalidConfig("所选空间不存在或父级关系不完整，请重新选择".into()))?;
    if requested_path.is_some_and(|expected| expected != normalized_path(&full_path)) {
        return Err(AppError::Conflict("空间路径和编号不一致，或空间已调整，请重新选择或导入".into()));
    }
    values.building_id = Some(id);
    values.space_path = Some(full_path);
    if fill_default && values.addr_alias.is_none() {
        values.addr_alias = values.building_id.as_ref().and_then(|id| directory.default_locations.get(id)).cloned();
        values.location = values.addr_alias.clone();
    }
    validate_location(values.building_id.as_deref(), values.addr_alias.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces() -> Vec<SpaceNode> {
        [("1", "项目", "0", "other"), ("2", "一号楼", "1", "building"), ("3", "一层", "2", "floor"), ("5", "房间", "3", "area")].into_iter()
            .map(|(id, name, parent, kind)| SpaceNode { id: id.into(), name: name.into(), parent_id: Some(parent.into()), kind: kind.into() }).collect()
    }

    #[test]
    fn csv_path_and_selected_id_resolve_to_the_same_platform_fields() {
        let directory = spaces();
        let mut csv = InventoryValues { space_path: Some(" 项目 / 一号楼 / 一层 ".into()), addr_alias: Some("门口弱电柜".into()), ..Default::default() };
        resolve_inventory_space(&mut csv, &directory).unwrap();
        let mut selected = InventoryValues { building_id: Some("3".into()), addr_alias: Some("门口弱电柜".into()), ..Default::default() };
        resolve_inventory_space(&mut selected, &directory).unwrap();
        assert_eq!(csv, selected);
        assert_eq!(selected.building_id.as_deref(), Some("3"));
        assert_eq!(selected.space_path.as_deref(), Some("项目/一号楼/一层"));
        assert_eq!(selected.addr_alias.as_deref(), Some("门口弱电柜"));
    }

    #[test]
    fn missing_ambiguous_or_changed_spaces_never_guess_an_id() {
        let mut directory = spaces();
        let input = InventoryValues { space_path: Some("项目/一号楼/一层".into()), ..Default::default() };
        directory.push(SpaceNode { id: "4".into(), ..directory[2].clone() });
        assert!(resolve_inventory_space(&mut input.clone(), &directory).is_err());
        let mut selected = InventoryValues { building_id: Some("3".into()), ..input.clone() };
        resolve_inventory_space(&mut selected, &directory).unwrap();
        directory[2].name = "新一层".into();
        assert!(resolve_inventory_space(&mut selected, &directory).is_err());
        assert!(resolve_inventory_space(&mut input.clone(), &[]).is_err());
        directory[1].parent_id = Some("3".into());
        assert!(resolve_inventory_space(&mut InventoryValues { building_id: Some("3".into()), ..Default::default() }, &directory).is_err());
        resolve_inventory_space(&mut InventoryValues::default(), &[]).unwrap();
    }

    #[test]
    fn create_and_csv_fill_building_floor_default_but_preserve_custom_location() {
        let directory = spaces();
        for (id, expected) in [("2", "一号楼"), ("3", "一号楼_一层"), ("5", "一号楼_一层")] {
            let mut selected = InventoryValues { building_id: Some(id.into()), addr_alias: Some("  ".into()), ..Default::default() };
            resolve_inventory_space(&mut selected, &directory).unwrap();
            assert_eq!(selected.addr_alias.as_deref(), Some(expected));
            assert_eq!(selected.location, selected.addr_alias);
        }
        let mut csv = InventoryValues { space_path: Some("项目/一号楼/一层/房间".into()), ..Default::default() };
        resolve_inventory_space(&mut csv, &directory).unwrap();
        assert_eq!(csv.building_id.as_deref(), Some("5"));
        assert_eq!(csv.addr_alias.as_deref(), Some("一号楼_一层"));
        csv.addr_alias = Some("  北侧弱电柜  ".into());
        resolve_inventory_space(&mut csv, &directory).unwrap();
        assert_eq!(csv.addr_alias.as_deref(), Some("北侧弱电柜"));
        assert_eq!(csv.location, csv.addr_alias);
    }

    #[test]
    fn editing_selected_space_rejects_cleared_location_instead_of_restoring_it() {
        let directory = spaces();
        for addr_alias in [None, Some("".into()), Some(" \t\n ".into())] {
            let mut input = InventoryValues { building_id: Some("3".into()), addr_alias,
                location: Some("旧位置不能恢复用户清空的值".into()), ..Default::default() };
            assert!(resolve_inventory_space_for_edit(&mut input, &directory).is_err());
        }
        let mut custom = InventoryValues { building_id: Some("3".into()), addr_alias: Some("  靠窗柜  ".into()), ..Default::default() };
        resolve_inventory_space_for_edit(&mut custom, &directory).unwrap();
        assert_eq!(custom.addr_alias.as_deref(), Some("靠窗柜"));
        for location in [None, Some("自由填写的位置".into())] {
            let mut input = InventoryValues { addr_alias: location, ..Default::default() };
            resolve_inventory_space_for_edit(&mut input, &[]).unwrap();
            assert_eq!(input.location, input.addr_alias);
        }
    }

    #[test]
    fn unknown_space_kinds_require_manual_location_and_long_defaults_are_rejected() {
        let mut directory = spaces();
        let mut root = InventoryValues { building_id: Some("1".into()), ..Default::default() };
        assert!(resolve_inventory_space(&mut root, &directory).is_err());
        root.addr_alias = Some("项目现场设备间".into());
        resolve_inventory_space(&mut root, &directory).unwrap();
        directory[1].name = "楼".repeat(128);
        let mut floor = InventoryValues { building_id: Some("3".into()), ..Default::default() };
        assert!(resolve_inventory_space(&mut floor, &directory).is_err());
        assert_eq!(floor.addr_alias.unwrap().chars().count(), 131);
        assert!(validate_location(Some("3"), Some(&"位".repeat(128))).is_ok());
        assert!(validate_location(Some("3"), Some(&"位".repeat(129))).is_err());
        assert!(validate_location(Some("invalid-id"), Some("位置")).is_err());
        assert!(validate_location(Some("-1"), Some("位置")).is_err());
    }

    #[test]
    fn nested_duplicate_kinds_use_the_nearest_building_and_floor_once() {
        let mut directory = spaces();
        directory.push(SpaceNode {id:"6".into(),name:"内楼栋".into(),parent_id:Some("3".into()),kind:"building".into()});
        directory.push(SpaceNode {id:"7".into(),name:"内楼层".into(),parent_id:Some("6".into()),kind:"floor".into()});
        let mut values = InventoryValues {building_id:Some("7".into()),..Default::default()};
        resolve_inventory_space(&mut values, &directory).unwrap();
        assert_eq!(values.addr_alias.as_deref(), Some("内楼栋_内楼层"));
    }
}
