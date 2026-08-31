use std::path::{Path, PathBuf};

use super::error::{FormalError, FormalResult};

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub data_dir: PathBuf,
    pub local_db: PathBuf,
    pub known_hosts: PathBuf,
    pub logs_dir: PathBuf,
    pub projects_dir: PathBuf,
    pub task_logs_dir: PathBuf,
    pub task_artifacts_dir: PathBuf,
    pub runtime_agent_dir: PathBuf,
    pub process_lock: PathBuf,
}

impl AppPaths {
    pub fn from_data_dir(data_dir: impl AsRef<Path>) -> FormalResult<Self> {
        let data_dir = data_dir.as_ref();
        if data_dir.as_os_str().is_empty() {
            return Err(FormalError::InvalidConfig("工作台数据目录不能为空".into()));
        }
        let data_dir = data_dir.to_path_buf();
        Ok(Self {
            local_db: data_dir.join("local.db"),
            known_hosts: data_dir.join("known_hosts"),
            logs_dir: data_dir.join("logs"),
            projects_dir: data_dir.join("projects"),
            task_logs_dir: data_dir.join("task-logs"),
            task_artifacts_dir: data_dir.join("task-artifacts"),
            runtime_agent_dir: data_dir.join("runtime/agent"),
            process_lock: data_dir.join(".inxaiot-desk-buddy.lock"),
            data_dir,
        })
    }

    pub fn ensure(&self) -> FormalResult<()> {
        for directory in [
            &self.data_dir,
            &self.logs_dir,
            &self.projects_dir,
            &self.task_logs_dir,
            &self.task_artifacts_dir,
            &self.runtime_agent_dir,
        ] {
            std::fs::create_dir_all(directory).map_err(|error| {
                tracing::error!(path = %directory.display(), error = ?error, "create app directory failed");
                FormalError::LocalIo("创建工作台目录")
            })?;
        }
        Ok(())
    }

    pub fn project_task_dir(&self, project_id: &str, task_id: &str) -> FormalResult<PathBuf> {
        validate_path_segment(project_id)?;
        validate_path_segment(task_id)?;
        Ok(self.task_artifacts_dir.join(project_id).join(task_id))
    }

    pub fn project_task_log_path(&self, project_id: &str, task_id: &str) -> FormalResult<PathBuf> {
        validate_path_segment(project_id)?;
        validate_path_segment(task_id)?;
        Ok(self
            .task_logs_dir
            .join(project_id)
            .join(format!("{task_id}.jsonl")))
    }
}

fn validate_path_segment(value: &str) -> FormalResult<()> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        return Err(FormalError::InvalidConfig("路径标识符无效".into()));
    }
    Ok(())
}
