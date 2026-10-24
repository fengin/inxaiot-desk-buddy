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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrphanArtifactCandidate {
    pub project_id: String,
    pub task_id: String,
}

#[derive(Clone, Copy)]
enum ArtifactPolicy {
    AioTemporaryDirectory,
    ScreenApk,
    Keep,
}

impl<'a> TaskDataLifecycle<'a> {
    pub fn new(paths: &'a AppPaths) -> Self {
        Self { paths }
    }

    /// 仅供已有一体机调用兼容使用；公共任务和智能屏必须传入实际业务及操作类型。
    pub fn finalize_task(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
    ) -> AppResult<()> {
        self.finalize_task_at(project_id, task_id, state, OffsetDateTime::now_utc())
    }

    pub fn finalize_task_for_operation(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
        domain_type: &str,
        operation_type: &str,
    ) -> AppResult<()> {
        let policy = match (domain_type, operation_type) {
            (
                "aio",
                "first_deploy"
                | "full_upgrade"
                | "service_upgrade"
                | "deployment_preflight"
                | "service_inspection",
            ) => ArtifactPolicy::AioTemporaryDirectory,
            ("smart_screen", "install") => ArtifactPolicy::ScreenApk,
            _ => ArtifactPolicy::Keep,
        };
        self.finalize_with_policy(
            project_id,
            task_id,
            state,
            OffsetDateTime::now_utc(),
            policy,
        )
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

    /// 待核实任务可能带着旧版本写入的保留标记；只解除过期标记，不删除日志正文。
    pub fn protect_task_log(&self, project_id: &str, task_id: &str) -> AppResult<()> {
        validate_identity(project_id)?;
        validate_identity(task_id)?;
        let root = &self.paths.task_logs_dir;
        if !require_plain_directory_if_present(root)? {
            return Ok(());
        }
        let project = root.join(project_id);
        if !require_plain_directory_if_present(&project)? {
            return Ok(());
        }
        let canonical_root =
            fs::canonicalize(root).map_err(|error| AppError::io("核实任务日志根目录", &error))?;
        let canonical_project = fs::canonicalize(&project)
            .map_err(|error| AppError::io("核实任务日志项目目录", &error))?;
        if canonical_project == canonical_root || !canonical_project.starts_with(&canonical_root) {
            return Err(AppError::Conflict("任务日志目录超出受管范围".into()));
        }
        let log = self
            .paths
            .project_task_log_path(project_id, task_id)
            .map_err(crate::infrastructure::project_context::map_formal_error)?;
        let marker = retention_path(&log);
        if require_plain_file(&marker)? {
            remove_file_if_present(&marker, "解除待核实任务日志过期标记")?;
        }
        Ok(())
    }

    fn finalize_task_at(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
        now: OffsetDateTime,
    ) -> AppResult<()> {
        self.finalize_with_policy(
            project_id,
            task_id,
            state,
            now,
            ArtifactPolicy::AioTemporaryDirectory,
        )
    }

    fn finalize_with_policy(
        &self,
        project_id: &str,
        task_id: &str,
        state: TaskState,
        now: OffsetDateTime,
        policy: ArtifactPolicy,
    ) -> AppResult<()> {
        if !state.is_terminal() {
            return Err(AppError::Conflict(format!(
                "任务尚未到安全清理边界：{}",
                state.as_str()
            )));
        }
        validate_identity(project_id)?;
        validate_identity(task_id)?;
        // 一体机可能在远端已经产生副作用、但本机尚未写出补写文件时中断。
        // 没有恢复文件不等于结果已确认，不能据此释放制品或开始日志过期计时。
        if state == TaskState::Interrupted
            && matches!(policy, ArtifactPolicy::AioTemporaryDirectory)
        {
            return self.protect_task_log(project_id, task_id);
        }
        // 先保留日志期限；临时文件被占用而删除失败时，日志仍有完整保留策略。
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
        if matches!(policy, ArtifactPolicy::Keep) {
            return Ok(());
        }
        let Some(artifacts) = self.checked_artifact_dir(project_id, task_id)? else {
            return Ok(());
        };
        if state == TaskState::Interrupted && has_pending_finalization(&artifacts)? {
            return Ok(());
        }
        match policy {
            ArtifactPolicy::AioTemporaryDirectory => {
                validate_tree_without_links(&artifacts)?;
                fs::remove_dir_all(&artifacts)
                    .map_err(|error| AppError::io("清理任务临时制品", &error))?;
            }
            ArtifactPolicy::ScreenApk => {
                remove_managed_file(&artifacts.join("xiaoxin.apk"))?;
                remove_empty_directory(&artifacts)?;
            }
            ArtifactPolicy::Keep => {}
        }
        remove_empty_parent(&artifacts, &self.paths.task_artifacts_dir)?;
        Ok(())
    }

    /// 只枚举受管的项目/任务两级目录。调用方必须在持有实例锁时核实数据库无引用。
    pub fn list_orphan_artifact_candidates(&self) -> AppResult<Vec<OrphanArtifactCandidate>> {
        let root = &self.paths.task_artifacts_dir;
        if !require_plain_directory_if_present(root)? {
            return Ok(Vec::new());
        }
        let mut candidates = Vec::new();
        for project in
            fs::read_dir(root).map_err(|error| AppError::io("读取任务制品项目目录", &error))?
        {
            let project = project.map_err(|error| AppError::io("读取任务制品项目项", &error))?;
            let Some(project_id) = project.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_plain_directory(&project.path()) || validate_identity(&project_id).is_err() {
                continue;
            }
            for task in fs::read_dir(project.path())
                .map_err(|error| AppError::io("读取任务制品任务目录", &error))?
            {
                let task = task.map_err(|error| AppError::io("读取任务制品任务项", &error))?;
                let Some(task_id) = task.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_plain_directory(&task.path()) || validate_identity(&task_id).is_err() {
                    continue;
                }
                let Some(directory) = self.checked_artifact_dir(&project_id, &task_id)? else {
                    continue;
                };
                if has_pending_finalization(&directory)? {
                    continue;
                }
                if !orphan_large_files(&directory)?.is_empty() {
                    candidates.push(OrphanArtifactCandidate {
                        project_id: project_id.clone(),
                        task_id,
                    });
                }
            }
        }
        Ok(candidates)
    }

    /// 只回收无任务引用的已知大文件，不删除日志、诊断、恢复依据或未知目录内容。
    pub fn clear_orphan_artifacts(&self, project_id: &str, task_id: &str) -> AppResult<usize> {
        let Some(directory) = self.checked_artifact_dir(project_id, task_id)? else {
            return Ok(0);
        };
        if has_pending_finalization(&directory)? {
            return Ok(0);
        }
        let files = orphan_large_files(&directory)?;
        // 删除前先验证全部候选，不因途中发现链接而已经部分删除。
        for file in &files {
            require_plain_file(file)?;
        }
        let mut removed = 0;
        for file in files {
            removed += usize::from(remove_managed_file(&file)?);
        }
        for release in ["generated-release", "generated-release.part"] {
            let path = directory.join(release);
            remove_empty_directory(&path.join("images"))?;
            remove_empty_directory(&path)?;
        }
        remove_empty_directory(&directory)?;
        remove_empty_parent(&directory, &self.paths.task_artifacts_dir)?;
        Ok(removed)
    }

    fn checked_artifact_dir(&self, project_id: &str, task_id: &str) -> AppResult<Option<PathBuf>> {
        validate_identity(project_id)?;
        validate_identity(task_id)?;
        let root = &self.paths.task_artifacts_dir;
        if !require_plain_directory_if_present(root)? {
            return Ok(None);
        }
        let project = root.join(project_id);
        if !require_plain_directory_if_present(&project)? {
            return Ok(None);
        }
        let task = project.join(task_id);
        if !require_plain_directory_if_present(&task)? {
            return Ok(None);
        }
        let canonical_root =
            fs::canonicalize(root).map_err(|error| AppError::io("核实任务制品根目录", &error))?;
        let canonical_task =
            fs::canonicalize(&task).map_err(|error| AppError::io("核实任务制品目录", &error))?;
        if canonical_task == canonical_root || !canonical_task.starts_with(&canonical_root) {
            return Err(AppError::Conflict("任务制品目录超出受管范围".into()));
        }
        Ok(Some(task))
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

fn validate_identity(value: &str) -> AppResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains(['/', '\\', ':'])
        || value.ends_with(['.', ' '])
    {
        return Err(AppError::InvalidConfig("任务制品路径标识符无效".into()));
    }
    Ok(())
}

fn has_pending_finalization(directory: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(directory.join("pending-local-finalization.json")) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io("核实任务待补写依据", &error)),
    }
}

