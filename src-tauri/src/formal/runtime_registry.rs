use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use time::OffsetDateTime;
use tokio::sync::{OnceCell, RwLock};

use super::error::{FormalError, FormalResult};
use crate::core::error::AppResult;
use crate::infrastructure::database::DualMySqlPools;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionHealth {
    Connecting,
    Ready,
    Degraded,
    Closed,
}

pub struct ProjectRuntime {
    pub local_project_id: String,
    pub opened_at: OffsetDateTime,
    health: RwLock<ConnectionHealth>,
    database: OnceCell<Arc<DualMySqlPools>>,
    platform_write: OnceCell<sqlx::MySqlPool>,
}

impl ProjectRuntime {
    pub fn new(local_project_id: impl Into<String>) -> FormalResult<Self> {
        let local_project_id = local_project_id.into();
        if local_project_id.is_empty() {
            return Err(FormalError::InvalidConfig("本地项目ID不能为空".into()));
        }
        Ok(Self {
            local_project_id,
            opened_at: OffsetDateTime::now_utc(),
            health: RwLock::new(ConnectionHealth::Connecting),
            database: OnceCell::new(),
            platform_write: OnceCell::new(),
        })
    }

    pub async fn health(&self) -> ConnectionHealth {
        *self.health.read().await
    }

    pub async fn set_health(&self, health: ConnectionHealth) {
        *self.health.write().await = health;
    }

    pub async fn database_or_try_init<F, Fut>(
        &self,
        initializer: F,
    ) -> AppResult<Arc<DualMySqlPools>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = AppResult<DualMySqlPools>>,
    {
        let pools = self
            .database
            .get_or_try_init(|| async { initializer().await.map(Arc::new) })
            .await?;
        Ok(pools.clone())
    }

    pub fn database(&self) -> Option<Arc<DualMySqlPools>> {
        self.database.get().cloned()
    }

    /// 与普通只读连接独立；只有已校验权限的限定业务方法使用。
    pub async fn platform_write_or_try_init<F, Fut>(&self, initializer: F) -> AppResult<sqlx::MySqlPool>
    where F: FnOnce() -> Fut, Fut: Future<Output = AppResult<sqlx::MySqlPool>> {
        self.platform_write.get_or_try_init(initializer).await.cloned()
    }

    pub async fn close_database(&self) {
        if let Some(pool) = self.platform_write.get() { pool.close().await; }
        if let Some(pools) = self.database.get() {
            pools.close().await;
        }
    }
}

#[derive(Clone, Default)]
pub struct ProjectRuntimeRegistry {
    runtimes: Arc<RwLock<HashMap<String, Arc<ProjectRuntime>>>>,
}

impl ProjectRuntimeRegistry {
    pub async fn open(&self, project_id: &str) -> FormalResult<Arc<ProjectRuntime>> {
        if let Some(runtime) = self.runtimes.read().await.get(project_id).cloned() {
            return Ok(runtime);
        }
        let runtime = Arc::new(ProjectRuntime::new(project_id)?);
        let mut runtimes = self.runtimes.write().await;
        Ok(runtimes
            .entry(project_id.to_string())
            .or_insert_with(|| runtime.clone())
            .clone())
    }

    pub async fn get(&self, project_id: &str) -> Option<Arc<ProjectRuntime>> {
        self.runtimes.read().await.get(project_id).cloned()
    }

    pub async fn close(&self, project_id: &str) -> FormalResult<()> {
        let runtime = self
            .runtimes
            .write()
            .await
            .remove(project_id)
            .ok_or_else(|| FormalError::NotFound(format!("项目运行时不存在：{project_id}")))?;
        runtime.close_database().await;
        runtime.set_health(ConnectionHealth::Closed).await;
        Ok(())
    }

    pub async fn close_all(&self) {
        let runtimes = self
            .runtimes
            .write()
            .await
            .drain()
            .map(|(_, runtime)| runtime)
            .collect::<Vec<_>>();
        for runtime in runtimes {
            runtime.close_database().await;
            runtime.set_health(ConnectionHealth::Closed).await;
        }
    }

    pub async fn len(&self) -> usize {
        self.runtimes.read().await.len()
    }

    pub async fn is_empty(&self) -> bool {
        self.runtimes.read().await.is_empty()
    }
}
