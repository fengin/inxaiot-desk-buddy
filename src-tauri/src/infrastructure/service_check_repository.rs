use std::collections::HashMap;

use sqlx::{Row, SqlitePool};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::service_check::{NodeServiceCheckSnapshot, ServiceCheckReport};

/// 按本机项目隔离的最近一次有效服务观测及最近检查尝试。
#[derive(Clone)]
pub struct ServiceCheckRepository {
    pool: SqlitePool,
    local_project_id: String,
}

impl ServiceCheckRepository {
    pub fn new(pool: SqlitePool, local_project_id: impl Into<String>) -> Self {
        Self {
            pool,
            local_project_id: local_project_id.into(),
        }
    }

    pub async fn list(&self) -> AppResult<HashMap<String, NodeServiceCheckSnapshot>> {
        let rows = sqlx::query("SELECT mac_normalized, snapshot_json FROM local_aio_service_check WHERE local_project_id = ?")
            .bind(&self.local_project_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::database("读取一体机服务检查记录", &error))?;
        rows.into_iter()
            .map(|row| {
                let mac = row
                    .try_get("mac_normalized")
                    .map_err(|error| AppError::database("解析服务检查MAC", &error))?;
                let json: String = row
                    .try_get("snapshot_json")
                    .map_err(|error| AppError::database("解析服务检查快照", &error))?;
                Ok((mac, decode_snapshot(&json)?))
            })
            .collect()
    }

    pub async fn get(&self, mac: &str) -> AppResult<Option<NodeServiceCheckSnapshot>> {
        let mac = MacAddress::parse(mac)?;
        let row = sqlx::query(
            "SELECT snapshot_json FROM local_aio_service_check WHERE local_project_id = ? AND mac_normalized = ?",
        )
        .bind(&self.local_project_id)
        .bind(mac.normalized())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取一体机服务检查详情", &error))?;
        row.map(|row| {
            let json: String = row
                .try_get("snapshot_json")
                .map_err(|error| AppError::database("解析服务检查快照", &error))?;
            decode_snapshot(&json)
        })
        .transpose()
    }

    pub async fn save_report(
        &self,
        mac: &str,
        report: &ServiceCheckReport,
    ) -> AppResult<NodeServiceCheckSnapshot> {
        let mac = MacAddress::parse(mac)?;
        report.validate()?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始保存服务检查记录", &error))?;
        // 在读取前先执行写语句取得 SQLite 写锁，避免两个事务读到同一旧快照后互相覆盖。
        sqlx::query(concat!(
            "INSERT INTO local_aio_service_check (local_project_id, mac_normalized, snapshot_json, revision, updated_at) ",
            "VALUES (?, ?, ?, 0, ?) ON CONFLICT (local_project_id, mac_normalized) DO NOTHING"
        )).bind(&self.local_project_id).bind(mac.normalized())
            .bind(encode_snapshot(&NodeServiceCheckSnapshot::default())?)
            .bind(timestamp()?).execute(&mut *transaction).await
            .map_err(|error| AppError::database("锁定服务检查记录", &error))?;
        let row = sqlx::query(
            "SELECT snapshot_json FROM local_aio_service_check WHERE local_project_id = ? AND mac_normalized = ?",
        )
        .bind(&self.local_project_id)
        .bind(mac.normalized())
        .fetch_one(&mut *transaction)
        .await
        .map_err(|error| AppError::database("读取待合并服务检查记录", &error))?;
        let json: String = row
            .try_get("snapshot_json")
            .map_err(|error| AppError::database("解析服务检查快照", &error))?;
        let mut snapshot = decode_snapshot(&json)?;
        if snapshot.apply_report(report)? {
            sqlx::query(concat!(
                "UPDATE local_aio_service_check SET snapshot_json = ?, revision = revision + 1, ",
                "updated_at = ? WHERE local_project_id = ? AND mac_normalized = ?"
            ))
            .bind(encode_snapshot(&snapshot)?)
            .bind(timestamp()?)
            .bind(&self.local_project_id)
            .bind(mac.normalized())
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("保存服务检查快照", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交服务检查记录", &error))?;
        Ok(snapshot)
    }
}

fn timestamp() -> AppResult<String> {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .map_err(|_| AppError::InvalidConfig("服务检查保存时间无法格式化".into()))
}

fn decode_snapshot(json: &str) -> AppResult<NodeServiceCheckSnapshot> {
    serde_json::from_str(json).map_err(|_| AppError::InvalidConfig("服务检查快照格式无效".into()))
}

fn encode_snapshot(snapshot: &NodeServiceCheckSnapshot) -> AppResult<String> {
    serde_json::to_string(snapshot)
        .map_err(|_| AppError::InvalidConfig("服务检查快照无法序列化".into()))
}
