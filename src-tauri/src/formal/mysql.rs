use std::collections::BTreeSet;
use std::time::Duration;

use serde::Serialize;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::{ConnectOptions, MySqlPool, Row};

use super::error::{FormalError, FormalResult};

const REQUIRED_PLATFORM_AIO_COLUMNS: &[&str] = &[
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MySqlTlsMode {
    Disabled,
    Required,
}

impl From<MySqlTlsMode> for MySqlSslMode {
    fn from(value: MySqlTlsMode) -> Self {
        match value {
            MySqlTlsMode::Disabled => Self::Disabled,
            MySqlTlsMode::Required => Self::Required,
        }
    }
}

#[derive(Clone)]
pub struct MySqlConnectionSpec {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub platform_schema: String,
    pub workbench_schema: String,
    pub tls_mode: MySqlTlsMode,
    pub connect_timeout: Duration,
}

impl std::fmt::Debug for MySqlConnectionSpec {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MySqlConnectionSpec")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("platform_schema", &self.platform_schema)
            .field("workbench_schema", &self.workbench_schema)
            .field("tls_mode", &self.tls_mode)
            .finish()
    }
}

impl MySqlConnectionSpec {
    pub fn validate(&self) -> FormalResult<()> {
        if self.host.trim().is_empty()
            || self.port == 0
            || self.username.trim().is_empty()
            || self.password.is_empty()
        {
            return Err(FormalError::InvalidConfig("数据库连接参数不完整".into()));
        }
        validate_identifier(&self.platform_schema)?;
        validate_identifier(&self.workbench_schema)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProjectMySqlPools {
    platform: MySqlPool,
    workbench: MySqlPool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaCapabilities {
    pub server_version: String,
    pub connection_encrypted: bool,
    pub tls_mode: MySqlTlsMode,
    pub platform_aio_columns: Vec<String>,
    pub missing_platform_aio_columns: Vec<String>,
    pub workbench_charset: Option<String>,
    pub workbench_collation: Option<String>,
}

impl ProjectMySqlPools {
    pub async fn connect(spec: &MySqlConnectionSpec) -> FormalResult<Self> {
        spec.validate()?;
        let platform = connect_pool(spec, &spec.platform_schema, 3, true).await?;
        let workbench = match connect_pool(spec, &spec.workbench_schema, 5, false).await {
            Ok(pool) => pool,
            Err(error) => {
                platform.close().await;
                return Err(error);
            }
        };
        Ok(Self {
            platform,
            workbench,
        })
    }

    pub fn platform(&self) -> &MySqlPool {
        &self.platform
    }

    pub fn workbench(&self) -> &MySqlPool {
        &self.workbench
    }

    pub async fn probe(&self, spec: &MySqlConnectionSpec) -> FormalResult<SchemaCapabilities> {
        let server_version = sqlx::query_scalar::<_, String>("SELECT VERSION()")
            .fetch_one(&self.platform)
            .await
            .map_err(|error| map_database_error("读取MySQL版本", error))?;
        let tls_cipher = sqlx::query("SHOW STATUS LIKE 'Ssl_cipher'")
            .fetch_optional(&self.platform)
            .await
            .map_err(|error| map_database_error("读取MySQL TLS状态", error))?
            .map(|row| {
                row.try_get::<String, _>("Value")
                    .map_err(|error| map_database_error("解析MySQL TLS状态", error))
            })
            .transpose()?
            .filter(|value| !value.is_empty());
        let rows = sqlx::query(
            "SELECT column_name FROM information_schema.columns \
             WHERE table_schema = ? AND table_name = 'op_edge_aio_server'",
        )
        .bind(&spec.platform_schema)
        .fetch_all(&self.platform)
        .await
        .map_err(|error| map_database_error("探测平台一体机表", error))?;
        let columns = rows
            .into_iter()
            .map(|row| {
                row.try_get::<String, _>("column_name")
                    .map_err(|error| map_database_error("解析平台一体机表字段", error))
            })
            .collect::<FormalResult<BTreeSet<_>>>()?;
        let missing_platform_aio_columns = REQUIRED_PLATFORM_AIO_COLUMNS
            .iter()
            .filter(|column| !columns.contains(**column))
            .map(|column| (*column).to_string())
            .collect::<Vec<_>>();
        let schema = sqlx::query(
            "SELECT default_character_set_name, default_collation_name \
             FROM information_schema.schemata WHERE schema_name = ?",
        )
        .bind(&spec.workbench_schema)
        .fetch_optional(&self.workbench)
        .await
        .map_err(|error| map_database_error("读取工作台库字符集", error))?;
        let (workbench_charset, workbench_collation) = match schema {
            Some(row) => (
                Some(
                    row.try_get("default_character_set_name")
                        .map_err(|error| map_database_error("解析工作台库字符集", error))?,
                ),
                Some(
                    row.try_get("default_collation_name")
                        .map_err(|error| map_database_error("解析工作台库排序规则", error))?,
                ),
            ),
            None => (None, None),
        };
        Ok(SchemaCapabilities {
            server_version,
            connection_encrypted: tls_cipher.is_some(),
            tls_mode: spec.tls_mode,
            platform_aio_columns: columns.into_iter().collect(),
            missing_platform_aio_columns,
            workbench_charset,
            workbench_collation,
        })
    }

    pub async fn close(&self) {
        self.platform.close().await;
        self.workbench.close().await;
    }
}

async fn connect_pool(
    spec: &MySqlConnectionSpec,
    schema: &str,
    max_connections: u32,
    read_only: bool,
) -> FormalResult<MySqlPool> {
    let options = MySqlConnectOptions::new()
        .host(&spec.host)
        .port(spec.port)
        .username(&spec.username)
        .password(&spec.password)
        .database(schema)
        .charset("utf8mb4")
        .ssl_mode(spec.tls_mode.into())
        .disable_statement_logging();
    let mut pool_options = MySqlPoolOptions::new()
        .min_connections(0)
        .max_connections(max_connections)
        .acquire_timeout(spec.connect_timeout);
    if read_only {
        pool_options = pool_options.after_connect(|connection, _metadata| {
            Box::pin(async move {
                sqlx::query("SET SESSION TRANSACTION READ ONLY")
                    .execute(&mut *connection)
                    .await?;
                Ok(())
            })
        });
    }
    pool_options
        .connect_with(options)
        .await
        .map_err(|error| map_database_error("建立MySQL连接池", error))
}

fn validate_identifier(value: &str) -> FormalResult<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(FormalError::InvalidConfig(
            "数据库名不符合标识符规则".into(),
        ));
    }
    Ok(())
}

fn map_database_error(operation: &'static str, error: sqlx::Error) -> FormalError {
    tracing::error!(operation, error = ?crate::core::log_safety::safe_error(&error), "mysql operation failed");
    FormalError::LocalDatabase(operation)
}
