use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, Row};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::inventory::PlatformNodeSnapshot;
use crate::domain::aio::mac::MacAddress;

const PLATFORM_AIO_SELECT: &str = concat!(
    "SELECT CAST(id AS CHAR) AS id_text, COALESCE(name, '') AS name_text, ",
    "COALESCE(ip, '') AS ip_text, COALESCE(mac, '') AS mac_text, ",
    "CAST(building_id AS CHAR) AS building_id_text, addr_alias, ",
    "CAST(status AS SIGNED) AS status_value, CAST(last_beat_time AS CHAR) AS last_beat_text, ",
    "DATE_FORMAT(last_sync_time, '%Y-%m-%dT%H:%i:%s') AS last_sync_time_text ",
    "FROM op_edge_aio_server ORDER BY id"
);

const REQUIRED_COLUMNS: &[&str] = &[
    "id",
    "name",
    "ip",
    "mac",
    "building_id",
    "addr_alias",
    "status",
    "last_beat_time",
    "last_sync_time",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformRecordIssue {
    pub platform_aio_id: String,
    pub code: String,
    pub message: String,
    pub raw_mac: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInventorySnapshot {
    pub nodes: Vec<PlatformNodeSnapshot>,
    pub issues: Vec<PlatformRecordIssue>,
}

#[derive(Clone)]
pub struct PlatformAioRepository {
    pool: MySqlPool,
}

impl PlatformAioRepository {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn assert_read_capabilities(&self) -> AppResult<Vec<String>> {
        let schema = sqlx::query_scalar::<_, String>("SELECT DATABASE()")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::database("读取平台数据库名", &error))?;
        let rows = sqlx::query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = 'op_edge_aio_server'",
        )
        .bind(schema)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("探测平台一体机表字段", &error))?;
        let columns = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("column_name").ok())
            .collect::<Vec<_>>();
        let missing = REQUIRED_COLUMNS
            .iter()
            .filter(|column| !columns.iter().any(|item| item == **column))
            .copied()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(AppError::InvalidConfig(format!(
                "平台一体机表缺少字段：{}",
                missing.join("、")
            )));
        }
        Ok(columns)
    }

    pub async fn list_all(&self) -> AppResult<PlatformInventorySnapshot> {
        self.assert_read_capabilities().await?;
        let rows = sqlx::query(PLATFORM_AIO_SELECT)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::database("读取平台一体机列表", &error))?;
        let mut snapshot = PlatformInventorySnapshot::default();
        for row in rows {
            let id = row
                .try_get::<String, _>("id_text")
                .map_err(|error| AppError::database("解析平台一体机ID", &error))?;
            let raw_mac = row
                .try_get::<String, _>("mac_text")
                .map_err(|error| AppError::database("解析平台一体机MAC", &error))?;
            let mac = match MacAddress::parse(&raw_mac) {
                Ok(mac) => mac,
                Err(_) => {
                    snapshot.issues.push(PlatformRecordIssue {
                        platform_aio_id: id,
                        code: "PLATFORM_MAC_INVALID".into(),
                        message: "平台一体机 MAC 为空或格式无效，无法参与资产匹配".into(),
                        raw_mac,
                    });
                    continue;
                }
            };
            let last_beat_time = row
                .try_get::<Option<String>, _>("last_beat_text")
                .ok()
                .flatten()
                .and_then(|value| value.parse::<i64>().ok());
            snapshot.nodes.push(PlatformNodeSnapshot {
                id,
                name: row.try_get("name_text").unwrap_or_default(),
                ip: row.try_get("ip_text").unwrap_or_default(),
                mac_raw: raw_mac,
                mac_normalized: mac.normalized().into(),
                building_id: row.try_get("building_id_text").ok(),
                addr_alias: row.try_get("addr_alias").ok(),
                status: row
                    .try_get::<Option<i64>, _>("status_value")
                    .ok()
                    .flatten()
                    .and_then(|value| i32::try_from(value).ok()),
                last_beat_time,
                last_sync_time: row.try_get("last_sync_time_text").ok(),
            });
        }
        Ok(snapshot)
    }
}

#[cfg(test)]
mod tests {
    use super::PLATFORM_AIO_SELECT;

    #[test]
    fn platform_adapter_statement_is_strictly_read_only() {
        let normalized = PLATFORM_AIO_SELECT.trim().to_ascii_uppercase();
        assert!(normalized.starts_with("SELECT"));
        for forbidden in [
            "INSERT ", "UPDATE ", "DELETE ", "REPLACE ", "ALTER ", "DROP ",
        ] {
            assert!(!normalized.contains(forbidden));
        }
        assert!(normalized.contains("FROM OP_EDGE_AIO_SERVER"));
    }
}
