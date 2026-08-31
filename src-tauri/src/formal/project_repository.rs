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

#[derive(Clone)]
pub struct LocalProjectRepository {
    pool: SqlitePool,
    secrets: Arc<dyn SecretStore>,
}

impl LocalProjectRepository {
    pub fn new(pool: SqlitePool, secrets: Arc<dyn SecretStore>) -> Self {
        Self { pool, secrets }
    }

    pub async fn create(&self, input: CreateLocalProject) -> FormalResult<LocalProjectRecord> {
        input.validate()?;
        let id = Uuid::now_v7().to_string();
        let secret_ref = format!("project/{id}/database-password");
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
            let _ = self.secrets.delete(&secret_ref);
            tracing::error!(error = ?error, "insert local project failed");
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
            tracing::error!(error = ?error, "list local projects failed");
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
            "SELECT platform_url, db_password_secret_ref FROM local_project WHERE id = ?",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| {
            tracing::error!(error = ?error, "read local project before update failed");
            FormalError::LocalDatabase("读取待编辑项目")
        })?
        .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        let old_platform_url: String = row
            .try_get("platform_url")
            .map_err(|_| FormalError::LocalDatabase("解析原平台地址"))?;
        let old_secret_ref: String = row
            .try_get("db_password_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析数据库凭据引用"))?;
        let new_secret_ref = input
            .db_password
            .as_ref()
            .filter(|password| !password.is_empty())
            .map(|_| format!("project/{project_id}/database-password/{}", Uuid::now_v7()));
        if let (Some(password), Some(secret_ref)) = (&input.db_password, &new_secret_ref) {
            self.secrets.save(secret_ref, password.as_bytes())?;
        }
        let secret_ref = new_secret_ref.as_deref().unwrap_or(&old_secret_ref);
        let mut transaction = self.pool.begin().await.map_err(|error| {
            tracing::error!(error = ?error, "begin local project update failed");
            FormalError::LocalDatabase("开始编辑项目事务")
        })?;
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
                if let Some(secret_ref) = &new_secret_ref {
                    let _ = self.secrets.delete(secret_ref);
                }
                return Err(map_project_mutation_error(
                    error,
                    "编辑本地项目入口",
                    "update local project failed",
                ));
            }
        };
        if result.rows_affected() != 1 {
            if let Some(secret_ref) = &new_secret_ref {
                let _ = self.secrets.delete(secret_ref);
            }
            return Err(FormalError::NotFound(format!("项目不存在：{project_id}")));
        }
        if old_platform_url.trim() != input.platform_url.trim() {
            sqlx::query("DELETE FROM local_project_session WHERE local_project_id = ?")
                .bind(project_id)
                .execute(&mut *transaction)
                .await
                .map_err(|error| {
                    tracing::error!(error = ?error, "invalidate project session after update failed");
                    FormalError::LocalDatabase("使项目会话失效")
                })?;
        }
        transaction.commit().await.map_err(|error| {
            tracing::error!(error = ?error, "commit local project update failed");
            FormalError::LocalDatabase("提交编辑项目事务")
        })?;
        if new_secret_ref.is_some() {
            let _ = self.secrets.delete(&old_secret_ref);
        }
        if old_platform_url.trim() != input.platform_url.trim() {
            let token_ref = format!("project/{project_id}/platform-access-token");
            let _ = self.secrets.delete(&token_ref);
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
            tracing::error!(error = ?error, "get local project failed");
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
            tracing::error!(error = ?error, "load project connection failed");
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
                    tracing::error!(error = ?error, "touch local project failed");
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
        let token_ref = format!("project/{project_id}/platform-access-token");
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
            let _ = self.secrets.delete(&token_ref);
            tracing::error!(error = ?error, "save project session failed");
            return Err(FormalError::LocalDatabase("保存项目会话"));
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
            tracing::error!(error = ?error, "load project session failed");
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
        sqlx::query("DELETE FROM local_project_session WHERE local_project_id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?error, "clear project session failed");
                FormalError::LocalDatabase("退出项目会话")
            })?;
        let token_ref = format!("project/{project_id}/platform-access-token");
        let _ = self.secrets.delete(&token_ref);
        Ok(())
    }

    pub async fn delete(&self, project_id: &str) -> FormalResult<()> {
        let row = sqlx::query("SELECT db_password_secret_ref FROM local_project WHERE id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|error| {
                tracing::error!(error = ?error, "read project before delete failed");
                FormalError::LocalDatabase("读取待删除项目")
            })?
            .ok_or_else(|| FormalError::NotFound(format!("项目不存在：{project_id}")))?;
        let password_ref: String = row
            .try_get("db_password_secret_ref")
            .map_err(|_| FormalError::LocalDatabase("解析数据库凭据引用"))?;
        let token_ref = format!("project/{project_id}/platform-access-token");
        sqlx::query("DELETE FROM local_project WHERE id = ?")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|error| {
                map_project_mutation_error(error, "删除本地项目入口", "delete local project failed")
            })?;
        let _ = self.secrets.delete(&password_ref);
        let _ = self.secrets.delete(&token_ref);
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
    tracing::error!(error = ?error, operation = log_message, "project mutation failed");
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

fn timestamp() -> String {
    OffsetDateTime::now_utc().unix_timestamp_nanos().to_string()
}
