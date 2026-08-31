use std::path::PathBuf;

use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentArtifact {
    pub local_project_id: String,
    pub domain_type: String,
    pub artifact_type: String,
    pub service: String,
    pub path: PathBuf,
    pub used_at: String,
}

#[derive(Clone)]
pub struct RecentArtifactRepository {
    pool: SqlitePool,
}

impl RecentArtifactRepository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn save(
        &self,
        local_project_id: &str,
        domain_type: &str,
        artifact_type: &str,
        service: &str,
        path: PathBuf,
    ) -> AppResult<RecentArtifact> {
        validate(local_project_id, domain_type, artifact_type, &path)?;
        let used_at = OffsetDateTime::now_utc().unix_timestamp_nanos().to_string();
        let path_text = path.to_string_lossy().into_owned();
        sqlx::query(
            "INSERT INTO local_recent_artifact \
             (local_project_id, domain_type, artifact_type, service, path, used_at) \
             VALUES (?, ?, ?, ?, ?, ?) \
             ON CONFLICT(local_project_id, domain_type, artifact_type, service) DO UPDATE SET \
             path = excluded.path, used_at = excluded.used_at",
        )
        .bind(local_project_id)
        .bind(domain_type)
        .bind(artifact_type)
        .bind(service)
        .bind(&path_text)
        .bind(&used_at)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::database("保存最近使用文件", &error))?;
        Ok(RecentArtifact {
            local_project_id: local_project_id.into(),
            domain_type: domain_type.into(),
            artifact_type: artifact_type.into(),
            service: service.into(),
            path,
            used_at,
        })
    }

    pub async fn get(
        &self,
        local_project_id: &str,
        domain_type: &str,
        artifact_type: &str,
        service: &str,
    ) -> AppResult<Option<RecentArtifact>> {
        let row = sqlx::query(
            "SELECT local_project_id, domain_type, artifact_type, service, path, used_at \
             FROM local_recent_artifact WHERE local_project_id = ? AND domain_type = ? \
             AND artifact_type = ? AND service = ?",
        )
        .bind(local_project_id)
        .bind(domain_type)
        .bind(artifact_type)
        .bind(service)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::database("读取最近使用文件", &error))?;
        row.map(map_record).transpose()
    }
}

fn validate(
    local_project_id: &str,
    domain_type: &str,
    artifact_type: &str,
    path: &std::path::Path,
) -> AppResult<()> {
    if local_project_id.trim().is_empty()
        || domain_type.trim().is_empty()
        || artifact_type.trim().is_empty()
        || path.as_os_str().is_empty()
        || !path.is_absolute()
    {
        return Err(AppError::InvalidConfig("最近使用文件参数无效".into()));
    }
    Ok(())
}

fn map_record(row: sqlx::sqlite::SqliteRow) -> AppResult<RecentArtifact> {
    let path: String = row
        .try_get("path")
        .map_err(|error| AppError::database("解析最近使用文件路径", &error))?;
    Ok(RecentArtifact {
        local_project_id: row
            .try_get("local_project_id")
            .map_err(|error| AppError::database("解析最近文件项目", &error))?,
        domain_type: row
            .try_get("domain_type")
            .map_err(|error| AppError::database("解析最近文件业务域", &error))?,
        artifact_type: row
            .try_get("artifact_type")
            .map_err(|error| AppError::database("解析最近文件类型", &error))?,
        service: row
            .try_get("service")
            .map_err(|error| AppError::database("解析最近文件服务", &error))?,
        path: PathBuf::from(path),
        used_at: row
            .try_get("used_at")
            .map_err(|error| AppError::database("解析最近文件时间", &error))?,
    })
}
