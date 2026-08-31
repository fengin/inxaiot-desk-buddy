use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::error::{FormalError, FormalResult};
use super::secret_store::SecretStore;

const ACTIVE_PROJECT_TASK_CONSTRAINT: &str = "ACTIVE_PROJECT_TASK";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalProject {
    pub name: String,
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: String,
    pub business_db: String,
    pub workbench_db: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateLocalProject {
    pub name: String,
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub db_password: Option<String>,
    pub business_db: String,
    pub workbench_db: String,
}

impl CreateLocalProject {
    fn validate(&self) -> FormalResult<()> {
        if self.name.trim().is_empty()
            || self.platform_url.trim().is_empty()
            || self.db_host.trim().is_empty()
            || self.db_port == 0
            || self.db_user.trim().is_empty()
            || self.db_password.is_empty()
            || self.business_db.trim().is_empty()
            || self.workbench_db.trim().is_empty()
        {
            return Err(FormalError::InvalidConfig("项目连接信息不完整".into()));
        }
        Ok(())
    }
}

impl UpdateLocalProject {
    fn validate(&self) -> FormalResult<()> {
        if self.name.trim().is_empty()
            || self.platform_url.trim().is_empty()
            || self.db_host.trim().is_empty()
            || self.db_port == 0
            || self.db_user.trim().is_empty()
            || self.business_db.trim().is_empty()
            || self.workbench_db.trim().is_empty()
        {
            return Err(FormalError::InvalidConfig("项目连接信息不完整".into()));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProjectRecord {
    pub id: String,
    pub name: String,
    pub platform_url: String,
    pub db_host: String,
    pub db_port: u16,
    pub db_user: String,
    pub business_db: String,
    pub workbench_db: String,
    pub last_opened_at: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectConnectionSecrets {
    pub project: LocalProjectRecord,
    pub db_password: String,
}

impl std::fmt::Display for ProjectConnectionSecrets {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "ProjectConnectionSecrets(project_id={}, db_password=[REDACTED])",
            self.project.id
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalProjectSession {
    pub local_project_id: String,
    pub username: String,
    pub expires_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug)]
pub struct ProjectSessionSecrets {
    pub session: LocalProjectSession,
    pub access_token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecretCleanupReport {
    pub attempted: usize,
    pub deleted: usize,
    pub pending: usize,
}

#[derive(Clone)]
pub struct LocalProjectRepository {
    pool: SqlitePool,
    secrets: Arc<dyn SecretStore>,
}

impl LocalProjectRepository {
    pub fn new(pool: SqlitePool, secrets: Arc<dyn SecretStore>) -> Self {
        Self { pool, secrets }
    }

    pub async fn retry_pending_secret_cleanup(&self) -> FormalResult<SecretCleanupReport> {
        let rows = sqlx::query(
            "SELECT secret_ref, reason FROM local_secret_cleanup ORDER BY updated_at, secret_ref",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "list pending secret cleanup failed");
            FormalError::LocalDatabase("读取待清理本机凭据")
        })?;
        let attempted = rows.len();
        let mut deleted = 0;
        for row in rows {
            let secret_ref: String = row
                .try_get("secret_ref")
                .map_err(|_| FormalError::LocalDatabase("解析待清理凭据引用"))?;
            match self.secrets.delete(&secret_ref) {
                Ok(()) => {
                    sqlx::query("DELETE FROM local_project_master_key WHERE secret_ref = ?")
                        .bind(&secret_ref)
                        .execute(&self.pool)
                        .await
                        .map_err(|error| {
                            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "remove cleaned master key registry failed");
                            FormalError::LocalDatabase("清理项目主密钥注册表")
                        })?;
                    sqlx::query("DELETE FROM local_secret_cleanup WHERE secret_ref = ?")
                        .bind(&secret_ref)
                        .execute(&self.pool)
                        .await
                        .map_err(|error| {
                            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "remove completed secret cleanup failed");
                            FormalError::LocalDatabase("完成本机凭据清理")
                        })?;
                    deleted += 1;
                }
                Err(error) => {
                    sqlx::query(
                        "UPDATE local_secret_cleanup SET retry_count = retry_count + 1, \
                         last_error_code = ?, updated_at = ? WHERE secret_ref = ?",
                    )
                    .bind(error.to_dto().code)
                    .bind(timestamp())
                    .bind(&secret_ref)
                    .execute(&self.pool)
                    .await
                    .map_err(|database_error| {
                        tracing::error!(error = ?crate::core::log_safety::safe_error(&database_error), "update pending secret cleanup failed");
                        FormalError::LocalDatabase("更新本机凭据清理重试")
                    })?;
                }
            }
        }
        Ok(SecretCleanupReport {
            attempted,
            deleted,
            pending: attempted.saturating_sub(deleted),
        })
    }

    pub async fn pending_secret_cleanup_count(&self) -> FormalResult<usize> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_secret_cleanup")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "count pending secret cleanup failed");
                FormalError::LocalDatabase("统计待清理本机凭据")
            })?;
        usize::try_from(count).map_err(|_| FormalError::LocalDatabase("解析待清理本机凭据数量"))
    }

    pub(crate) async fn delete_secret_or_enqueue(
        &self,
        secret_ref: &str,
        reason: &str,
    ) -> FormalResult<()> {
        if self.secrets.delete(secret_ref).is_ok() {
            sqlx::query("DELETE FROM local_project_master_key WHERE secret_ref = ?")
                .bind(secret_ref)
                .execute(&self.pool)
                .await
                .map_err(|error| {
                    tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "clear master key registry row failed");
                    FormalError::LocalDatabase("清理项目主密钥注册表")
                })?;
            sqlx::query("DELETE FROM local_secret_cleanup WHERE secret_ref = ?")
                .bind(secret_ref)
                .execute(&self.pool)
                .await
                .map_err(|error| {
                    tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "clear stale secret cleanup row failed");
                    FormalError::LocalDatabase("清理本机凭据Outbox")
                })?;
            return Ok(());
        }
        let now = timestamp();
        sqlx::query(
            "INSERT INTO local_secret_cleanup \
             (secret_ref, reason, retry_count, last_error_code, created_at, updated_at) \
             VALUES (?, ?, 0, 'SECRET_STORE', ?, ?) \
             ON CONFLICT(secret_ref) DO UPDATE SET reason = excluded.reason, \
             last_error_code = excluded.last_error_code, updated_at = excluded.updated_at",
        )
        .bind(secret_ref)
        .bind(reason)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "enqueue secret cleanup failed");
            FormalError::LocalDatabase("登记待清理本机凭据")
        })?;
        Ok(())
    }

    async fn current_database_secret_ref(&self, project_id: &str) -> FormalResult<Option<String>> {
        sqlx::query_scalar("SELECT db_password_secret_ref FROM local_project WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "read current database secret ref failed");
                FormalError::LocalDatabase("核对数据库凭据引用")
            })
    }

    async fn current_session_secret_ref(&self, project_id: &str) -> FormalResult<Option<String>> {
        sqlx::query_scalar(
            "SELECT token_secret_ref FROM local_project_session WHERE local_project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "read current session secret ref failed");
            FormalError::LocalDatabase("核对平台令牌引用")
        })
    }

    pub async fn create(&self, input: CreateLocalProject) -> FormalResult<LocalProjectRecord> {
        input.validate()?;
        let id = Uuid::now_v7().to_string();
        let secret_ref = format!("project/{id}/database-password/{}", Uuid::now_v7());
        self.secrets
            .save(&secret_ref, input.db_password.as_bytes())?;
        let now = timestamp();
        let result = sqlx::query(
            "INSERT INTO local_project \
             (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
              db_password_secret_ref, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&id)
        .bind(input.name.trim())
        .bind(input.platform_url.trim())
        .bind(input.db_host.trim())
        .bind(i64::from(input.db_port))
        .bind(input.db_user.trim())
        .bind(input.business_db.trim())
        .bind(input.workbench_db.trim())
        .bind(&secret_ref)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            if self.current_database_secret_ref(&id).await?.as_deref() == Some(&secret_ref) {
                return self.get(&id).await;
            }
            let _ = self
                .delete_secret_or_enqueue(&secret_ref, "project_create_rollback")
                .await;
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "insert local project failed");
            return Err(FormalError::LocalDatabase("创建本地项目入口"));
        }
        self.get(&id).await
    }

    pub async fn list(&self) -> FormalResult<Vec<LocalProjectRecord>> {
        let rows = sqlx::query(
            "SELECT id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, last_opened_at \
             FROM local_project ORDER BY COALESCE(last_opened_at, updated_at) DESC, name ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "list local projects failed");
            FormalError::LocalDatabase("读取本地项目入口")
        })?;
        rows.into_iter().map(map_project).collect()
    }

    pub async fn update(
        &self,
        project_id: &str,
        input: UpdateLocalProject,
    ) -> FormalResult<LocalProjectRecord> {
        input.validate()?;
        let row = sqlx::query(
            "SELECT project.platform_url, project.db_password_secret_ref, session.token_secret_ref \
             FROM local_project project LEFT JOIN local_project_session session \
             ON session.local_project_id = project.id WHERE project.id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "read local project before update failed");
            FormalError::LocalDatabase("读取待编辑项目")
        })?
        .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        let old_platform_url: String = row
            .try_get("platform_url")
            .map_err(|_| FormalError::LocalDatabase("解析原平台地址"))?;
        let old_secret_ref: String = row
            .try_get("db_password_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析数据库凭据引用"))?;
        let old_token_ref: Option<String> = row
            .try_get("token_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析原平台令牌引用"))?;
        let new_secret_ref = input
            .db_password
            .as_ref()
            .filter(|password| !password.is_empty())
            .map(|_| format!("project/{project_id}/database-password/{}", Uuid::now_v7()));
        if let (Some(password), Some(secret_ref)) = (&input.db_password, &new_secret_ref) {
            self.secrets.save(secret_ref, password.as_bytes())?;
        }
        let secret_ref = new_secret_ref.as_deref().unwrap_or(&old_secret_ref);
        let mut transaction = match self.pool.begin().await {
            Ok(transaction) => transaction,
            Err(error) => {
                if let Some(secret_ref) = &new_secret_ref {
                    let _ = self
                        .delete_secret_or_enqueue(secret_ref, "project_update_begin_rollback")
                        .await;
                }
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "begin local project update failed");
                return Err(FormalError::LocalDatabase("开始编辑项目事务"));
            }
        };
        let result = sqlx::query(
            "UPDATE local_project SET name = ?, platform_url = ?, db_host = ?, db_port = ?, \
             db_user = ?, business_db = ?, workbench_db = ?, db_password_secret_ref = ?, \
             updated_at = ? WHERE id = ?",
        )
        .bind(input.name.trim())
        .bind(input.platform_url.trim())
        .bind(input.db_host.trim())
        .bind(i64::from(input.db_port))
        .bind(input.db_user.trim())
        .bind(input.business_db.trim())
        .bind(input.workbench_db.trim())
        .bind(secret_ref)
        .bind(timestamp())
        .bind(project_id)
        .execute(&mut *transaction)
        .await;
        let result = match result {
            Ok(result) => result,
            Err(error) => {
                drop(transaction);
                if let Some(secret_ref) = &new_secret_ref {
                    let _ = self
                        .delete_secret_or_enqueue(secret_ref, "project_update_statement_rollback")
                        .await;
                }
                return Err(map_project_mutation_error(
                    error,
                    "编辑本地项目入口",
                    "update local project failed",
                ));
            }
        };
        if result.rows_affected() != 1 {
            drop(transaction);
            if let Some(secret_ref) = &new_secret_ref {
                let _ = self
                    .delete_secret_or_enqueue(secret_ref, "project_update_missing_rollback")
                    .await;
            }
            return Err(FormalError::NotFound(format!("项目不存在：{project_id}")));
        }
        if old_platform_url.trim() != input.platform_url.trim() {
            let session_result =
                sqlx::query("DELETE FROM local_project_session WHERE local_project_id = ?")
                    .bind(project_id)
                    .execute(&mut *transaction)
                    .await;
            if let Err(error) = session_result {
                drop(transaction);
                if let Some(secret_ref) = &new_secret_ref {
                    let _ = self
                        .delete_secret_or_enqueue(secret_ref, "project_update_session_rollback")
                        .await;
                }
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "invalidate project session after update failed");
                return Err(FormalError::LocalDatabase("使项目会话失效"));
            }
        }
        if let Err(error) = transaction.commit().await {
            let current = self.get(project_id).await.ok();
            let current_ref = self
                .current_database_secret_ref(project_id)
                .await
                .ok()
                .flatten();
            if current
                .as_ref()
                .is_some_and(|record| project_matches_update(record, &input))
                && current_ref.as_deref() == Some(secret_ref)
            {
                if new_secret_ref.is_some() {
                    self.delete_secret_or_enqueue(&old_secret_ref, "project_update_old_password")
                        .await?;
                }
                if old_platform_url.trim() != input.platform_url.trim()
                    && let Some(token_ref) = &old_token_ref
                {
                    self.delete_secret_or_enqueue(token_ref, "project_update_old_session")
                        .await?;
                }
                return self.get(project_id).await;
            }
            if let Some(secret_ref) = &new_secret_ref
                && current_ref.as_deref() != Some(secret_ref)
            {
                let _ = self
                    .delete_secret_or_enqueue(secret_ref, "project_update_commit_rollback")
                    .await;
            }
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "commit local project update failed");
            return Err(FormalError::LocalDatabase("提交编辑项目事务"));
        }
        if new_secret_ref.is_some() {
            self.delete_secret_or_enqueue(&old_secret_ref, "project_update_old_password")
                .await?;
        }
        if old_platform_url.trim() != input.platform_url.trim()
            && let Some(token_ref) = &old_token_ref
        {
            self.delete_secret_or_enqueue(token_ref, "project_update_old_session")
                .await?;
        }
        self.get(project_id).await
    }

    pub async fn get(&self, project_id: &str) -> FormalResult<LocalProjectRecord> {
        let row = sqlx::query(
            "SELECT id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, last_opened_at \
             FROM local_project WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "get local project failed");
            FormalError::LocalDatabase("读取本地项目入口")
        })?
        .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        map_project(row)
    }

    pub async fn connection_secrets(
        &self,
        project_id: &str,
    ) -> FormalResult<ProjectConnectionSecrets> {
        let row = sqlx::query(
            "SELECT id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
             last_opened_at, db_password_secret_ref FROM local_project WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "load project connection failed");
            FormalError::LocalDatabase("读取项目连接")
        })?
        .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        let secret_ref: String = row
            .try_get("db_password_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析数据库凭据引用"))?;
        let project = map_project(row)?;
        let password = self.secrets.load(&secret_ref)?;
        let db_password = String::from_utf8(password)
            .map_err(|_| FormalError::SecretStore("数据库密码不是UTF-8文本"))?;
        Ok(ProjectConnectionSecrets {
            project,
            db_password,
        })
    }

    pub async fn touch_opened(&self, project_id: &str) -> FormalResult<()> {
        let result =
            sqlx::query("UPDATE local_project SET last_opened_at = ?, updated_at = ? WHERE id = ?")
                .bind(timestamp())
                .bind(timestamp())
                .bind(project_id)
                .execute(&self.pool)
                .await
                .map_err(|error| {
                    tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "touch local project failed");
                    FormalError::LocalDatabase("更新项目打开时间")
                })?;
        if result.rows_affected() == 0 {
            return Err(FormalError::NotFound(format!("项目不存在：{project_id}")));
        }
        Ok(())
    }

    pub async fn save_session(
        &self,
        project_id: &str,
        username: &str,
        access_token: &str,
        expires_at: Option<String>,
    ) -> FormalResult<LocalProjectSession> {
        self.get(project_id).await?;
        if username.trim().is_empty() || access_token.is_empty() {
            return Err(FormalError::InvalidConfig("项目会话参数不完整".into()));
        }
        let old_token_ref = self.current_session_secret_ref(project_id).await?;
        let token_ref = format!(
            "project/{project_id}/platform-access-token/{}",
            Uuid::now_v7()
        );
        self.secrets.save(&token_ref, access_token.as_bytes())?;
        let now = timestamp();
        let result = sqlx::query(
            "INSERT INTO local_project_session \
             (local_project_id, username, token_secret_ref, expires_at, updated_at) \
             VALUES (?, ?, ?, ?, ?) \
             ON CONFLICT(local_project_id) DO UPDATE SET username = excluded.username, \
             token_secret_ref = excluded.token_secret_ref, expires_at = excluded.expires_at, \
             updated_at = excluded.updated_at",
        )
        .bind(project_id)
        .bind(username.trim())
        .bind(&token_ref)
        .bind(&expires_at)
        .bind(&now)
        .execute(&self.pool)
        .await;
        if let Err(error) = result {
            if self
                .current_session_secret_ref(project_id)
                .await?
                .as_deref()
                == Some(&token_ref)
            {
                if let Some(old_ref) = &old_token_ref {
                    self.delete_secret_or_enqueue(old_ref, "session_replace_old_token")
                        .await?;
                }
                return Ok(LocalProjectSession {
                    local_project_id: project_id.to_string(),
                    username: username.trim().to_string(),
                    expires_at,
                    updated_at: now,
                });
            }
            let _ = self
                .delete_secret_or_enqueue(&token_ref, "session_save_rollback")
                .await;
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "save project session failed");
            return Err(FormalError::LocalDatabase("保存项目会话"));
        }
        if let Some(old_ref) = &old_token_ref {
            self.delete_secret_or_enqueue(old_ref, "session_replace_old_token")
                .await?;
        }
        Ok(LocalProjectSession {
            local_project_id: project_id.to_string(),
            username: username.trim().to_string(),
            expires_at,
            updated_at: now,
        })
    }

    pub async fn load_session(&self, project_id: &str) -> FormalResult<ProjectSessionSecrets> {
        let row = sqlx::query(
            "SELECT local_project_id, username, token_secret_ref, expires_at, updated_at \
             FROM local_project_session WHERE local_project_id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "load project session failed");
            FormalError::LocalDatabase("读取项目会话")
        })?
        .ok_or_else(|| FormalError::NotFound(format!("项目会话不存在：{project_id}")))?;
        let token_ref: String = row
            .try_get("token_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析平台令牌引用"))?;
        let access_token = String::from_utf8(self.secrets.load(&token_ref)?)
            .map_err(|_| FormalError::SecretStore("平台令牌不是UTF-8文本"))?;
        Ok(ProjectSessionSecrets {
            session: LocalProjectSession {
                local_project_id: row
                    .try_get("local_project_id")
                    .map_err(|_| FormalError::LocalDatabase("解析项目会话ID"))?,
                username: row
                    .try_get("username")
                    .map_err(|_| FormalError::LocalDatabase("解析项目会话用户"))?,
                expires_at: row
                    .try_get("expires_at")
                    .map_err(|_| FormalError::LocalDatabase("解析项目会话过期时间"))?,
                updated_at: row
                    .try_get("updated_at")
                    .map_err(|_| FormalError::LocalDatabase("解析项目会话更新时间"))?,
            },
            access_token,
        })
    }

    pub async fn clear_session(&self, project_id: &str) -> FormalResult<()> {
        self.get(project_id).await?;
        let token_ref = self.current_session_secret_ref(project_id).await?;
        sqlx::query("DELETE FROM local_project_session WHERE local_project_id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "clear project session failed");
                FormalError::LocalDatabase("退出项目会话")
            })?;
        if let Some(token_ref) = token_ref {
            self.delete_secret_or_enqueue(&token_ref, "session_logout")
                .await?;
        }
        Ok(())
    }

    pub async fn delete(&self, project_id: &str) -> FormalResult<()> {
        let row = sqlx::query(
            "SELECT project.db_password_secret_ref, session.token_secret_ref \
             FROM local_project project LEFT JOIN local_project_session session \
             ON session.local_project_id = project.id WHERE project.id = ?",
        )
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "read project before delete failed");
                FormalError::LocalDatabase("读取待删除项目")
            })?
            .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        let password_ref: String = row
            .try_get("db_password_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析数据库凭据引用"))?;
        let token_ref: Option<String> = row
            .try_get("token_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析平台令牌引用"))?;
        let master_key_refs: Vec<String> = sqlx::query_scalar(
            "SELECT secret_ref FROM local_project_master_key \
             WHERE local_project_id = ? ORDER BY key_version",
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "read project master keys before delete failed");
            FormalError::LocalDatabase("读取待删除项目主密钥")
        })?;
        sqlx::query("DELETE FROM local_project WHERE id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                map_project_mutation_error(error, "删除本地项目入口", "delete local project failed")
            })?;
        self.delete_secret_or_enqueue(&password_ref, "project_delete_password")
            .await?;
        if let Some(token_ref) = token_ref {
            self.delete_secret_or_enqueue(&token_ref, "project_delete_session")
                .await?;
        }
        for master_key_ref in master_key_refs {
            self.delete_secret_or_enqueue(&master_key_ref, "project_delete_master_key")
                .await?;
        }
        Ok(())
    }
}

