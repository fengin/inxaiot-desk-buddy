use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::application::ports::remote_session::{HostKeyIdentity, HostKeyPolicy, RemoteTarget};
use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKeyRecord {
    pub local_project_id: String,
    pub host: String,
    pub port: u16,
    pub identity: HostKeyIdentity,
    pub accepted_at: String,
}

#[derive(Clone)]
pub struct HostKeyRepository {
    pool: SqlitePool,
}

impl HostKeyRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn policy(
        &self,
        local_project_id: &str,
        target: &RemoteTarget,
    ) -> AppResult<HostKeyPolicy> {
        Ok(match self.get(local_project_id, target).await? {
            Some(record) => HostKeyPolicy::Require(record.identity),
            None => HostKeyPolicy::Capture,
        })
    }

    pub async fn get(
        &self,
        local_project_id: &str,
        target: &RemoteTarget,
    ) -> AppResult<Option<HostKeyRecord>> {
        validate_key(local_project_id, target)?;
        let row = sqlx::query(
            "SELECT local_project_id, host, port, algorithm, fingerprint, accepted_at \
             FROM local_host_key WHERE local_project_id = ? AND host = ? AND port = ?",
        )
        .bind(local_project_id)
        .bind(target.host.trim())
        .bind(i64::from(target.port))
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取SSH主机密钥", &error))?;
        row.map(map_record).transpose()
    }

    pub async fn list(&self, local_project_id: &str) -> AppResult<Vec<HostKeyRecord>> {
        if local_project_id.trim().is_empty() {
            return Err(AppError::InvalidConfig("本地项目ID不能为空".into()));
        }
        let rows = sqlx::query(
            "SELECT local_project_id, host, port, algorithm, fingerprint, accepted_at \
             FROM local_host_key WHERE local_project_id = ? ORDER BY host, port",
        )
        .bind(local_project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| AppError::database("列出SSH主机密钥", &error))?;
        rows.into_iter().map(map_record).collect()
    }

    pub async fn confirm(
        &self,
        local_project_id: &str,
        target: &RemoteTarget,
        identity: &HostKeyIdentity,
        replace_changed: bool,
    ) -> AppResult<HostKeyRecord> {
        validate_key(local_project_id, target)?;
        if identity.algorithm.trim().is_empty() || identity.fingerprint.trim().is_empty() {
            return Err(AppError::InvalidConfig("SSH主机密钥信息不完整".into()));
        }
        if let Some(existing) = self.get(local_project_id, target).await?
            && existing.identity != *identity
            && !replace_changed
        {
            return Err(AppError::HostKeyChanged {
                expected: existing.identity.fingerprint,
                actual: identity.fingerprint.clone(),
            });
        }
        let accepted_at = OffsetDateTime::now_utc().unix_timestamp_nanos().to_string();
        sqlx::query(
            "INSERT INTO local_host_key \
             (local_project_id, host, port, algorithm, fingerprint, accepted_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(local_project_id, host, port) DO UPDATE SET \
             algorithm = excluded.algorithm, fingerprint = excluded.fingerprint, \
             accepted_at = excluded.accepted_at",
        )
        .bind(local_project_id)
        .bind(target.host.trim())
        .bind(i64::from(target.port))
        .bind(&identity.algorithm)
        .bind(&identity.fingerprint)
        .bind(&accepted_at)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("保存SSH主机密钥", &error))?;
        Ok(HostKeyRecord {
            local_project_id: local_project_id.into(),
            host: target.host.trim().into(),
            port: target.port,
            identity: identity.clone(),
            accepted_at,
        })
    }

    pub async fn delete(&self, local_project_id: &str, target: &RemoteTarget) -> AppResult<bool> {
        validate_key(local_project_id, target)?;
        let result = sqlx::query(
            "DELETE FROM local_host_key WHERE local_project_id = ? AND host = ? AND port = ?",
        )
        .bind(local_project_id)
        .bind(target.host.trim())
        .bind(i64::from(target.port))
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("删除SSH主机密钥", &error))?;
        Ok(result.rows_affected() == 1)
    }
}

fn validate_key(local_project_id: &str, target: &RemoteTarget) -> AppResult<()> {
    if local_project_id.trim().is_empty() {
        return Err(AppError::InvalidConfig("本地项目ID不能为空".into()));
    }
    target.validate()
}

fn map_record(row: sqlx::sqlite::SqliteRow) -> AppResult<HostKeyRecord> {
    let port: i64 = row
        .try_get("port")
        .map_err(|error| AppError::database("解析SSH主机密钥端口", &error))?;
    Ok(HostKeyRecord {
        local_project_id: row
            .try_get("local_project_id")
            .map_err(|error| AppError::database("解析SSH主机密钥项目", &error))?,
        host: row
            .try_get("host")
            .map_err(|error| AppError::database("解析SSH主机", &error))?,
        port: u16::try_from(port)
            .map_err(|_| AppError::InvalidConfig("SSH主机密钥端口超出范围".into()))?,
        identity: HostKeyIdentity {
            algorithm: row
                .try_get("algorithm")
                .map_err(|error| AppError::database("解析SSH主机密钥算法", &error))?,
            fingerprint: row
                .try_get("fingerprint")
                .map_err(|error| AppError::database("解析SSH主机指纹", &error))?,
        },
        accepted_at: row
            .try_get("accepted_at")
            .map_err(|error| AppError::database("解析SSH主机密钥确认时间", &error))?,
    })
}
