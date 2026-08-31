use std::path::Path;
use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{ConnectOptions, SqlitePool};

use super::error::{FormalError, FormalResult};

static LOCAL_MIGRATOR: Migrator = sqlx::migrate!("./migrations/local");

#[derive(Clone)]
pub struct LocalStore {
    pool: SqlitePool,
}

impl LocalStore {
    pub async fn open(path: impl AsRef<Path>) -> FormalResult<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5))
            .disable_statement_logging();
        let pool = SqlitePoolOptions::new()
            .min_connections(1)
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await
            .map_err(|error| {
                tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "open local sqlite failed");
                FormalError::LocalDatabase("打开本地数据库")
            })?;
        LOCAL_MIGRATOR.run(&pool).await.map_err(|error| {
            tracing::error!(error = ?crate::core::log_safety::safe_error(&error), "run local sqlite migration failed");
            FormalError::LocalDatabase("执行本地数据库迁移")
        })?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }
}
