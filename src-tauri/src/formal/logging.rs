use std::path::Path;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

use super::error::{FormalError, FormalResult};

pub fn init_file_logging(logs_dir: &Path) -> FormalResult<WorkerGuard> {
    std::fs::create_dir_all(logs_dir).map_err(|error| {
        tracing::error!(path = %logs_dir.display(), error = ?error, "create logs directory failed");
        FormalError::LocalIo("创建日志目录")
    })?;
    let appender = tracing_appender::rolling::daily(logs_dir, "app.log");
    let (writer, guard) = tracing_appender::non_blocking(appender);
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(writer)
        .with_ansi(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber).map_err(|error| {
        eprintln!("set tracing subscriber failed: {error}");
        FormalError::LocalIo("初始化结构化日志")
    })?;
    Ok(guard)
}
