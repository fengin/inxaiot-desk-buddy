use crate::core::error::{AppError, AppResult};
use crate::domain::common::project_space::{SpaceNode, space_paths};
use super::inventory::InventoryValues;

fn normalized_path(value: &str) -> String {
    value.split('/').map(str::trim).collect::<Vec<_>>().join("/")
}

/// ID 是关联依据；导入路径必须唯一匹配，路径文字不能充当 ID。
pub fn resolve_inventory_space(values: &mut InventoryValues, spaces: &[SpaceNode]) -> AppResult<()> {
    resolve_inventory_space_in_directory(values, &space_paths(spaces))
}

pub fn resolve_inventory_space_in_directory(values: &mut InventoryValues, paths: &std::collections::BTreeMap<String, String>) -> AppResult<()> {
    values.normalize_text();
    values.addr_alias = values.addr_alias.clone().or_else(|| values.location.clone());
    values.location = values.addr_alias.clone();
    if values.addr_alias.as_ref().is_some_and(|value| value.chars().count() > 128) {
        return Err(AppError::InvalidConfig("具体位置不能超过 128 个字符".into()));
    }
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
        return Ok(());
    };
    let full_path = paths.get(&id).cloned().ok_or_else(|| AppError::InvalidConfig("所选空间不存在或父级关系不完整，请重新选择".into()))?;
    if requested_path.is_some_and(|expected| expected != normalized_path(&full_path)) {
        return Err(AppError::Conflict("空间路径和编号不一致，或空间已调整，请重新选择或导入".into()));
    }
    values.building_id = Some(id);
    values.space_path = Some(full_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces() -> Vec<SpaceNode> {
        [("1", "项目", "0"), ("2", "一号楼", "1"), ("3", "一层", "2")].into_iter()
            .map(|(id, name, parent)| SpaceNode { id: id.into(), name: name.into(), parent_id: Some(parent.into()), kind: "other".into() }).collect()
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
}
