use serde::{Deserialize, Serialize};
use sqlx::{MySqlPool, Row};

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::assets::PlatformRecordIssue;
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformInventorySnapshot {
    pub nodes: Vec<PlatformNodeSnapshot>,
    pub issues: Vec<PlatformRecordIssue>,
}

impl PlatformInventorySnapshot {
    /// 内部锁和资产按规范化 MAC 匹配；平台接口按原始文本匹配，部署配置必须保留该写法。
    pub fn deployment_mac(&self, mac: &str) -> AppResult<String> {
        let normalized = MacAddress::parse(mac)?.normalized().to_string();
        let matches = self
            .nodes
            .iter()
            .filter(|node| node.mac_normalized == normalized)
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => Ok(normalized),
            [node] => Ok(node.mac_raw.clone()),
            _ => Err(AppError::Conflict(format!(
                "平台中同一 MAC（{normalized}）有多条一体机记录，请先在平台处理重复记录后重试"
            ))),
        }
    }
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
            .map(|row| {
                row.try_get::<String, _>("column_name")
                    .map_err(|error| AppError::database("解析平台一体机表字段", &error))
            })
            .collect::<AppResult<Vec<_>>>()?;
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
            let name: String = row
                .try_get("name_text")
                .map_err(|error| AppError::database("解析平台一体机名称", &error))?;
            let ip: String = row
                .try_get("ip_text")
                .map_err(|error| AppError::database("解析平台一体机IP", &error))?;
            let raw_mac = row
                .try_get::<String, _>("mac_text")
                .map_err(|error| AppError::database("解析平台一体机MAC", &error))?;
            let mac = match MacAddress::parse(&raw_mac) {
                Ok(mac) => mac,
                Err(_) => {
                    snapshot
                        .issues
                        .push(invalid_mac_issue(id, name, ip, raw_mac));
                    continue;
                }
            };
            let last_beat_text: Option<String> = row
                .try_get("last_beat_text")
                .map_err(|error| AppError::database("解析平台心跳时间", &error))?;
            let last_beat_time = last_beat_text
                .map(|value| {
                    value
                        .parse::<i64>()
                        .map_err(|_| AppError::InvalidConfig("平台一体机心跳时间格式无效".into()))
                })
                .transpose()?;
            let status_value: Option<i64> = row
                .try_get("status_value")
                .map_err(|error| AppError::database("解析平台一体机状态", &error))?;
            snapshot.nodes.push(PlatformNodeSnapshot {
                id,
                name,
                ip,
                mac_raw: raw_mac,
                mac_normalized: mac.normalized().into(),
                building_id: row
                    .try_get("building_id_text")
                    .map_err(|error| AppError::database("解析平台楼宇ID", &error))?,
                addr_alias: row
                    .try_get("addr_alias")
                    .map_err(|error| AppError::database("解析平台地址别名", &error))?,
                status: status_value
                    .map(|value| {
                        i32::try_from(value).map_err(|_| {
                            AppError::InvalidConfig("平台一体机状态超出整数范围".into())
                        })
                    })
                    .transpose()?,
                last_beat_time,
                last_sync_time: row
                    .try_get("last_sync_time_text")
                    .map_err(|error| AppError::database("解析平台同步时间", &error))?,
            });
        }
        Ok(snapshot)
    }
}

fn invalid_mac_issue(
    platform_aio_id: String,
    name: String,
    ip: String,
    raw_mac: String,
) -> PlatformRecordIssue {
    PlatformRecordIssue {
        platform_aio_id,
        name,
        ip,
        code: "PLATFORM_MAC_INVALID".into(),
        message: if raw_mac.trim().is_empty() {
            "未填写 MAC 地址".into()
        } else {
            "MAC 地址格式无效".into()
        },
        raw_mac,
    }
}

#[cfg(test)]
mod tests {
    use super::{PLATFORM_AIO_SELECT, PlatformInventorySnapshot, invalid_mac_issue};
    use crate::domain::aio::inventory::PlatformNodeSnapshot;

    #[test]
    fn deployment_preserves_platform_mac_text_and_rejects_duplicate_identity() {
        let mut snapshot = PlatformInventorySnapshot::default();
        assert_eq!(
            snapshot.deployment_mac("00:0c:29:3b:b9:33").unwrap(),
            "000C293BB933"
        );
        let node = PlatformNodeSnapshot {
            id: "old-id".into(),
            name: "existing".into(),
            ip: "192.168.3.79".into(),
            mac_raw: "00:0c:29:3b:b9:33".into(),
            mac_normalized: "000C293BB933".into(),
            building_id: None,
            addr_alias: None,
            status: Some(1),
            last_beat_time: None,
            last_sync_time: None,
        };
        snapshot.nodes.push(node.clone());
        assert_eq!(
            snapshot.deployment_mac("000C293BB933").unwrap(),
            node.mac_raw
        );
        assert_eq!(
            snapshot.deployment_mac("00-0C-29-3B-B9-33").unwrap(),
            node.mac_raw
        );
        let mut duplicate = node;
        duplicate.id = "new-id".into();
        duplicate.mac_raw = "000C293BB933".into();
        snapshot.nodes.push(duplicate);
        assert!(snapshot.deployment_mac("000C293BB933").is_err());
        assert!(snapshot.deployment_mac("invalid").is_err());
    }

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

    #[test]
    fn invalid_mac_issue_keeps_the_platform_record_locator() {
        let issue = invalid_mac_issue(
            "42".into(),
            "东区弱电间".into(),
            "192.168.3.42".into(),
            "bad-mac".into(),
        );
        assert_eq!(issue.platform_aio_id, "42");
        assert_eq!(issue.name, "东区弱电间");
        assert_eq!(issue.ip, "192.168.3.42");
        assert_eq!(issue.raw_mac, "bad-mac");
        assert_eq!(issue.message, "MAC 地址格式无效");
        assert_eq!(
            invalid_mac_issue(
                "43".into(),
                "西区弱电间".into(),
                "192.168.3.43".into(),
                "".into()
            )
            .message,
            "未填写 MAC 地址"
        );
    }
}

pub async fn require_aio_schema(pool: &sqlx::MySqlPool) -> crate::core::error::AppResult<()> {
    crate::infrastructure::database::require_table_columns(
        pool,
        "op_edge_aio_server",
        &[
            "id",
            "name",
            "ip",
            "mac",
            "building_id",
            "addr_alias",
            "status",
            "last_beat_time",
            "last_sync_time",
        ],
    )
    .await
}
