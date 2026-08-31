use std::fs;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use walkdir::WalkDir;

use crate::application::data_directory::{
    DataDirectoryStatus, DataDirectorySwitchMode, DataDirectorySwitchRequest,
};
use crate::application::ports::data_directory::DataDirectoryPort;
use crate::core::error::{AppError, AppResult};

const BOOTSTRAP_VERSION: u32 = 1;
const BOOTSTRAP_DIR: &str = ".bootstrap";
const BOOTSTRAP_FILE: &str = "data-directory.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingSwitch {
    target_directory: String,
    mode: DataDirectorySwitchMode,
    requested_at: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapConfig {
    version: u32,
    active_directory: Option<String>,
    previous_directory: Option<String>,
    pending_switch: Option<PendingSwitch>,
    last_switch_error: Option<String>,
}

impl Default for BootstrapConfig {
    fn default() -> Self {
        Self {
            version: BOOTSTRAP_VERSION,
            active_directory: None,
            previous_directory: None,
            pending_switch: None,
            last_switch_error: None,
        }
    }
}

#[derive(Debug)]
pub struct DataDirectoryManager {
    default_directory: PathBuf,
    bootstrap_file: PathBuf,
    config: RwLock<BootstrapConfig>,
}

impl DataDirectoryManager {
    pub fn resolve(default_directory: impl AsRef<Path>) -> AppResult<(PathBuf, Self)> {
        let default_directory = absolute_directory(default_directory.as_ref())?;
        let bootstrap_file = default_directory.join(BOOTSTRAP_DIR).join(BOOTSTRAP_FILE);
        let mut config = read_config(&bootstrap_file)?;
        if config.version != BOOTSTRAP_VERSION {
            return Err(AppError::InvalidConfig(format!(
                "不支持的数据目录启动配置版本：{}",
                config.version
            )));
        }
        let current = config
            .active_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| default_directory.clone());
        let active = if let Some(pending) = config.pending_switch.clone() {
            match apply_pending_switch(&default_directory, &current, &bootstrap_file, &pending) {
                Ok(target) => {
                    config.previous_directory = Some(current.to_string_lossy().into_owned());
                    config.active_directory = Some(target.to_string_lossy().into_owned());
                    config.pending_switch = None;
                    config.last_switch_error = None;
                    if let Err(error) = write_config(&bootstrap_file, &config) {
                        rollback_created_target(&target, &pending);
                        return Err(error);
                    }
                    target
                }
                Err(error) => {
                    config.pending_switch = None;
                    config.last_switch_error = Some(error.to_string());
                    write_config(&bootstrap_file, &config)?;
                    current
                }
            }
        } else {
            current
        };
        Ok((
            active,
            Self {
                default_directory,
                bootstrap_file,
                config: RwLock::new(config),
            },
        ))
    }

    fn active_directory(&self, config: &BootstrapConfig) -> PathBuf {
        config
            .active_directory
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.default_directory.clone())
    }

    fn write_locked(&self, config: &BootstrapConfig) -> AppResult<()> {
        write_config(&self.bootstrap_file, config)
    }
}

impl DataDirectoryPort for DataDirectoryManager {
    fn status(&self) -> AppResult<DataDirectoryStatus> {
        let config = self
            .config
            .read()
            .map_err(|_| AppError::Conflict("数据目录启动配置锁已损坏".into()))?;
        let active = self.active_directory(&config);
        Ok(DataDirectoryStatus {
            active_directory: active.to_string_lossy().into_owned(),
            default_directory: self.default_directory.to_string_lossy().into_owned(),
            pending_directory: config
                .pending_switch
                .as_ref()
                .map(|pending| pending.target_directory.clone()),
            previous_directory: config.previous_directory.clone(),
            pending_mode: config.pending_switch.as_ref().map(|pending| pending.mode),
            restart_required: config.pending_switch.is_some(),
            first_setup: config.active_directory.is_none()
                && config.previous_directory.is_none()
                && config.pending_switch.is_none(),
            last_switch_error: config.last_switch_error.clone(),
        })
    }

