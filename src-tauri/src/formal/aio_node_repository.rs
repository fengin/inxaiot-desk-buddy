use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use super::error::{FormalError, FormalResult};
use super::mac::{display_mac, normalize_mac};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeValues {
    pub mac: String,
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
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AioNodeRecord {
    pub mac_normalized: String,
    pub display_mac: String,
    pub name: String,
    pub ip: String,
    pub platform_aio_id: Option<String>,
    pub management_state: String,
    pub source: String,
    pub version: u64,
}

#[derive(Clone, Debug)]
pub struct AioNodeWrite {
    pub values: AioNodeValues,
    pub expected_version: Option<u64>,
    pub action: String,
    pub operator_name: String,
    pub instance_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceVersionWrite {
    pub mac: String,
    pub service_name: String,
    pub expected_image_name: Option<String>,
    pub expected_version: Option<String>,
    pub observed_image_name: Option<String>,
    pub observed_version: Option<String>,
    pub source_operation_id: Option<String>,
}

#[derive(Clone)]
pub struct AioNodeRepository {
    pool: MySqlPool,
}

impl AioNodeRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn save(&self, write: AioNodeWrite) -> FormalResult<AioNodeRecord> {
        validate_write(&write)?;
        let mac = normalize_mac(&write.values.mac)?;
        let display = display_mac(&mac)?;
        let now = OffsetDateTime::now_utc();
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin aio node transaction failed");
            FormalError::LocalDatabase("开始一体机资产事务")
        })?;
        let (old_version, new_version, action) = if let Some(expected) = write.expected_version {
            let result = sqlx::query(
                "UPDATE aio_node SET name = ?, ip = ?, display_mac = ?, building_id = ?, \
                 region_id = ?, addr_alias = ?, floor = ?, location = ?, remark = ?, \
                 platform_aio_id = ?, management_state = ?, source = ?, version = version + 1, \
                 updated_at = ? WHERE mac_normalized = ? AND version = ?",
            )
            .bind(write.values.name.trim())
            .bind(write.values.ip.trim())
            .bind(&display)
            .bind(&write.values.building_id)
            .bind(&write.values.region_id)
            .bind(&write.values.addr_alias)
            .bind(&write.values.floor)
            .bind(&write.values.location)
            .bind(&write.values.remark)
            .bind(&write.values.platform_aio_id)
            .bind(&write.values.management_state)
            .bind(&write.values.source)
            .bind(now)
            .bind(&mac)
            .bind(expected)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("更新一体机资产", error))?;
            if result.rows_affected() != 1 {
                return Err(FormalError::Conflict("一体机资产已被其他实例更新".into()));
            }
            (Some(expected), expected + 1, write.action.as_str())
        } else {
            sqlx::query(
                "INSERT INTO aio_node \
                 (mac_normalized, name, ip, display_mac, building_id, region_id, addr_alias, floor, \
                  location, remark, platform_aio_id, management_state, source, version, created_at, updated_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
            )
            .bind(&mac)
            .bind(write.values.name.trim())
            .bind(write.values.ip.trim())
            .bind(&display)
            .bind(&write.values.building_id)
            .bind(&write.values.region_id)
            .bind(&write.values.addr_alias)
            .bind(&write.values.floor)
            .bind(&write.values.location)
            .bind(&write.values.remark)
            .bind(&write.values.platform_aio_id)
            .bind(&write.values.management_state)
            .bind(&write.values.source)
            .bind(now)
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("创建一体机资产", error))?;
            (None, 1, write.action.as_str())
        };
        sqlx::query(
            "INSERT INTO audit_event \
             (id, domain_type, object_type, object_key, action, operator_name, instance_id, \
              old_version, new_version, changed_fields_json, created_at) \
             VALUES (?, 'aio', 'aio_node', ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::now_v7().to_string())
        .bind(&mac)
        .bind(action)
        .bind(&write.operator_name)
        .bind(&write.instance_id)
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
                "source"
            ])
            .to_string(),
        )
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(|error| map_error("记录一体机资产审计", error))?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit aio node transaction failed");
            FormalError::LocalDatabase("提交一体机资产事务")
        })?;
        self.get(&mac).await
    }

    pub async fn get(&self, mac: &str) -> FormalResult<AioNodeRecord> {
        let mac = normalize_mac(mac)?;
        let row = sqlx::query(
            "SELECT mac_normalized, display_mac, name, ip, platform_aio_id, management_state, source, version \
             FROM aio_node WHERE mac_normalized = ?",
        )
        .bind(&mac)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| map_error("读取一体机资产", error))?
        .ok_or_else(|| FormalError::NotFound(format!("一体机不存在：{mac}")))?;
        map_node(row)
    }

    pub async fn save_service_version(&self, write: ServiceVersionWrite) -> FormalResult<()> {
        let mac = normalize_mac(&write.mac)?;
        if write.service_name.trim().is_empty() {
            return Err(FormalError::InvalidConfig("服务名不能为空".into()));
        }
        sqlx::query(
            "INSERT INTO aio_node_service_version \
             (mac_normalized, service_name, expected_image_name, expected_version, \
              observed_image_name, observed_version, observed_at, source_operation_id) \
             VALUES (?, ?, ?, ?, ?, ?, UTC_TIMESTAMP(6), ?) \
             ON DUPLICATE KEY UPDATE expected_image_name = VALUES(expected_image_name), \
             expected_version = VALUES(expected_version), observed_image_name = VALUES(observed_image_name), \
             observed_version = VALUES(observed_version), observed_at = VALUES(observed_at), \
             source_operation_id = VALUES(source_operation_id)",
        )
        .bind(&mac)
        .bind(write.service_name.trim())
        .bind(&write.expected_image_name)
        .bind(&write.expected_version)
        .bind(&write.observed_image_name)
        .bind(&write.observed_version)
        .bind(&write.source_operation_id)
        .execute(&self.pool)
        .await
        .map_err(|error| map_error("保存一体机服务版本", error))?;
        Ok(())
    }

    pub async fn delete_test_node(&self, mac: &str) -> FormalResult<()> {
        let mac = normalize_mac(mac)?;
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin aio node cleanup failed");
            FormalError::LocalDatabase("开始测试一体机清理事务")
        })?;
        sqlx::query("DELETE FROM audit_event WHERE object_type = 'aio_node' AND object_key = ?")
            .bind(&mac)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("清理一体机审计", error))?;
        sqlx::query("DELETE FROM aio_node WHERE mac_normalized = ?")
            .bind(&mac)
            .execute(&mut *transaction)
            .await
            .map_err(|error| map_error("清理一体机资产", error))?;
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit aio node cleanup failed");
            FormalError::LocalDatabase("提交测试一体机清理事务")
        })?;
        Ok(())
    }
}

