use std::collections::{BTreeMap, BTreeSet};
use std::net::Ipv4Addr;

use super::model::{ScreenFields, SpaceNode};
use crate::core::error::{AppError, AppResult};

pub fn normalize_mac(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !matches!(ch, ':' | '-'))
        .collect::<String>()
        .to_ascii_uppercase()
}

pub fn valid_mac(value: &str) -> bool {
    let value = normalize_mac(value);
    value.len() == 12
        && value.bytes().all(|ch| ch.is_ascii_hexdigit())
        && value != "000000000000"
        && value != "FFFFFFFFFFFF"
        && u8::from_str_radix(&value[..2], 16).is_ok_and(|first| first & 1 == 0)
}

pub fn validate_fields(fields: &ScreenFields, platform: bool) -> AppResult<()> {
    if fields.ip.parse::<Ipv4Addr>().is_err() {
        return Err(AppError::InvalidConfig("请输入有效的 IPv4 地址".into()));
    }
    if !["4", "10", "unknown"].contains(&fields.size.as_str()) {
        return Err(AppError::InvalidConfig(
            "尺寸请选择 4 寸、10 寸或待确认".into(),
        ));
    }
    if fields.name.chars().count() > 32 || fields.location.chars().count() > 128 {
        return Err(AppError::InvalidConfig(
            "名称最多 32 字符，详细位置最多 128 字符".into(),
        ));
    }
    if !fields.mac.is_empty() && !valid_mac(&fields.mac) {
        return Err(AppError::InvalidConfig(
            "MAC 必须为有效网卡地址，不能使用全零、广播或组播地址".into(),
        ));
    }
    if platform
        && (fields.name.trim().is_empty()
            || fields.size == "unknown"
            || fields.space_id.as_deref().is_none_or(str::is_empty))
    {
        return Err(AppError::InvalidConfig(
            "注册前请填写名称、明确尺寸和有效空间".into(),
        ));
    }
    Ok(())
}

pub fn space_path(nodes: &[SpaceNode], id: &str) -> Option<Vec<SpaceNode>> {
    let map: BTreeMap<&str, &SpaceNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    if map.len() != nodes.len() {
        return None;
    }
    let mut current = id;
    let mut seen = BTreeSet::new();
    let mut path = Vec::new();
    while !current.is_empty() && current != "0" {
        if !seen.insert(current) {
            return None;
        }
        let node = map.get(current)?;
        if node.name.trim().is_empty() {
            return None;
        }
        path.push((*node).clone());
        current = node.parent_id.as_deref().unwrap_or("0");
    }
    if path.is_empty() {
        return None;
    }
    path.reverse();
    Some(path)
}

pub fn validate_space(fields: &ScreenFields, nodes: &[SpaceNode], required: bool) -> AppResult<()> {
    match fields.space_id.as_deref().filter(|id| !id.is_empty()) {
        Some(id) if space_path(nodes, id).is_none() => Err(AppError::InvalidConfig(
            "所选空间不存在或父级关系不完整，请重新选择".into(),
        )),
        None if required => Err(AppError::InvalidConfig("请选择当前项目的有效空间".into())),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mac_and_deep_space_validation_rejects_ambiguous_identity() {
        assert!(valid_mac("02:11:22:33:44:55"));
        for value in [
            "00:00:00:00:00:00",
            "FF:FF:FF:FF:FF:FF",
            "01:11:22:33:44:55",
            "xx",
        ] {
            assert!(!valid_mac(value));
        }
        let mut nodes: Vec<_> = (1..=5)
            .map(|i| SpaceNode {
                id: i.to_string(),
                name: format!("空间{i}"),
                parent_id: Some((i - 1).to_string()),
                kind: "area".into(),
            })
            .collect();
        assert_eq!(space_path(&nodes, "5").unwrap().len(), 5);
        nodes[0].parent_id = Some("5".into());
        assert!(space_path(&nodes, "5").is_none());
        nodes[0].parent_id = Some("missing".into());
        assert!(space_path(&nodes, "5").is_none());
    }
}