    fn schedule_switch(
        &self,
        request: DataDirectorySwitchRequest,
    ) -> AppResult<DataDirectoryStatus> {
        let target = absolute_directory(Path::new(request.target_directory.trim()))?;
        let mut config = self
            .config
            .write()
            .map_err(|_| AppError::Conflict("数据目录启动配置锁已损坏".into()))?;
        let active = self.active_directory(&config);
        validate_switch_target(&active, &target, false)?;
        config.pending_switch = Some(PendingSwitch {
            target_directory: target.to_string_lossy().into_owned(),
            mode: request.mode,
            requested_at: OffsetDateTime::now_utc().to_string(),
        });
        config.last_switch_error = None;
        self.write_locked(&config)?;
        drop(config);
        self.status()
    }

    fn schedule_rollback(&self) -> AppResult<DataDirectoryStatus> {
        let mut config = self
            .config
            .write()
            .map_err(|_| AppError::Conflict("数据目录启动配置锁已损坏".into()))?;
        let active = self.active_directory(&config);
        let previous = config
            .previous_directory
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::NotFound("没有可回滚的上一数据目录".into()))?;
        validate_switch_target(&active, &previous, true)?;
        config.pending_switch = Some(PendingSwitch {
            target_directory: previous.to_string_lossy().into_owned(),
            mode: DataDirectorySwitchMode::UseExisting,
            requested_at: OffsetDateTime::now_utc().to_string(),
        });
        config.last_switch_error = None;
        self.write_locked(&config)?;
        drop(config);
        self.status()
    }
}

fn apply_pending_switch(
    default_directory: &Path,
    current: &Path,
    bootstrap_file: &Path,
    pending: &PendingSwitch,
) -> AppResult<PathBuf> {
    let target = absolute_directory(Path::new(&pending.target_directory))?;
    validate_switch_target(
        current,
        &target,
        pending.mode == DataDirectorySwitchMode::UseExisting,
    )?;
    match pending.mode {
        DataDirectorySwitchMode::UseExisting => {
            if !target.join("local.db").is_file() {
                return Err(AppError::InvalidConfig(
                    "回滚目录缺少local.db，已保持当前数据目录".into(),
                ));
            }
        }
        DataDirectorySwitchMode::Empty => {
            create_empty_target(&target)?;
        }
        DataDirectorySwitchMode::Migrate => {
            create_empty_target(&target)?;
            if current.exists()
                && let Err(error) =
                    copy_directory(current, &target, default_directory, bootstrap_file)
            {
                let _ = fs::remove_dir_all(&target);
                return Err(error);
            }
        }
    }
    Ok(target)
}

fn create_empty_target(target: &Path) -> AppResult<()> {
    if target.exists() {
        let mut entries =
            fs::read_dir(target).map_err(|error| AppError::io("检查目标数据目录", &error))?;
        if entries
            .next()
            .transpose()
            .map_err(|error| AppError::io("检查目标数据目录", &error))?
            .is_some()
        {
            return Err(AppError::Conflict(
                "目标数据目录不是空目录，已阻止覆盖".into(),
            ));
        }
    } else {
        fs::create_dir_all(target).map_err(|error| AppError::io("创建目标数据目录", &error))?;
    }
    Ok(())
}

fn copy_directory(
    source: &Path,
    target: &Path,
    default_directory: &Path,
    bootstrap_file: &Path,
) -> AppResult<()> {
    let skip_bootstrap = source == default_directory;
    for entry in WalkDir::new(source).min_depth(1).into_iter() {
        let entry = entry.map_err(|_| AppError::Io {
            operation: "遍历原数据目录",
        })?;
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| AppError::Io {
                operation: "计算数据目录相对路径",
            })?;
        if skip_bootstrap
            && relative
                .components()
                .next()
                .is_some_and(|component| component.as_os_str() == BOOTSTRAP_DIR)
        {
            continue;
        }
        if entry.path() == bootstrap_file {
            continue;
        }
        let destination = target.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&destination)
                .map_err(|error| AppError::io("迁移数据目录结构", &error))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| AppError::io("迁移数据目录结构", &error))?;
            }
            fs::copy(entry.path(), &destination)
                .map_err(|error| AppError::io("迁移数据目录文件", &error))?;
        }
    }
    Ok(())
}

