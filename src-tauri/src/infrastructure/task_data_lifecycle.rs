use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use walkdir::WalkDir;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskState;
use crate::formal::config::AppPaths;

const SUCCESS_LOG_RETENTION: Duration = Duration::days(30);
const FAILURE_LOG_RETENTION: Duration = Duration::days(90);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RetentionRecord {
    task_id: String,
    task_state: String,
    delete_after_epoch_seconds: i64,
}

pub struct TaskDataLifecycle<'a> {
    paths: &'a AppPaths,
}

impl<'a> TaskDataLifecycle<'a> {
    pub fn new(paths: &'a AppPaths) -> Self {
        Self { paths }
    }

    pub fn finalize_task(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
    ) -> AppResult<()> {
        self.finalize_task_at(project_id, task_id, state, OffsetDateTime::now_utc())
    }

    pub fn clear_task_log(&self, project_id: &str, task_id: &str) -> AppResult<()> {
        let log = self
            .paths
            .project_task_log_path(project_id, task_id)
            .map_err(crate::infrastructure::project_context::map_formal_error)?;
        remove_file_if_present(&log, "清空任务日志")?;
        remove_file_if_present(&retention_path(&log), "清空任务日志保留标记")?;
        remove_empty_parent(&log, &self.paths.task_logs_dir)
    }

    fn finalize_task_at(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
        now: OffsetDateTime,
    ) -> AppResult<()> {
        if !state.is_terminal() {
            return Err(AppError::Conflict(format!(
                "任务尚未到安全清理边界：{}",
                state.as_str()
            )));
        }
        let artifacts = self
            .paths
            .project_task_dir(project_id, task_id)
            .map_err(crate::infrastructure::project_context::map_formal_error)?;
        if artifacts.exists() {
            fs::remove_dir_all(&artifacts)
                .map_err(|error| AppError::io("清理任务临时制品", &error))?;
        }
        remove_empty_parent(&artifacts, &self.paths.task_artifacts_dir)?;

        let log = self
            .paths
            .project_task_log_path(project_id, task_id)
            .map_err(crate::infrastructure::project_context::map_formal_error)?;
        if log.is_file() {
            let retention = match state {
                TaskState::Succeeded | TaskState::PartiallySucceeded | TaskState::Cancelled => {
                    SUCCESS_LOG_RETENTION
                }
                _ => FAILURE_LOG_RETENTION,
            };
            let record = RetentionRecord {
                task_id: task_id.into(),
                task_state: state.as_str().into(),
                delete_after_epoch_seconds: (now + retention).unix_timestamp(),
            };
            let sidecar = retention_path(&log);
            if !sidecar.exists() {
                write_retention(&sidecar, &record)?;
            }
        }
        Ok(())
    }

    pub fn sweep_expired_logs(&self) -> AppResult<usize> {
        self.sweep_expired_logs_at(OffsetDateTime::now_utc())
    }

    fn sweep_expired_logs_at(&self, now: OffsetDateTime) -> AppResult<usize> {
        if !self.paths.task_logs_dir.exists() {
            return Ok(0);
        }
        let mut removed = 0usize;
        for entry in WalkDir::new(&self.paths.task_logs_dir)
            .min_depth(1)
            .into_iter()
        {
            let entry = entry.map_err(|_| AppError::Io {
                operation: "遍历任务日志保留策略",
            })?;
            if !entry.file_type().is_file()
                || !entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".retention.json")
            {
                continue;
            }
            let sidecar = entry.path();
            let record = read_retention(sidecar)?;
            if record.delete_after_epoch_seconds > now.unix_timestamp() {
                continue;
            }
            let log = log_path_from_retention(sidecar)?;
            if log.is_file() {
                fs::remove_file(&log).map_err(|error| AppError::io("清理超期任务日志", &error))?;
            }
            fs::remove_file(sidecar)
                .map_err(|error| AppError::io("清理任务日志保留标记", &error))?;
            if let Some(parent) = sidecar.parent() {
                remove_empty_parent(parent, &self.paths.task_logs_dir)?;
            }
            removed = removed.saturating_add(1);
        }
        Ok(removed)
    }
}

fn retention_path(log: &Path) -> PathBuf {
    PathBuf::from(format!("{}.retention.json", log.to_string_lossy()))
}

fn remove_file_if_present(path: &Path, operation: &'static str) -> AppResult<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::io(operation, &error)),
    }
}

fn log_path_from_retention(sidecar: &Path) -> AppResult<PathBuf> {
    let value = sidecar.to_string_lossy();
    let value = value
        .strip_suffix(".retention.json")
        .ok_or_else(|| AppError::InvalidConfig("任务日志保留标记名称无效".into()))?;
    Ok(PathBuf::from(value))
}

fn write_retention(path: &Path, record: &RetentionRecord) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::InvalidConfig("任务日志保留标记没有父目录".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io("创建任务日志保留目录", &error))?;
    let bytes = serde_json::to_vec(record)
        .map_err(|_| AppError::InvalidConfig("序列化任务日志保留策略失败".into()))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, bytes).map_err(|error| AppError::io("写入任务日志保留标记", &error))?;
    fs::rename(&temporary, path).map_err(|error| AppError::io("发布任务日志保留标记", &error))
}