fn validate_write(write: &AioNodeWrite) -> FormalResult<()> {
    if write.values.name.trim().is_empty()
        || write.values.ip.trim().is_empty()
        || write.values.management_state.trim().is_empty()
        || write.values.source.trim().is_empty()
        || write.action.trim().is_empty()
        || write.operator_name.trim().is_empty()
        || write.instance_id.trim().is_empty()
    {
        return Err(FormalError::InvalidConfig("一体机资产参数不完整".into()));
    }
    Ok(())
}

fn map_node(row: sqlx::mysql::MySqlRow) -> FormalResult<AioNodeRecord> {
    Ok(AioNodeRecord {
        mac_normalized: row
            .try_get("mac_normalized")
            .map_err(|_| FormalError::LocalDatabase("解析一体机MAC"))?,
        display_mac: row
            .try_get("display_mac")
            .map_err(|_| FormalError::LocalDatabase("解析显示MAC"))?,
        name: row
            .try_get("name")
            .map_err(|_| FormalError::LocalDatabase("解析一体机名称"))?,
        ip: row
            .try_get("ip")
            .map_err(|_| FormalError::LocalDatabase("解析一体机IP"))?,
        platform_aio_id: row
            .try_get("platform_aio_id")
            .map_err(|_| FormalError::LocalDatabase("解析平台一体机ID"))?,
        management_state: row
            .try_get("management_state")
            .map_err(|_| FormalError::LocalDatabase("解析管理状态"))?,
        source: row
            .try_get("source")
            .map_err(|_| FormalError::LocalDatabase("解析资产来源"))?,
        version: row
            .try_get("version")
            .map_err(|_| FormalError::LocalDatabase("解析资产版本"))?,
    })
}

fn map_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?crate::core::log_safety::safe_error(&error), "aio node mysql operation failed");
    FormalError::LocalDatabase(operation)
}
