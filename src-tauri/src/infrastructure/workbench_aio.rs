use sqlx::{MySqlPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::assets::{
    InventoryApplyResult, OperationRecordSummary, ServiceVersionRecord,
};
use crate::domain::aio::inventory::WorkbenchNodeSnapshot;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryAssetWrite {
    pub mac_normalized: String,
    pub display_mac: String,
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
    pub expected_version: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyInventoryWrite {
    pub file_name: String,
    pub operator_name: String,
    pub instance_id: String,
    pub classification_counts: serde_json::Value,
    pub assets: Vec<InventoryAssetWrite>,
}

#[derive(Clone)]
pub struct WorkbenchAioRepository {
    pool: MySqlPool,
}

impl WorkbenchAioRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn list_snapshots(&self) -> AppResult<Vec<WorkbenchNodeSnapshot>> {
        let rows = sqlx::query(concat!(
            "SELECT mac_normalized, name, ip, building_id, region_id, addr_alias, floor, ",
            "location, remark, platform_aio_id, management_state, source, last_operation_id, version ",
            "FROM aio_node ORDER BY mac_normalized"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("读取工作台一体机资产", &error))?;
        rows.into_iter().map(map_snapshot).collect()
    }

    pub async fn get_snapshot(
        &self,
        mac_normalized: &str,
    ) -> AppResult<Option<WorkbenchNodeSnapshot>> {
        let row = sqlx::query(concat!(
            "SELECT mac_normalized, name, ip, building_id, region_id, addr_alias, floor, ",
            "location, remark, platform_aio_id, management_state, source, last_operation_id, version ",
            "FROM aio_node WHERE mac_normalized = ?"
        ))
        .bind(mac_normalized)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取工作台一体机资产", &error))?;
        row.map(map_snapshot).transpose()
    }

    pub async fn list_service_versions(&self) -> AppResult<Vec<ServiceVersionRecord>> {
        let rows = sqlx::query(concat!(
            "SELECT mac_normalized, service_name, expected_image_name, expected_version, ",
            "observed_image_name, observed_version, ",
            "DATE_FORMAT(observed_at, '%Y-%m-%dT%H:%i:%s') AS observed_at_text ",
            "FROM aio_node_service_version ORDER BY mac_normalized, service_name"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("读取一体机服务版本", &error))?;
        rows.into_iter()
            .map(|row| -> AppResult<ServiceVersionRecord> {
                Ok(ServiceVersionRecord {
                    mac_normalized: row
                        .try_get("mac_normalized")
                        .map_err(|error| AppError::database("解析服务版本MAC", &error))?,
                    service_name: row
                        .try_get("service_name")
                        .map_err(|error| AppError::database("解析服务版本名称", &error))?,
                    expected_image_name: row
                        .try_get("expected_image_name")
                        .map_err(|error| AppError::database("解析期望镜像名", &error))?,
                    expected_version: row
                        .try_get("expected_version")
                        .map_err(|error| AppError::database("解析期望服务版本", &error))?,
                    observed_image_name: row
                        .try_get("observed_image_name")
                        .map_err(|error| AppError::database("解析观测镜像名", &error))?,
                    observed_version: row
                        .try_get("observed_version")
                        .map_err(|error| AppError::database("解析观测服务版本", &error))?,
                    observed_at: row
                        .try_get("observed_at_text")
                        .map_err(|error| AppError::database("解析服务观测时间", &error))?,
                })
            })
            .collect::<AppResult<Vec<_>>>()
    }

    pub async fn list_last_operations(
        &self,
    ) -> AppResult<std::collections::HashMap<String, OperationRecordSummary>> {
        let rows = sqlx::query(concat!(
            "SELECT n.mac_normalized, o.id, o.operation_type, o.operation_name, o.state, ",
            "o.operator_name, DATE_FORMAT(o.ended_at, '%Y-%m-%dT%H:%i:%s') AS ended_at_text, ",
            "o.result_summary FROM aio_node n ",
            "JOIN operation_record o ON o.id = n.last_operation_id"
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("读取一体机最近操作", &error))?;
        rows.into_iter()
            .map(|row| -> AppResult<(String, OperationRecordSummary)> {
                let mac = row
                    .try_get("mac_normalized")
                    .map_err(|error| AppError::database("解析最近操作MAC", &error))?;
                Ok((
                    mac,
                    OperationRecordSummary {
                        id: row
                            .try_get("id")
                            .map_err(|error| AppError::database("解析最近操作ID", &error))?,
                        operation_type: row
                            .try_get("operation_type")
                            .map_err(|error| AppError::database("解析最近操作类型", &error))?,
                        operation_name: row
                            .try_get("operation_name")
                            .map_err(|error| AppError::database("解析最近操作名称", &error))?,
                        state: row
                            .try_get("state")
                            .map_err(|error| AppError::database("解析最近操作状态", &error))?,
                        operator_name: row
                            .try_get("operator_name")
                            .map_err(|error| AppError::database("解析最近操作用户", &error))?,
                        ended_at: row
                            .try_get("ended_at_text")
                            .map_err(|error| AppError::database("解析最近操作结束时间", &error))?,
                        result_summary: row
                            .try_get("result_summary")
                            .map_err(|error| AppError::database("解析最近操作摘要", &error))?,
                    },
                ))
            })
            .collect::<AppResult<std::collections::HashMap<_, _>>>()
    }

    pub async fn apply_inventory(
        &self,
        write: ApplyInventoryWrite,
    ) -> AppResult<InventoryApplyResult> {
        if write.file_name.trim().is_empty()
            || write.operator_name.trim().is_empty()
            || write.instance_id.trim().is_empty()
            || write.assets.is_empty()
        {
            return Err(AppError::InvalidConfig(
                "导入文件、操作人、实例和待应用资产不能为空".into(),
            ));
        }
        let operation_id = Uuid::now_v7().to_string();
        let now = OffsetDateTime::now_utc();
        let summary = serde_json::json!({
            "schemaVersion": 1,
            "fileName": write.file_name,
            "classificationCounts": write.classification_counts,
        })
        .to_string();
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始导入应用事务", &error))?;
        sqlx::query(concat!(
            "INSERT INTO operation_record ",
            "(id, domain_type, operation_type, operation_name, operator_name, instance_id, ",
            "state, target_count, success_count, failure_count, cancelled_count, artifact_name, ",
            "operation_summary_json, started_at, ended_at, heartbeat_at, result_summary, version) ",
            "VALUES (?, 'aio', 'inventory_import', '导入一体机清单', ?, ?, 'succeeded', ",
            "?, ?, 0, 0, ?, ?, ?, ?, ?, ?, 1)"
        ))
        .bind(&operation_id)
        .bind(write.operator_name.trim())
        .bind(write.instance_id.trim())
        .bind(u32::try_from(write.assets.len()).unwrap_or(u32::MAX))
        .bind(u32::try_from(write.assets.len()).unwrap_or(u32::MAX))
        .bind(write.file_name.trim())
        .bind(summary)
        .bind(now)
        .bind(now)
        .bind(now)
        .bind(format!("成功应用 {} 台一体机最终资产", write.assets.len()))
        .execute(&mut *transaction)
        .await
        .map_err(|error| AppError::database("创建导入操作摘要", &error))?;

        for asset in &write.assets {
            let (old_version, new_version, action) = if let Some(expected_version) =
                asset.expected_version
            {
                let result = sqlx::query(concat!(
                        "UPDATE aio_node SET name = ?, ip = ?, display_mac = ?, building_id = ?, ",
                        "region_id = ?, addr_alias = ?, floor = ?, location = ?, remark = ?, ",
                        "platform_aio_id = ?, management_state = ?, source = ?, last_operation_id = ?, ",
                        "version = version + 1, updated_at = ? ",
                        "WHERE mac_normalized = ? AND version = ?"
                    ))
                    .bind(asset.name.trim())
                    .bind(asset.ip.trim())
                    .bind(&asset.display_mac)
                    .bind(&asset.building_id)
                    .bind(&asset.region_id)
                    .bind(&asset.addr_alias)
                    .bind(&asset.floor)
                    .bind(&asset.location)
                    .bind(&asset.remark)
                    .bind(&asset.platform_aio_id)
                    .bind(&asset.management_state)
                    .bind(&asset.source)
                    .bind(&operation_id)
                    .bind(now)
                    .bind(&asset.mac_normalized)
                    .bind(expected_version)
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| map_write_error("更新一体机最终资产", error))?;
                if result.rows_affected() != 1 {
                    return Err(AppError::Conflict(format!(
                        "一体机 {} 已被其他实例更新",
                        asset.display_mac
                    )));
                }
                (
                    Some(expected_version),
                    expected_version + 1,
                    "inventory_update",
                )
            } else {
                sqlx::query(concat!(
                    "INSERT INTO aio_node ",
                    "(mac_normalized, name, ip, display_mac, building_id, region_id, addr_alias, ",
                    "floor, location, remark, platform_aio_id, management_state, source, ",
                    "last_operation_id, version, created_at, updated_at) ",
                    "VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)"
                ))
                .bind(&asset.mac_normalized)
                .bind(asset.name.trim())
                .bind(asset.ip.trim())
                .bind(&asset.display_mac)
                .bind(&asset.building_id)
                .bind(&asset.region_id)
                .bind(&asset.addr_alias)
                .bind(&asset.floor)
                .bind(&asset.location)
                .bind(&asset.remark)
                .bind(&asset.platform_aio_id)
                .bind(&asset.management_state)
                .bind(&asset.source)
                .bind(&operation_id)
                .bind(now)
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(|error| map_write_error("创建一体机最终资产", error))?;
                (None, 1, "inventory_create")
            };
            sqlx::query(concat!(
                "INSERT INTO audit_event ",
                "(id, domain_type, object_type, object_key, action, operator_name, instance_id, ",
                "old_version, new_version, changed_fields_json, created_at) ",
                "VALUES (?, 'aio', 'aio_node', ?, ?, ?, ?, ?, ?, ?, ?)"
            ))
            .bind(Uuid::now_v7().to_string())
            .bind(&asset.mac_normalized)
            .bind(action)
            .bind(write.operator_name.trim())
            .bind(write.instance_id.trim())
            .bind(old_version)
            .bind(new_version)
            .bind(
                serde_json::json!([
                    "name",
                    "ip",
                    "building_id",
                    "region_id",
                    "addr_alias",
                    "floor",
                    "location",
                    "remark",
                    "platform_aio_id",
                    "management_state",
                    "source",
                    "last_operation_id"
                ])
                .to_string(),
            )
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("记录导入资产审计", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交导入应用事务", &error))?;
        Ok(InventoryApplyResult {
            operation_id,
            applied_count: u32::try_from(write.assets.len()).unwrap_or(u32::MAX),
        })
    }

    pub async fn mark_operation_success(
        &self,
        mac_normalized: &str,
        operation_id: &str,
    ) -> AppResult<()> {
        let result = sqlx::query(
            "UPDATE aio_node SET last_operation_id = ?, management_state = 'managed', \
             version = version + 1, updated_at = UTC_TIMESTAMP(6) WHERE mac_normalized = ?",
        )
        .bind(operation_id)
        .bind(mac_normalized)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("更新一体机最近成功操作", &error))?;
        if result.rows_affected() != 1 {
            return Err(AppError::NotFound(format!(
                "一体机最终资产不存在：{mac_normalized}"
            )));
        }
        Ok(())
    }
}

fn map_write_error(operation: &'static str, error: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(database) = &error
        && database.is_unique_violation()
    {
        tracing::warn!(operation, error = ?crate::core::log_safety::safe_error(&error), "aio inventory write conflict");
        return AppError::Conflict("一体机 MAC、IP 或平台关联已经存在".into());
    }
    AppError::database(operation, &error)
}

fn map_snapshot(row: sqlx::mysql::MySqlRow) -> AppResult<WorkbenchNodeSnapshot> {
    Ok(WorkbenchNodeSnapshot {
        mac_normalized: row
            .try_get("mac_normalized")
            .map_err(|error| AppError::database("解析一体机MAC", &error))?,
        name: row
            .try_get("name")
            .map_err(|error| AppError::database("解析一体机名称", &error))?,
        ip: row
            .try_get("ip")
            .map_err(|error| AppError::database("解析一体机IP", &error))?,
        building_id: row
            .try_get("building_id")
            .map_err(|error| AppError::database("解析一体机楼宇ID", &error))?,
        region_id: row
            .try_get("region_id")
            .map_err(|error| AppError::database("解析一体机区域ID", &error))?,
        addr_alias: row
            .try_get("addr_alias")
            .map_err(|error| AppError::database("解析一体机地址别名", &error))?,
        floor: row
            .try_get("floor")
            .map_err(|error| AppError::database("解析一体机楼层", &error))?,
        location: row
            .try_get("location")
            .map_err(|error| AppError::database("解析一体机位置", &error))?,
        remark: row
            .try_get("remark")
            .map_err(|error| AppError::database("解析一体机备注", &error))?,
        platform_aio_id: row
            .try_get("platform_aio_id")
            .map_err(|error| AppError::database("解析平台一体机ID", &error))?,
        management_state: row
            .try_get("management_state")
            .map_err(|error| AppError::database("解析一体机管理状态", &error))?,
        source: row
            .try_get("source")
            .map_err(|error| AppError::database("解析一体机来源", &error))?,
        last_operation_id: row
            .try_get("last_operation_id")
            .map_err(|error| AppError::database("解析最近操作ID", &error))?,
        version: row
            .try_get::<u64, _>("version")
            .map_err(|error| AppError::database("解析一体机版本", &error))?,
    })
}