fn read_retention(path: &Path) -> AppResult<RetentionRecord> {
    let bytes = fs::read(path).map_err(|error| AppError::io("读取任务日志保留标记", &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::InvalidConfig("任务日志保留标记无法解析".into()))
}

fn remove_empty_parent(path: &Path, boundary: &Path) -> AppResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    if parent == boundary || !parent.starts_with(boundary) || !parent.is_dir() {
        return Ok(());
    }
    let mut entries =
        fs::read_dir(parent).map_err(|error| AppError::io("检查空任务目录", &error))?;
    if entries
        .next()
        .transpose()
        .map_err(|error| AppError::io("检查空任务目录", &error))?
        .is_none()
    {
        fs::remove_dir(parent).map_err(|error| AppError::io("清理空任务目录", &error))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use time::{Duration, OffsetDateTime};

    use super::{TaskDataLifecycle, read_retention, retention_path};
    use crate::domain::common::task::TaskState;
    use crate::formal::config::AppPaths;

    #[test]
    fn success_failure_and_expiry_policies_remove_artifacts_and_expire_logs() {
        let root = tempfile::tempdir().expect("root");
        let paths = AppPaths::from_data_dir(root.path()).expect("paths");
        paths.ensure().expect("ensure");
        let lifecycle = TaskDataLifecycle::new(&paths);
        let now = OffsetDateTime::from_unix_timestamp(2_000_000_000).expect("time");

        for (task_id, state, days) in [
            ("success", TaskState::Succeeded, 30i64),
            ("failure", TaskState::Failed, 90i64),
        ] {
            let artifacts = paths
                .project_task_dir("project", task_id)
                .expect("artifacts");
            std::fs::create_dir_all(&artifacts).expect("artifact dir");
            std::fs::write(artifacts.join(".env"), b"secret").expect("artifact");
            let log = paths
                .project_task_log_path("project", task_id)
                .expect("log");
            std::fs::create_dir_all(log.parent().expect("parent")).expect("log dir");
            std::fs::write(&log, b"redacted log").expect("log");

            lifecycle
                .finalize_task_at("project", task_id, state, now)
                .expect("finalize");
            assert!(!artifacts.exists());
            let record = read_retention(&retention_path(&log)).expect("retention");
            assert_eq!(
                record.delete_after_epoch_seconds,
                (now + Duration::days(days)).unix_timestamp()
            );
        }

        assert_eq!(
            lifecycle
                .sweep_expired_logs_at(now + Duration::days(31))
                .expect("sweep success"),
            1
        );
        assert!(
            !paths
                .project_task_log_path("project", "success")
                .expect("success log")
                .exists()
        );
        assert!(
            paths
                .project_task_log_path("project", "failure")
                .expect("failure log")
                .exists()
        );
        assert_eq!(
            lifecycle
                .sweep_expired_logs_at(now + Duration::days(91))
                .expect("sweep failure"),
            1
        );
    }

    #[test]
    fn active_task_artifacts_cannot_be_cleaned() {
        let root = tempfile::tempdir().expect("root");
        let paths = AppPaths::from_data_dir(root.path()).expect("paths");
        paths.ensure().expect("ensure");
        assert!(
            TaskDataLifecycle::new(&paths)
                .finalize_task("project", "running", TaskState::Running)
                .is_err()
        );
        let retry_artifacts = paths
            .project_task_dir("project", "finalizing")
            .expect("retry artifacts");
        std::fs::create_dir_all(&retry_artifacts).expect("retry dir");
        std::fs::write(
            retry_artifacts.join("pending-local-finalization.json"),
            b"retry",
        )
        .expect("retry marker");
        assert!(
            TaskDataLifecycle::new(&paths)
                .finalize_task("project", "finalizing", TaskState::FinalizingFailed)
                .is_err()
        );
        assert!(retry_artifacts.exists());
    }

    #[test]
    fn clearing_a_task_log_removes_only_the_trusted_log_and_retention_marker() {
        let root = tempfile::tempdir().expect("root");
        let paths = AppPaths::from_data_dir(root.path()).expect("paths");
        paths.ensure().expect("ensure");
        let lifecycle = TaskDataLifecycle::new(&paths);
        let log = paths
            .project_task_log_path("project", "terminal-task")
            .expect("task log");
        std::fs::create_dir_all(log.parent().expect("log parent")).expect("log directory");
        std::fs::write(&log, b"event").expect("task log");
        std::fs::write(retention_path(&log), b"{}").expect("retention");

        lifecycle
            .clear_task_log("project", "terminal-task")
            .expect("clear task log");
        assert!(!log.exists());
        assert!(!retention_path(&log).exists());
        lifecycle
            .clear_task_log("project", "terminal-task")
            .expect("repeat clear task log");
    }
}