fn validate_switch_target(current: &Path, target: &Path, allow_existing: bool) -> AppResult<()> {
    if current == target {
        return Err(AppError::Conflict("目标数据目录与当前目录相同".into()));
    }
    if paths_overlap(current, target) {
        return Err(AppError::InvalidConfig("新旧数据目录不能互相包含".into()));
    }
    if target.parent().is_none() {
        return Err(AppError::InvalidConfig("不能使用磁盘根目录".into()));
    }
    if target.exists() {
        if !target.is_dir() {
            return Err(AppError::InvalidConfig("目标数据目录不是目录".into()));
        }
        if !allow_existing
            && fs::read_dir(target)
                .map_err(|error| AppError::io("检查目标数据目录", &error))?
                .next()
                .transpose()
                .map_err(|error| AppError::io("检查目标数据目录", &error))?
                .is_some()
        {
            return Err(AppError::Conflict(
                "目标数据目录不是空目录，已阻止覆盖".into(),
            ));
        }
    } else if allow_existing {
        return Err(AppError::NotFound("上一数据目录不存在，无法回滚".into()));
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = normalized_path(left);
    let right = normalized_path(right);
    left.starts_with(&(right.clone() + "\\")) || right.starts_with(&(left.clone() + "\\"))
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

fn absolute_directory(path: &Path) -> AppResult<PathBuf> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(AppError::InvalidConfig(
            "工作台数据目录必须是绝对路径".into(),
        ));
    }
    Ok(path.to_path_buf())
}

fn read_config(path: &Path) -> AppResult<BootstrapConfig> {
    let backup = backup_path(path);
    let candidate = if path.is_file() {
        Some(path)
    } else if backup.is_file() {
        Some(backup.as_path())
    } else {
        None
    };
    let Some(candidate) = candidate else {
        return Ok(BootstrapConfig::default());
    };
    let bytes =
        fs::read(candidate).map_err(|error| AppError::io("读取数据目录启动配置", &error))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| AppError::InvalidConfig("数据目录启动配置无法解析".into()))
}

fn write_config(path: &Path, config: &BootstrapConfig) -> AppResult<()> {
    let parent = path
        .parent()
        .ok_or_else(|| AppError::InvalidConfig("数据目录启动配置路径无父目录".into()))?;
    fs::create_dir_all(parent).map_err(|error| AppError::io("创建数据目录启动配置目录", &error))?;
    let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
    let backup = backup_path(path);
    let bytes = serde_json::to_vec_pretty(config)
        .map_err(|_| AppError::InvalidConfig("序列化数据目录启动配置失败".into()))?;
    fs::write(&temporary, bytes).map_err(|error| AppError::io("写入数据目录启动配置", &error))?;
    if backup.exists() {
        fs::remove_file(&backup)
            .map_err(|error| AppError::io("清理数据目录启动配置备份", &error))?;
    }
    if path.exists() {
        fs::rename(path, &backup).map_err(|error| AppError::io("备份数据目录启动配置", &error))?;
    }
    if let Err(error) = fs::rename(&temporary, path) {
        if backup.exists() {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&temporary);
        return Err(AppError::io("发布数据目录启动配置", &error));
    }
    if backup.exists() {
        fs::remove_file(&backup)
            .map_err(|error| AppError::io("清理数据目录启动配置备份", &error))?;
    }
    Ok(())
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("json.bak")
}

fn rollback_created_target(target: &Path, pending: &PendingSwitch) {
    if pending.mode != DataDirectorySwitchMode::UseExisting {
        let _ = fs::remove_dir_all(target);
    }
}

