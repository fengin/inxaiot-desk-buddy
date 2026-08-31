use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sqlx::migrate::Migrator;
use sqlx::{MySqlPool, Row};

use super::error::{FormalError, FormalResult};

static WORKBENCH_MIGRATOR: Migrator = sqlx::migrate!("./migrations/workbench");

const EXPECTED_BUSINESS_TABLES: &[&str] = &[
    "aio_node",
    "aio_node_service_version",
    "aio_release_profile",
    "audit_event",
    "operation_record",
    "operation_target_result",
    "resource_lease",
];

const FORBIDDEN_PROJECT_TABLES: &[&str] = &[
    "workspace_meta",
    "aio_import_batch",
    "aio_import_item",
    "aio_operation_detail",
    "operation_target",
    "operation_step",
    "task",
    "task_step",
];

#[derive(Clone)]
pub struct WorkbenchStore {
    pool: MySqlPool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchSchemaAudit {
    pub business_tables: Vec<String>,
    pub missing_tables: Vec<String>,
    pub forbidden_tables: Vec<String>,
    pub migration_count: i64,
    pub release_credentials_are_encrypted: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkbenchSchemaState {
    Uninitialized,
    UpgradeRequired,
    Ready,
    Incompatible,
}

impl WorkbenchSchemaState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Uninitialized => "uninitialized",
            Self::UpgradeRequired => "upgrade_required",
            Self::Ready => "ready",
            Self::Incompatible => "incompatible",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchSchemaStatus {
    pub state: WorkbenchSchemaState,
    pub current_version: Option<i64>,
    pub latest_available_version: i64,
    pub applied_migration_count: i64,
    pub failed_migration_count: i64,
    pub missing_tables: Vec<String>,
    pub forbidden_tables: Vec<String>,
    pub message: String,
}

impl WorkbenchSchemaStatus {
    pub fn is_ready(&self) -> bool {
        self.state == WorkbenchSchemaState::Ready
    }

    pub fn can_upgrade(&self) -> bool {
        matches!(
            self.state,
            WorkbenchSchemaState::Uninitialized | WorkbenchSchemaState::UpgradeRequired
        )
    }
}

impl WorkbenchStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    pub async fn migrate(&self) -> FormalResult<()> {
        WORKBENCH_MIGRATOR.run(&self.pool).await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "run workbench mysql migration failed");
            FormalError::LocalDatabase("执行工作台MySQL迁移")
        })
    }

    pub async fn schema_status(&self, schema: &str) -> FormalResult<WorkbenchSchemaStatus> {
        let rows = sqlx::query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = ? AND table_type = 'BASE TABLE'",
        )
        .bind(schema)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("检查工作台MySQL表", error))?;
        let tables = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("table_name").ok())
            .collect::<BTreeSet<_>>();
        let missing_tables = EXPECTED_BUSINESS_TABLES
            .iter()
            .filter(|table| !tables.contains(**table))
            .map(|table| (*table).to_string())
            .collect::<Vec<_>>();
        let forbidden_tables = FORBIDDEN_PROJECT_TABLES
            .iter()
            .filter(|table| tables.contains(**table))
            .map(|table| (*table).to_string())
            .collect::<Vec<_>>();
        let migration_table_exists = tables.contains("_sqlx_migrations");
        let (current_version, applied_migration_count, failed_migration_count) =
            if migration_table_exists {
                let row = sqlx::query(
                    "SELECT MAX(CASE WHEN success = 1 THEN version END) AS current_version, \
                            COUNT(CASE WHEN success = 1 THEN 1 END) AS applied_count, \
                            COUNT(CASE WHEN success = 0 THEN 1 END) AS failed_count \
                     FROM _sqlx_migrations",
                )
                .fetch_one(&self.pool)
                .await
                .map_err(|error| map_error("读取工作台迁移状态", error))?;
                (
                    row.try_get::<Option<i64>, _>("current_version")
                        .ok()
                        .flatten(),
                    row.try_get::<Option<i64>, _>("applied_count")
                        .ok()
                        .flatten()
                        .unwrap_or(0),
                    row.try_get::<Option<i64>, _>("failed_count")
                        .ok()
                        .flatten()
                        .unwrap_or(0),
                )
            } else {
                (None, 0, 0)
            };
        let latest_available_version = latest_available_version();
        Ok(classify_schema_status(
            migration_table_exists,
            current_version,
            latest_available_version,
            applied_migration_count,
            failed_migration_count,
            missing_tables,
            forbidden_tables,
            tables
                .iter()
                .filter(|table| table.as_str() != "_sqlx_migrations")
                .count(),
        ))
    }

    pub async fn audit_schema(&self, schema: &str) -> FormalResult<WorkbenchSchemaAudit> {
        let rows = sqlx::query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = ? AND table_type = 'BASE TABLE'",
        )
        .bind(schema)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("审计工作台MySQL表", error))?;
        let tables = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("table_name").ok())
            .collect::<BTreeSet<_>>();
        let missing_tables = EXPECTED_BUSINESS_TABLES
            .iter()
            .filter(|table| !tables.contains(**table))
            .map(|table| (*table).to_string())
            .collect();
        let forbidden_tables = FORBIDDEN_PROJECT_TABLES
            .iter()
            .filter(|table| tables.contains(**table))
            .map(|table| (*table).to_string())
            .collect();
        let status = self.schema_status(schema).await?;
        let migration_count = status.applied_migration_count;
        let profile_columns = sqlx::query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = 'aio_release_profile'",
        )
        .bind(schema)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| map_error("审计发布配置密文字段", error))?
        .into_iter()
        .filter_map(|row| row.try_get::<String, _>("column_name").ok())
        .collect::<BTreeSet<_>>();
        let release_credentials_are_encrypted = [
            "credential_scheme",
            "credential_salt",
            "credential_nonce",
            "credential_ciphertext",
        ]
        .iter()
        .all(|column| profile_columns.contains(*column))
            && [
                "platform_auth_key",
                "platform_mqtt_password",
                "aio_mqtt_password",
                "ssh_password",
                "ssh_private_key",
            ]
            .iter()
            .all(|column| !profile_columns.contains(*column));
        Ok(WorkbenchSchemaAudit {
            business_tables: tables
                .into_iter()
                .filter(|table| table != "_sqlx_migrations")
                .collect(),
            missing_tables,
            forbidden_tables,
            migration_count,
            release_credentials_are_encrypted,
        })
    }
}

