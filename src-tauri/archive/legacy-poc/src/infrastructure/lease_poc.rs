use std::time::Duration;

use sqlx::{MySql, MySqlPool, Row, Transaction};

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeaseGrant {
    pub resource_key: String,
    pub owner_instance_id: String,
    pub fencing_token: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AcquireLeaseOutcome {
    Acquired(LeaseGrant),
    Busy {
        owner_instance_id: String,
        fencing_token: u64,
    },
}

#[derive(Clone)]
pub struct MySqlLeasePoc {
    pool: MySqlPool,
}

impl MySqlLeasePoc {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    pub async fn prepare(&self) -> AppResult<()> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _poc_resource_lease (\
                resource_key VARCHAR(191) NOT NULL PRIMARY KEY,\
                owner_instance_id VARCHAR(64) NOT NULL,\
                fencing_token BIGINT UNSIGNED NOT NULL,\
                expires_at DATETIME(6) NOT NULL,\
                updated_at DATETIME(6) NOT NULL\
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        )
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("创建PoC租约表", &error))?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS _poc_fenced_result (\
                resource_key VARCHAR(191) NOT NULL PRIMARY KEY,\
                fencing_token BIGINT UNSIGNED NOT NULL,\
                result_value VARCHAR(255) NOT NULL,\
                updated_at DATETIME(6) NOT NULL\
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
        )
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("创建PoC fencing结果表", &error))?;
        Ok(())
    }

    pub async fn acquire(
        &self,
        resource_key: &str,
        owner_instance_id: &str,
        ttl: Duration,
    ) -> AppResult<AcquireLeaseOutcome> {
        if resource_key.is_empty() || owner_instance_id.is_empty() || ttl.is_zero() {
            return Err(AppError::InvalidConfig("租约参数无效".into()));
        }
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始租约事务", &error))?;
        let current = sqlx::query(
            "SELECT owner_instance_id, fencing_token, expires_at <= UTC_TIMESTAMP(6) AS expired \
             FROM _poc_resource_lease WHERE resource_key = ? FOR UPDATE",
        )
        .bind(resource_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| AppError::database("读取当前租约", &error))?;
        let ttl_seconds = ttl.as_secs().max(1) as i64;

        let outcome = if let Some(row) = current {
            let current_owner: String = row
                .try_get("owner_instance_id")
                .map_err(|error| AppError::database("解析租约持有者", &error))?;
            let current_token: u64 = row
                .try_get("fencing_token")
                .map_err(|error| AppError::database("解析fencing token", &error))?;
            let expired: i8 = row
                .try_get("expired")
                .map_err(|error| AppError::database("解析租约过期状态", &error))?;
            if current_owner != owner_instance_id && expired == 0 {
                AcquireLeaseOutcome::Busy {
                    owner_instance_id: current_owner,
                    fencing_token: current_token,
                }
            } else {
                let next_token = current_token + 1;
                update_lease(
                    &mut transaction,
                    resource_key,
                    owner_instance_id,
                    next_token,
                    ttl_seconds,
                )
                .await?;
                AcquireLeaseOutcome::Acquired(LeaseGrant {
                    resource_key: resource_key.to_string(),
                    owner_instance_id: owner_instance_id.to_string(),
                    fencing_token: next_token,
                })
            }
        } else {
            sqlx::query(
                "INSERT INTO _poc_resource_lease \
                 (resource_key, owner_instance_id, fencing_token, expires_at, updated_at) \
                 VALUES (?, ?, 1, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), UTC_TIMESTAMP(6))",
            )
            .bind(resource_key)
            .bind(owner_instance_id)
            .bind(ttl_seconds)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("创建资源租约", &error))?;
            AcquireLeaseOutcome::Acquired(LeaseGrant {
                resource_key: resource_key.to_string(),
                owner_instance_id: owner_instance_id.to_string(),
                fencing_token: 1,
            })
        };
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交租约事务", &error))?;
        Ok(outcome)
    }

    pub async fn force_expire_for_test(&self, resource_key: &str) -> AppResult<()> {
        sqlx::query(
            "UPDATE _poc_resource_lease SET expires_at = DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 SECOND) \
             WHERE resource_key = ?",
        )
        .bind(resource_key)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("注入PoC租约过期", &error))?;
        Ok(())
    }

    pub async fn write_fenced_result(
        &self,
        grant: &LeaseGrant,
        result_value: &str,
    ) -> AppResult<bool> {
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| AppError::database("开始fencing写入事务", &error))?;
        let current = sqlx::query(
            "SELECT owner_instance_id, fencing_token, expires_at > UTC_TIMESTAMP(6) AS active \
             FROM _poc_resource_lease WHERE resource_key = ? FOR UPDATE",
        )
        .bind(&grant.resource_key)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| AppError::database("校验fencing token", &error))?;
        let accepted = current.is_some_and(|row| {
            row.try_get::<String, _>("owner_instance_id").ok().as_deref()
                == Some(grant.owner_instance_id.as_str())
                && row.try_get::<u64, _>("fencing_token").ok() == Some(grant.fencing_token)
                && row.try_get::<i8, _>("active").ok() == Some(1)
        });
        if accepted {
            sqlx::query(
                "INSERT INTO _poc_fenced_result \
                 (resource_key, fencing_token, result_value, updated_at) \
                 VALUES (?, ?, ?, UTC_TIMESTAMP(6)) \
                 ON DUPLICATE KEY UPDATE fencing_token = VALUES(fencing_token), \
                 result_value = VALUES(result_value), updated_at = VALUES(updated_at)",
            )
            .bind(&grant.resource_key)
            .bind(grant.fencing_token)
            .bind(result_value)
            .execute(&mut *transaction)
            .await
            .map_err(|error| AppError::database("写入fencing保护结果", &error))?;
        }
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交fencing写入事务", &error))?;
        Ok(accepted)
    }

    pub async fn cleanup(&self) -> AppResult<()> {
        sqlx::query("DROP TABLE IF EXISTS _poc_fenced_result")
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::database("清理PoC fencing结果表", &error))?;
        sqlx::query("DROP TABLE IF EXISTS _poc_resource_lease")
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::database("清理PoC租约表", &error))?;
        Ok(())
    }
}

async fn update_lease(
    transaction: &mut Transaction<'_, MySql>,
    resource_key: &str,
    owner_instance_id: &str,
    fencing_token: u64,
    ttl_seconds: i64,
) -> AppResult<()> {
    sqlx::query(
        "UPDATE _poc_resource_lease SET owner_instance_id = ?, fencing_token = ?, \
         expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL ? SECOND), updated_at = UTC_TIMESTAMP(6) \
         WHERE resource_key = ?",
    )
    .bind(owner_instance_id)
    .bind(fencing_token)
    .bind(ttl_seconds)
    .bind(resource_key)
    .execute(&mut **transaction)
    .await
    .map_err(|error| AppError::database("更新资源租约", &error))?;
    Ok(())
}

