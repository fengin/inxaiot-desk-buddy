use std::collections::{BTreeMap, BTreeSet};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceNode {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub kind: String,
}

pub fn space_path(nodes: &[SpaceNode], id: &str) -> Option<Vec<SpaceNode>> {
    let map: BTreeMap<&str, &SpaceNode> = nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    if map.len() != nodes.len() { return None; }
    path_from_map(&map, id)
}

fn path_from_map(map: &BTreeMap<&str, &SpaceNode>, id: &str) -> Option<Vec<SpaceNode>> {
    let mut current = id;
    let mut seen = BTreeSet::new();
    let mut path = Vec::new();
    while !current.is_empty() && current != "0" {
        if !seen.insert(current) { return None; }
        let node = map.get(current)?;
        if node.name.trim().is_empty() { return None; }
        path.push((*node).clone());
        current = node.parent_id.as_deref().unwrap_or("0");
    }
    if path.is_empty() { return None; }
    path.reverse();
    Some(path)
}

pub fn path_text(nodes: &[SpaceNode], id: &str) -> Option<String> {
    space_path(nodes, id).map(|path| path.iter().map(|node| node.name.trim()).collect::<Vec<_>>().join("/"))
}

/// 批量导入只构建一次目录索引，不为每行、每个节点重复建树。
pub fn space_paths(nodes: &[SpaceNode]) -> BTreeMap<String, String> {
    let map: BTreeMap<&str, &SpaceNode> = nodes.iter().map(|node| (node.id.as_str(), node)).collect();
    if map.len() != nodes.len() { return BTreeMap::new(); }
    nodes.iter().filter_map(|node| path_from_map(&map, &node.id).map(|path| (
        node.id.clone(), path.iter().map(|part| part.name.trim()).collect::<Vec<_>>().join("/")
    ))).collect()
}
