use std::time::Duration;
use sqlx::{MySqlPool, Row};
use crate::application::aio_assets::UpdateAioNodeInput;
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::{inventory::validate_inventory_values, mac::MacAddress, space::resolve_inventory_space};
use crate::formal::{app_state::FormalAppState, operation_repository::{OperationRepository,OperationStart,OperationFinalResult,TargetFinalResult}, resource_lease_repository::{LeaseGrant, LeaseRequest, ResourceLeaseRepository}};
use crate::infrastructure::{client_instance::application_instance_id, local_sqlite::aio_node_repository::LocalAioRepository, platform_aio::PlatformAioRepository, project_context::{map_formal_error, project_operator, project_pools, project_platform_write_pool}, project_spaces};

fn db(error: sqlx::Error) -> AppError { AppError::database("保存一体机资料", &error) }

pub async fn update(state: &FormalAppState, project: &str, mut input: UpdateAioNodeInput) -> AppResult<()> {
    let mac = MacAddress::parse(&input.mac)?.normalized().to_string();
    if MacAddress::parse(&input.values.mac)?.normalized() != mac {
        return Err(AppError::InvalidConfig("MAC 是一体机身份，不能通过编辑资料修改".into()));
    }
    let row = validate_inventory_values(1, input.values.clone());
    if !row.errors.is_empty() { return Err(AppError::InvalidConfig(row.errors.join("；"))); }
    input.values = row.values;
    if input.values.name.chars().count() > 32 || input.values.ip.len() > 32 {
        return Err(AppError::InvalidConfig("名称和 IP 不能超过平台支持的 32 个字符".into()));
    }
    let pools = project_pools(state, project).await?;
    let platform = PlatformAioRepository::new(pools.platform.clone()).list_all().await?;
    let matched = platform.nodes.iter().filter(|node| node.mac_normalized == mac).collect::<Vec<_>>();
    if matched.len() > 1 { return Err(AppError::Conflict("平台存在多条相同 MAC，请先处理重复记录".into())); }
    if matched.first().map(|node| node.id.as_str()) != input.platform_base.as_ref().map(|node| node.id.as_str()) {
        return Err(AppError::Conflict("一体机注册状态已变化，请刷新详情后再编辑".into()));
    }
    let spaces = if input.values.building_id.is_some() || input.values.space_path.is_some() {
        project_spaces::read(&pools.platform, None).await?
    } else { Vec::new() };
    resolve_inventory_space(&mut input.values, &spaces)?;
    if input.platform_base.is_none() {
        let local = LocalAioRepository::new(state.local_store.pool().clone());
        let nodes = local.list(project).await?;
        let old = nodes.iter().find(|node| node.mac_normalized == mac)
            .ok_or_else(|| AppError::NotFound("本机待实施一体机不存在，请刷新列表".into()))?;
        if nodes.iter().any(|node| node.mac_normalized != mac && node.ip == input.values.ip)
            || platform.nodes.iter().any(|node| node.mac_normalized != mac && node.ip == input.values.ip) {
            return Err(AppError::Conflict("IP 已被另一台一体机使用，请核对".into()));
        }
        input.values.region_id = old.region_id.clone(); input.values.floor = old.floor.clone(); input.values.remark = old.remark.clone();
        return local.save_many(project, &[(input.values, Some(input.expected_version))]).await;
    }
    crate::infrastructure::project_context::project_database(state, project).await?;
    let operator = project_operator(state, project).await?;
    let write = project_platform_write_pool(state, project).await?;
    let read_source: String = sqlx::query_scalar("SELECT CONCAT(@@server_uuid,':',DATABASE())").fetch_one(&pools.platform).await.map_err(db)?;
    let write_source: String = sqlx::query_scalar("SELECT CONCAT(@@server_uuid,':',DATABASE())").fetch_one(&write).await.map_err(db)?;
    if read_source != write_source { return Err(AppError::Conflict("平台读写连接不一致，请重新连接项目".into())); }
    let shared_schema: String = sqlx::query_scalar("SELECT DATABASE()").fetch_one(&pools.workbench).await.map_err(db)?;
    let operations = OperationRepository::new(pools.workbench.clone());
    let record = operations.start(OperationStart { domain_type:"aio".into(), operation_type:"asset_edit".into(), operation_name:"编辑一体机资料".into(),
        operator_name:operator.clone(),instance_id:application_instance_id().into(),targets:vec![("aio".into(),mac.clone())],
        artifact_name:None,artifact_version:None,operation_summary:None,retry_of_operation_id:None }).await.map_err(map_formal_error)?;
    let operation = record.id;
    let leases = ResourceLeaseRepository::new(pools.workbench.clone());
    let requests = vec![LeaseRequest { resource_type: "aio".into(), resource_key: mac.clone(), domain_type: "aio".into(),
        operation_id: operation.clone(), owner_instance_id: application_instance_id().into(), owner_user: operator.clone(), ttl: Duration::from_secs(60) }];
    let granted = if input.force_takeover { leases.takeover_for_new_operation(requests).await } else { leases.acquire_many(requests).await };
    let grants = match granted {
        Ok(grants) => grants,
        Err(error) => {
            finish_failed(&operations, &operation, &mac, record.version).await;
            if let Some(held) = leases.active_lease("aio", &mac).await.map_err(map_formal_error)? {
                return Err(AppError::ConfirmationRequired { code: "AIO_EDIT_LOCKED",
                    details: serde_json::json!({"instance": held.owner_instance_id, "operationId": held.operation_id}) });
            }
            return Err(map_formal_error(error));
        }
    };
    let result = save_registered(&write, &shared_schema, &input, &operator, &grants[0]).await;
    if result.is_err() { finish_failed(&operations, &operation, &mac, record.version).await; }
    let _ = leases.release(&grants[0]).await;
    result
}