fn is_link(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Windows junction 等重解析点不一定被 file_type 判为普通符号链接。
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    false
}

fn is_plain_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir() && !is_link(&metadata))
}

fn require_plain_directory_if_present(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) || !metadata.is_dir() => Err(AppError::Conflict(
            "任务制品目录不是普通目录，已保留文件".into(),
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io("核实任务制品目录类型", &error)),
    }
}

fn require_plain_file(path: &Path) -> AppResult<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if is_link(&metadata) || !metadata.is_file() => Err(AppError::Conflict(
            "任务制品不是普通文件，已保留文件".into(),
        )),
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io("核实任务制品文件类型", &error)),
    }
}

fn validate_tree_without_links(directory: &Path) -> AppResult<()> {
    for entry in WalkDir::new(directory).follow_links(false) {
        let entry = entry.map_err(|_| AppError::Io {
            operation: "核实任务临时制品子目录",
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|error| AppError::io("核实任务临时制品路径类型", &error))?;
        if is_link(&metadata) {
            return Err(AppError::Conflict(
                "任务临时制品中存在链接，已保留目录".into(),
            ));
        }
    }
    Ok(())
}

fn remove_managed_file(path: &Path) -> AppResult<bool> {
    if !require_plain_file(path)? {
        return Ok(false);
    }
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AppError::io("清理任务安装包副本", &error)),
    }
}