#[cfg(test)]
mod tests {
    use super::{BootstrapConfig, DataDirectoryManager, PendingSwitch, write_config};
    use crate::application::data_directory::{DataDirectorySwitchMode, DataDirectorySwitchRequest};
    use crate::application::ports::data_directory::DataDirectoryPort;

    #[test]
    fn migration_restart_and_rollback_change_the_effective_directory() {
        let root = tempfile::tempdir().expect("root");
        let default = root.path().join("default");
        let target = root.path().join("custom");
        std::fs::create_dir_all(&default).expect("default");
        std::fs::write(default.join("local.db"), b"sqlite").expect("db");
        std::fs::create_dir_all(default.join("logs")).expect("logs");
        std::fs::write(default.join("logs/app.log"), b"log").expect("log");

        let (_, manager) = DataDirectoryManager::resolve(&default).expect("initial");
        manager
            .schedule_switch(DataDirectorySwitchRequest {
                target_directory: target.to_string_lossy().into_owned(),
                mode: DataDirectorySwitchMode::Migrate,
            })
            .expect("schedule");
        assert!(manager.status().expect("status").restart_required);
        drop(manager);

        let (active, manager) = DataDirectoryManager::resolve(&default).expect("migrated");
        assert_eq!(active, target);
        assert_eq!(
            std::fs::read(target.join("local.db")).expect("db"),
            b"sqlite"
        );
        assert!(target.join("logs/app.log").is_file());
        manager.schedule_rollback().expect("rollback request");
        drop(manager);

        let (active, manager) = DataDirectoryManager::resolve(&default).expect("rollback");
        assert_eq!(active, default);
        assert_eq!(
            manager.status().expect("status").previous_directory,
            Some(target.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn failed_pending_migration_keeps_current_directory_and_records_error() {
        let root = tempfile::tempdir().expect("root");
        let default = root.path().join("default");
        let target = root.path().join("non-empty");
        std::fs::create_dir_all(&default).expect("default");
        std::fs::write(default.join("local.db"), b"sqlite").expect("db");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::write(target.join("keep.txt"), b"keep").expect("keep");
        let bootstrap = default.join(".bootstrap/data-directory.json");
        write_config(
            &bootstrap,
            &BootstrapConfig {
                pending_switch: Some(PendingSwitch {
                    target_directory: target.to_string_lossy().into_owned(),
                    mode: DataDirectorySwitchMode::Migrate,
                    requested_at: "test".into(),
                }),
                ..BootstrapConfig::default()
            },
        )
        .expect("pending config");

        let (active, manager) = DataDirectoryManager::resolve(&default).expect("fallback");
        assert_eq!(active, default);
        let status = manager.status().expect("status");
        assert!(status.last_switch_error.is_some());
        assert_eq!(
            std::fs::read(target.join("keep.txt")).expect("keep"),
            b"keep"
        );
    }

    #[test]
    fn nested_and_non_empty_targets_are_rejected_before_restart() {
        let root = tempfile::tempdir().expect("root");
        let default = root.path().join("default");
        std::fs::create_dir_all(&default).expect("default");
        let (_, manager) = DataDirectoryManager::resolve(&default).expect("manager");
        assert!(
            manager
                .schedule_switch(DataDirectorySwitchRequest {
                    target_directory: default.join("nested").to_string_lossy().into_owned(),
                    mode: DataDirectorySwitchMode::Migrate,
                })
                .is_err()
        );
        let non_empty = root.path().join("non-empty");
        std::fs::create_dir_all(&non_empty).expect("non-empty");
        std::fs::write(non_empty.join("keep.txt"), b"keep").expect("keep");
        assert!(
            manager
                .schedule_switch(DataDirectorySwitchRequest {
                    target_directory: non_empty.to_string_lossy().into_owned(),
                    mode: DataDirectorySwitchMode::Empty,
                })
                .is_err()
        );
    }
}