fn map_project_mutation_error(
    error: sqlx::Error,
    operation: &'static str,
    log_message: &'static str,
) -> FormalError {
    if error.as_database_error().is_some_and(|database_error| {
        database_error
            .message()
            .contains(ACTIVE_PROJECT_TASK_CONSTRAINT)
    }) {
        return FormalError::Conflict("项目存在活动任务，任务进入终态前不能编辑或删除".into());
    }
    tracing::error!(error = ?crate::core::log_safety::safe_error(&error), operation = log_message, "project mutation failed");
    FormalError::LocalDatabase(operation)
}

fn map_project(row: sqlx::sqlite::SqliteRow) -> FormalResult<LocalProjectRecord> {
    Ok(LocalProjectRecord {
        id: row
            .try_get("id")
            .map_err(|_| FormalError::LocalDatabase("解析项目ID"))?,
        name: row
            .try_get("name")
            .map_err(|_| FormalError::LocalDatabase("解析项目名称"))?,
        platform_url: row
            .try_get("platform_url")
            .map_err(|_| FormalError::LocalDatabase("解析平台地址"))?,
        db_host: row
            .try_get("db_host")
            .map_err(|_| FormalError::LocalDatabase("解析数据库主机"))?,
        db_port: row
            .try_get::<i64, _>("db_port")
            .map_err(|_| FormalError::LocalDatabase("解析数据库端口"))? as u16,
        db_user: row
            .try_get("db_user")
            .map_err(|_| FormalError::LocalDatabase("解析数据库用户"))?,
        business_db: row
            .try_get("business_db")
            .map_err(|_| FormalError::LocalDatabase("解析平台业务库"))?,
        workbench_db: row
            .try_get("workbench_db")
            .map_err(|_| FormalError::LocalDatabase("解析工作台库"))?,
        last_opened_at: row
            .try_get("last_opened_at")
            .map_err(|_| FormalError::LocalDatabase("解析项目打开时间"))?,
    })
}

fn project_matches_update(project: &LocalProjectRecord, input: &UpdateLocalProject) -> bool {
    project.name == input.name.trim()
        && project.platform_url == input.platform_url.trim()
        && project.db_host == input.db_host.trim()
        && project.db_port == input.db_port
        && project.db_user == input.db_user.trim()
        && project.business_db == input.business_db.trim()
        && project.workbench_db == input.workbench_db.trim()
}

fn timestamp() -> String {
    OffsetDateTime::now_utc().unix_timestamp_nanos().to_string()
}
