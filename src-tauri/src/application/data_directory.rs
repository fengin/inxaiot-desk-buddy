use serde::{Deserialize, Serialize};

use crate::application::ports::data_directory::DataDirectoryPort;
use crate::core::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataDirectorySwitchMode {
    Empty,
    Migrate,
    UseExisting,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataDirectorySwitchRequest {
    pub target_directory: String,
    pub mode: DataDirectorySwitchMode,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DataDirectoryStatus {
    pub active_directory: String,
    pub default_directory: String,
    pub pending_directory: Option<String>,
    pub previous_directory: Option<String>,
    pub pending_mode: Option<DataDirectorySwitchMode>,
    pub restart_required: bool,
    pub first_setup: bool,
    pub last_switch_error: Option<String>,
}

pub fn get_data_directory_status<P: DataDirectoryPort>(port: &P) -> AppResult<DataDirectoryStatus> {
    port.status()
}

pub fn schedule_data_directory_switch<P: DataDirectoryPort>(
    port: &P,
    active_task_count: u32,
    request: DataDirectorySwitchRequest,
) -> AppResult<DataDirectoryStatus> {
    if active_task_count > 0 {
        return Err(AppError::Conflict(format!(
            "当前有{active_task_count}个活动任务，不能切换数据目录"
        )));
    }
    if request.mode == DataDirectorySwitchMode::UseExisting {
        return Err(AppError::InvalidConfig(
            "已有目录只能通过明确的回滚入口启用".into(),
        ));
    }
    port.schedule_switch(request)
}

pub fn schedule_data_directory_rollback<P: DataDirectoryPort>(
    port: &P,
    active_task_count: u32,
) -> AppResult<DataDirectoryStatus> {
    if active_task_count > 0 {
        return Err(AppError::Conflict(format!(
            "当前有{active_task_count}个活动任务，不能回滚数据目录"
        )));
    }
    port.schedule_rollback()
}
