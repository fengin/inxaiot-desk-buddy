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

#[derive(Clone, Debug)]
pub struct HostKeyObservation {
    pub record: HostKeyRecord,
    pub previous: Option<HostKeyIdentity>,
}

impl HostKeyObservation {
    pub fn changed(&self) -> bool {
        self.previous
            .as_ref()
            .is_some_and(|previous| previous != &self.record.identity)
    }

    pub fn message(&self) -> String {
        match &self.previous {
            Some(previous) if self.changed() => format!(
                "{}:{}主机指纹发生变化：{} {} → {} {}；已自动记录新指纹并继续连接，无需人工确认",
                self.record.host,
                self.record.port,
                previous.algorithm,
                previous.fingerprint,
                self.record.identity.algorithm,
                self.record.identity.fingerprint
            ),
            Some(_) => format!(
                "{}:{}主机指纹与上次记录一致",
                self.record.host, self.record.port
            ),
            None => format!(
                "{}:{}主机指纹已自动采集并记录",
                self.record.host, self.record.port
            ),
        }
    }
}

#[derive(Clone)]
pub struct HostKeyRepository {
    pool: SqlitePool,
}

impl HostKeyRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// 指纹仅用于运维观测：首次自动记录，变化记录旧/新值但不拒绝连接。
    pub async fn observe(
        &self,
        local_project_id: &str,
        target: &RemoteTarget,
        identity: &HostKeyIdentity,
    ) -> AppResult<HostKeyObservation> {
        validate_key(local_project_id, target)?;
        if identity.algorithm.trim().is_empty() || identity.fingerprint.trim().is_empty() {
            return Err(AppError::InvalidConfig("SSH主机指纹采集结果为空".into()));
        }
        let mut transaction = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(|error| AppError::database("开始SSH指纹观测记录", &error))?;
        let row = sqlx::query(
            "SELECT local_project_id, host, port, algorithm, fingerprint, accepted_at \
             FROM local_host_key WHERE local_project_id = ? AND host = ? AND port = ?",
        )
        .bind(local_project_id)
        .bind(target.host.trim())
        .bind(i64::from(target.port))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(|error| AppError::database("读取上次SSH指纹", &error))?;
        let previous = row
            .map(map_record)
            .transpose()?
            .map(|record| record.identity);
        // 保留旧列名兼容现有本机数据；自动观测路径中表示最近记录时间，不代表用户确认。
        let accepted_at = OffsetDateTime::now_utc().unix_timestamp_nanos().to_string();
        sqlx::query(
            "INSERT INTO local_host_key (local_project_id,host,port,algorithm,fingerprint,accepted_at) \
             VALUES (?,?,?,?,?,?) ON CONFLICT(local_project_id,host,port) DO UPDATE SET \
             algorithm=excluded.algorithm,fingerprint=excluded.fingerprint,accepted_at=excluded.accepted_at",
        )
        .bind(local_project_id).bind(target.host.trim()).bind(i64::from(target.port))
        .bind(&identity.algorithm).bind(&identity.fingerprint).bind(&accepted_at)
        .execute(&mut *transaction).await
        .map_err(|error| AppError::database("保存SSH指纹观测记录", &error))?;
        transaction
            .commit()
            .await
            .map_err(|error| AppError::database("提交SSH指纹观测记录", &error))?;
        let observation = HostKeyObservation {
            record: HostKeyRecord {
                local_project_id: local_project_id.into(),
                host: target.host.trim().into(),
                port: target.port,
                identity: identity.clone(),
                accepted_at,
            },
            previous,
        };
        if let Some(previous) = &observation.previous
            && observation.changed()
        {
            tracing::warn!(
                project_id = local_project_id,
                host = target.host,
                port = target.port,
                previous_algorithm = previous.algorithm,
                previous_fingerprint = previous.fingerprint,
                observed_algorithm = identity.algorithm,
                observed_fingerprint = identity.fingerprint,
                "SSH主机指纹变化，已自动记录并继续连接"
            );
        }
        Ok(observation)
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