fn remove_empty_directory(path: &Path) -> AppResult<()> {
    if !require_plain_directory_if_present(path)? {
        return Ok(());
    }
    let mut entries = fs::read_dir(path).map_err(|error| AppError::io("检查任务空目录", &error))?;
    if entries
        .next()
        .transpose()
        .map_err(|error| AppError::io("检查任务空目录项", &error))?
        .is_none()
    {
        fs::remove_dir(path).map_err(|error| AppError::io("清理任务空目录", &error))?;
    }
    Ok(())
}

fn orphan_large_files(directory: &Path) -> AppResult<Vec<PathBuf>> {
    let mut files = Vec::new();
    for name in [
        "xiaoxin.apk",
        "release.tar",
        "service-image.tar",
        "service-image.tar.part",
    ] {
        let file = directory.join(name);
        if require_plain_file(&file)? {
            files.push(file);
        }
    }
    for release in ["generated-release", "generated-release.part"] {
        let release = directory.join(release);
        if !require_plain_directory_if_present(&release)? {
            continue;
        }
        let images = release.join("images");
        if !require_plain_directory_if_present(&images)? {
            continue;
        }
        for entry in
            fs::read_dir(images).map_err(|error| AppError::io("读取遗留镜像快照目录", &error))?
        {
            let entry = entry.map_err(|error| AppError::io("读取遗留镜像快照文件", &error))?;
            let path = entry.path();
            if path.extension().is_some_and(|extension| extension == "tar")
                && require_plain_file(&path)?
            {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn retention_path(log: &Path) -> PathBuf {
    PathBuf::from(format!("{}.retention.json", log.to_string_lossy()))
}

#[cfg(test)]
#[path = "task_data_lifecycle_cleanup_tests.rs"]
mod cleanup_tests;

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
