use std::collections::BTreeSet;
use std::time::Duration;

use serde::Serialize;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::{MySqlPool, Row};

use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DatabaseTlsMode {
    Disabled,
    Required,
}

impl From<DatabaseTlsMode> for MySqlSslMode {
    fn from(value: DatabaseTlsMode) -> Self {
        match value {
            DatabaseTlsMode::Disabled => Self::Disabled,
            DatabaseTlsMode::Required => Self::Required,
        }
    }
}

#[derive(Clone, Debug)]
pub struct MySqlProjectConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: SecretValue,
    pub platform_schema: String,
    pub workbench_schema: String,
    pub connect_timeout: Duration,
    pub tls_mode: DatabaseTlsMode,
}

impl MySqlProjectConfig {
    pub fn validate(&self) -> AppResult<()> {
        if self.host.trim().is_empty() {
            return Err(AppError::InvalidConfig("数据库主机不能为空".into()));
        }
        if self.port == 0 {
            return Err(AppError::InvalidConfig("数据库端口无效".into()));
        }
        if self.username.trim().is_empty() || self.password.is_empty() {
            return Err(AppError::InvalidConfig("数据库账号或密码不能为空".into()));
        }
        validate_identifier(&self.platform_schema)?;
        validate_identifier(&self.workbench_schema)?;
        Ok(())
    }
}

#[derive(Clone)]
pub struct DualMySqlPools {
    pub platform: MySqlPool,
    pub workbench: MySqlPool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseProbeReport {
    pub server_version: String,
    pub tls_mode: DatabaseTlsMode,
    pub tls_cipher: Option<String>,
    pub platform_schema: String,
    pub workbench_schema: String,
    pub workbench_charset: Option<String>,
    pub workbench_collation: Option<String>,
}

impl DualMySqlPools {
    pub async fn connect(config: &MySqlProjectConfig) -> AppResult<Self> {
        config.validate()?;
        let platform = connect_pool(config, &config.platform_schema, 3, true).await?;
        let workbench = match connect_pool(config, &config.workbench_schema, 5, false).await {
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

    /// 平台读操作可以独立使用；共享库只在第一次共享操作时建立连接。
    pub async fn connect_platform_first(config: &MySqlProjectConfig) -> AppResult<Self> {
        config.validate()?;
        let platform = connect_pool(config, &config.platform_schema, 3, true).await?;
        let options = MySqlConnectOptions::new()
            .host(&config.host)
            .port(config.port)
            .username(&config.username)
            .password(config.password.expose())
            .database(&config.workbench_schema)
            .charset("utf8mb4")
            .ssl_mode(config.tls_mode.into());
        let workbench = MySqlPoolOptions::new()
            .min_connections(0)
            .max_connections(5)
            .acquire_timeout(config.connect_timeout)
            .connect_lazy_with(options);
        Ok(Self {
            platform,
            workbench,
        })
    }

    pub async fn probe(&self, config: &MySqlProjectConfig) -> AppResult<DatabaseProbeReport> {
        let server_version = sqlx::query_scalar::<_, String>("SELECT VERSION()")
            .fetch_one(&self.platform)
            .await
            .map_err(|error| AppError::database("读取MySQL版本", &error))?;
        let tls_cipher = sqlx::query("SHOW STATUS LIKE 'Ssl_cipher'")
            .fetch_optional(&self.platform)
            .await
            .map_err(|error| AppError::database("读取MySQL TLS状态", &error))?
            .map(|row| {
                row.try_get::<String, _>("Value")
                    .map_err(|error| AppError::database("解析MySQL TLS状态", &error))
            })
            .transpose()?
            .filter(|value| !value.is_empty());

        let schema_row = sqlx::query(
            "SELECT default_character_set_name, default_collation_name \
             FROM information_schema.schemata WHERE schema_name = ?",
        )
        .bind(&config.workbench_schema)
        .fetch_optional(&self.workbench)
        .await
        .map_err(|error| AppError::database("读取工作台库字符集", &error))?;

        let (workbench_charset, workbench_collation) = match schema_row {
            Some(row) => (
                Some(
                    row.try_get("default_character_set_name")
                        .map_err(|error| AppError::database("解析工作台库字符集", &error))?,
                ),
                Some(
                    row.try_get("default_collation_name")
                        .map_err(|error| AppError::database("解析工作台库排序规则", &error))?,
                ),
            ),
            None => (None, None),
        };

        Ok(DatabaseProbeReport {
            server_version,
            tls_mode: config.tls_mode,
            tls_cipher,
            platform_schema: config.platform_schema.clone(),
            workbench_schema: config.workbench_schema.clone(),
            workbench_charset,
            workbench_collation,
        })
    }

    pub async fn close(&self) {
        self.platform.close().await;
        self.workbench.close().await;
    }
}

pub async fn connect_platform_write(config: &MySqlProjectConfig) -> AppResult<MySqlPool> {
    config.validate()?;
    connect_pool(config, &config.platform_schema, 3, false).await
}

async fn connect_pool(
    config: &MySqlProjectConfig,
    schema: &str,
    max_connections: u32,
    read_only: bool,
) -> AppResult<MySqlPool> {
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(config.password.expose())
        .database(schema)
        .charset("utf8mb4")
        .ssl_mode(config.tls_mode.into());

    let mut pool_options = MySqlPoolOptions::new()
        .min_connections(0)
        .max_connections(max_connections)
        .acquire_timeout(config.connect_timeout);
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
        .map_err(|error| AppError::database("建立MySQL连接池", &error))
}

pub async fn require_table_columns(
    pool: &MySqlPool,
    table: &str,
    required: &[&str],
) -> AppResult<()> {
    let columns = sqlx::query_scalar::<_, String>(
        "SELECT column_name FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = ?"
    ).bind(table).fetch_all(pool).await
        .map_err(|error| AppError::database("检查业务表字段", &error))?
        .into_iter().collect::<BTreeSet<_>>();
    let missing = required
        .iter()
        .filter(|name| !columns.contains(**name))
        .copied()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(AppError::InvalidConfig(format!(
            "业务表{table}缺少字段：{}",
            missing.join("、")
        )));
    }
    Ok(())
}

fn validate_identifier(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(AppError::InvalidConfig(format!(
            "数据库名不符合标识符规则：{value}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_identifier;

    #[test]
    fn database_identifier_rejects_sql_fragments() {
        assert!(validate_identifier("inxvision_iot_dev").is_ok());
        assert!(validate_identifier("inxaiot_desk_buddy").is_ok());
        assert!(validate_identifier("db-name").is_err());
        assert!(validate_identifier("db;drop table x").is_err());
    }
}
