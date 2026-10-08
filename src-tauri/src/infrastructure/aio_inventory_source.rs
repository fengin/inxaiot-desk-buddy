use crate::core::error::AppResult;
use crate::domain::aio::inventory::{WorkbenchNodeSnapshot, PlatformNodeSnapshot};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::{local_sqlite::aio_node_repository::LocalAioRepository, platform_aio::PlatformAioRepository, project_context::project_pools, workbench_aio::WorkbenchAioRepository};

/// 本机待实施资料 + 平台已注册资料。工作台库只补充部署结果，不提供未注册清单。
pub fn merge_sources(local: &[WorkbenchNodeSnapshot], shared: &[WorkbenchNodeSnapshot], platform: &[PlatformNodeSnapshot]) -> Vec<WorkbenchNodeSnapshot> {
    let mut result = local.iter().filter(|node| !platform.iter().any(|p| p.mac_normalized == node.mac_normalized)).cloned().collect::<Vec<_>>();
    for node in platform {
        let shared = shared.iter().find(|s| s.mac_normalized == node.mac_normalized);
        result.push(WorkbenchNodeSnapshot {
            mac_normalized: node.mac_normalized.clone(), name: node.name.clone(), ip: node.ip.clone(),
            building_id: node.building_id.clone().filter(|id| id != "0" && !id.is_empty()),
            addr_alias: node.addr_alias.clone(), location: node.addr_alias.clone(),
            region_id: shared.and_then(|s| s.region_id.clone()), floor: shared.and_then(|s| s.floor.clone()), remark: shared.and_then(|s| s.remark.clone()),
            platform_aio_id: Some(node.id.clone()), management_state: shared.map(|s| s.management_state.clone()).unwrap_or_else(|| "platform_existing".into()),
            source: if shared.is_some() { "merged" } else { "platform" }.into(),
            last_operation_id: shared.and_then(|s| s.last_operation_id.clone()), version: shared.map(|s| s.version).unwrap_or(0),
        });
    }
    result
}

pub async fn deployment_nodes(state: &FormalAppState, project: &str) -> AppResult<Vec<WorkbenchNodeSnapshot>> {
    let pools = project_pools(state, project).await?;
    let local_repository = LocalAioRepository::new(state.local_store.pool().clone());
    let shared_repository = WorkbenchAioRepository::new(pools.workbench.clone());
    let platform_repository = PlatformAioRepository::new(pools.platform.clone());
    let (local, shared, platform) = tokio::try_join!(
        local_repository.list(project),
        shared_repository.list_snapshots(),
        platform_repository.list_all(),
    )?;
    Ok(merge_sources(&local, &shared, &platform.nodes))
}

pub async fn retire_registered(state: &FormalAppState, project: &str, local: &[WorkbenchNodeSnapshot], platform: &[PlatformNodeSnapshot]) -> AppResult<()> {
    let repository = LocalAioRepository::new(state.local_store.pool().clone());
    for node in local {
        if platform.iter().filter(|p| p.mac_normalized == node.mac_normalized).count() == 1 {
            repository.retire(project, &node.mac_normalized, node.version).await?;
        }
    }
    Ok(())
}