fn latest_available_version() -> i64 {
    WORKBENCH_MIGRATOR
        .iter()
        .map(|migration| migration.version)
        .max()
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
fn classify_schema_status(
    migration_table_exists: bool,
    current_version: Option<i64>,
    latest_available_version: i64,
    applied_migration_count: i64,
    failed_migration_count: i64,
    missing_tables: Vec<String>,
    forbidden_tables: Vec<String>,
    business_table_count: usize,
) -> WorkbenchSchemaStatus {
    let state = if !migration_table_exists {
        if business_table_count == 0 {
            WorkbenchSchemaState::Uninitialized
        } else {
            WorkbenchSchemaState::Incompatible
        }
    } else if failed_migration_count > 0
        || !forbidden_tables.is_empty()
        || current_version.is_some_and(|version| version > latest_available_version)
    {
        WorkbenchSchemaState::Incompatible
    } else if current_version.unwrap_or(0) < latest_available_version {
        WorkbenchSchemaState::UpgradeRequired
    } else if !missing_tables.is_empty() {
        WorkbenchSchemaState::Incompatible
    } else {
        WorkbenchSchemaState::Ready
    };
    let message = match state {
        WorkbenchSchemaState::Uninitialized => "工作台数据库尚未初始化".to_string(),
        WorkbenchSchemaState::UpgradeRequired => format!(
            "工作台数据库需要从版本 {} 升级到版本 {}",
            current_version.unwrap_or(0),
            latest_available_version
        ),
        WorkbenchSchemaState::Ready => {
            format!("工作台数据库结构已是最新版本 {latest_available_version}")
        }
        WorkbenchSchemaState::Incompatible => {
            "工作台数据库结构不兼容，需要开发或管理员人工处理".to_string()
        }
    };
    WorkbenchSchemaStatus {
        state,
        current_version,
        latest_available_version,
        applied_migration_count,
        failed_migration_count,
        missing_tables,
        forbidden_tables,
        message,
    }
}

fn map_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?crate::core::log_safety::safe_error(&error), "workbench mysql operation failed");
    FormalError::LocalDatabase(operation)
}

#[cfg(test)]
mod tests {
    use super::{WorkbenchSchemaState, classify_schema_status};

    #[test]
    fn schema_status_distinguishes_empty_upgrade_ready_and_incompatible() {
        let empty =
            classify_schema_status(false, None, 2, 0, 0, vec!["aio_node".into()], vec![], 0);
        assert_eq!(empty.state, WorkbenchSchemaState::Uninitialized);
        assert!(empty.can_upgrade());

        let upgrade = classify_schema_status(true, Some(1), 2, 1, 0, vec![], vec![], 7);
        assert_eq!(upgrade.state, WorkbenchSchemaState::UpgradeRequired);
        assert!(upgrade.can_upgrade());

        let ready = classify_schema_status(true, Some(2), 2, 2, 0, vec![], vec![], 7);
        assert_eq!(ready.state, WorkbenchSchemaState::Ready);
        assert!(ready.is_ready());

        let unmanaged = classify_schema_status(false, None, 2, 0, 0, vec![], vec![], 3);
        assert_eq!(unmanaged.state, WorkbenchSchemaState::Incompatible);
        assert!(!unmanaged.can_upgrade());

        let newer = classify_schema_status(true, Some(3), 2, 3, 0, vec![], vec![], 7);
        assert_eq!(newer.state, WorkbenchSchemaState::Incompatible);
    }
}