async fn finish_failed(operations: &OperationRepository, id: &str, mac: &str, version: u64) {
    // 已确认提交的成功结果不被后续网络错误覆盖。
    if operations.get(id).await.is_ok_and(|record| record.state == "running") {
        let _ = operations.finalize_target(TargetFinalResult { operation_id:id.into(),resource_type:"aio".into(),resource_key:mac.into(),result_state:"failed".into(),before_version:None,after_version:None,result_summary:Some("本次保存未完成".into()),error_code:Some("SAVE_FAILED".into()),error_summary:None }).await;
        let _ = operations.finalize(OperationFinalResult {operation_id:id.into(),expected_version:version,state:"failed".into(),result_summary:Some("资料未保存，请刷新详情核对".into()),error_code:Some("SAVE_FAILED".into()),error_summary:None}).await;
    }
}

/// 字段白名单与共享占用在同一 MySQL 事务中核实；不改 MAC、布点、状态或连接凭据。
pub async fn save_registered(pool: &MySqlPool, shared_schema: &str, input: &UpdateAioNodeInput, operator: &str, grant: &LeaseGrant) -> AppResult<()> {
    if shared_schema.is_empty() || shared_schema.len() > 64 || !shared_schema.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
        return Err(AppError::InvalidConfig("工作台数据库名无效".into()));
    }
    let base = input.platform_base.as_ref().ok_or_else(|| AppError::InvalidConfig("缺少平台原始资料".into()))?;
    let mac = MacAddress::parse(&input.mac)?.normalized().to_string();
    if base.mac_normalized != mac || grant.resource_type != "aio" || grant.resource_key != mac {
        return Err(AppError::Conflict("一体机身份与当前操作不一致".into()));
    }
    let mut tx = pool.begin().await.map_err(db)?;
    let lock_sql = format!("SELECT fencing_token FROM `{shared_schema}`.resource_lease WHERE resource_type='aio' AND resource_key=? AND lease_token=? AND fencing_token=? AND owner_instance_id=? AND operation_id=? AND lease_state='active' AND expires_at>UTC_TIMESTAMP(6) FOR UPDATE");
    let held: Option<u64> = sqlx::query_scalar(&lock_sql).bind(&mac).bind(&grant.lease_token).bind(grant.fencing_token).bind(&grant.owner_instance_id).bind(&grant.operation_id).fetch_optional(&mut *tx).await.map_err(db)?;
    if held.is_none() { return Err(AppError::Conflict("操作已被接手，请重新读取资料".into())); }
    let actual = sqlx::query("SELECT name,ip,mac,CAST(building_id AS CHAR) AS building_id,addr_alias FROM op_edge_aio_server WHERE id=? FOR UPDATE")
        .bind(&base.id).fetch_optional(&mut *tx).await.map_err(db)?
        .ok_or_else(|| AppError::Conflict("平台一体机已删除，请刷新列表".into()))?;
    let before_name: String = actual.try_get("name").map_err(db)?;
    let before_ip: String = actual.try_get("ip").map_err(db)?;
    let before_mac: String = actual.try_get("mac").map_err(db)?;
    let before_space: String = actual.try_get("building_id").map_err(db)?;
    let before_location: String = actual.try_get("addr_alias").map_err(db)?;
    if before_name != base.name || before_ip != base.ip || before_mac != base.mac_raw
        || before_space != base.building_id.as_deref().unwrap_or("0") || before_location != base.addr_alias.as_deref().unwrap_or("") {
        return Err(AppError::Conflict("平台资料已变化，请重新读取后再保存".into()));
    }
    let mut values = input.values.clone();
    if let Some(id) = values.building_id.as_deref().filter(|id| *id != "0") {
        let exists: Option<String> = sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM t_project_building WHERE id=? AND delete_flag='0' FOR UPDATE")
            .bind(id).fetch_optional(&mut *tx).await.map_err(db)?;
        if exists.is_none() { return Err(AppError::Conflict("所选空间已删除，请重新选择".into())); }
        let spaces = project_spaces::read(&mut *tx, None).await?;
        resolve_inventory_space(&mut values, &spaces)?;
    }
    let duplicate: Option<String> = sqlx::query_scalar("SELECT CAST(id AS CHAR) FROM op_edge_aio_server WHERE id<>? AND (name=? OR ip=?) LIMIT 1 FOR UPDATE")
        .bind(&base.id).bind(&values.name).bind(&values.ip).fetch_optional(&mut *tx).await.map_err(db)?;
    if duplicate.is_some() { return Err(AppError::Conflict("名称或 IP 已被另一台平台一体机使用，请核对".into())); }
    sqlx::query("UPDATE op_edge_aio_server SET name=?,ip=?,building_id=?,addr_alias=? WHERE id=?")
        .bind(&values.name).bind(&values.ip).bind(values.building_id.as_deref().unwrap_or("0"))
        .bind(values.addr_alias.as_deref().unwrap_or("")).bind(&base.id).execute(&mut *tx).await.map_err(db)?;
    let update_shared = format!("UPDATE `{shared_schema}`.aio_node SET name=?,ip=?,building_id=?,addr_alias=?,location=?,platform_aio_id=?,version=version+1,updated_at=UTC_TIMESTAMP(6) WHERE mac_normalized=?");
    sqlx::query(&update_shared).bind(&values.name).bind(&values.ip).bind(&values.building_id).bind(&values.addr_alias).bind(&values.addr_alias).bind(&base.id).bind(&mac).execute(&mut *tx).await.map_err(db)?;
    let audit = serde_json::json!({"before":{"name":before_name,"ip":before_ip,"buildingId":before_space,"addrAlias":before_location},"after":{"name":values.name,"ip":values.ip,"buildingId":values.building_id,"addrAlias":values.addr_alias}}).to_string();
    let sql = format!("UPDATE `{shared_schema}`.operation_record SET state='succeeded',success_count=1,ended_at=UTC_TIMESTAMP(6),heartbeat_at=UTC_TIMESTAMP(6),result_summary='资料已保存到平台',version=version+1 WHERE id=? AND domain_type='aio' AND operation_type='asset_edit' AND state='running'");
    if sqlx::query(&sql).bind(&grant.operation_id).execute(&mut *tx).await.map_err(db)?.rows_affected() != 1 { return Err(AppError::Conflict("本次操作已结束，请刷新详情".into())); }
    let sql = format!("UPDATE `{shared_schema}`.operation_target_result SET result_state='succeeded',result_summary='资料已保存到平台',completed_at=UTC_TIMESTAMP(6) WHERE operation_id=? AND resource_type='aio' AND resource_key=? AND result_state='pending'");
    if sqlx::query(&sql).bind(&grant.operation_id).bind(&mac).execute(&mut *tx).await.map_err(db)?.rows_affected() != 1 { return Err(AppError::Conflict("操作目标已变化，请刷新详情".into())); }
    let sql = format!("INSERT INTO `{shared_schema}`.audit_event(id,domain_type,object_type,object_key,action,operator_name,instance_id,changed_fields_json,created_at) VALUES(?,'aio','aio_node',?,'asset_edit',?,?,?,UTC_TIMESTAMP(6))");
    sqlx::query(&sql).bind(uuid::Uuid::now_v7().to_string()).bind(&mac).bind(operator).bind(&grant.owner_instance_id).bind(audit).execute(&mut *tx).await.map_err(db)?;
    tx.commit().await.map_err(|_| AppError::Conflict("保存结果尚未确认，请刷新详情核对，暂勿重复提交".into()))
}
