use crate::application::data_directory::{DataDirectoryStatus, DataDirectorySwitchRequest};
use crate::core::error::AppResult;

pub trait DataDirectoryPort: Send + Sync {
    fn status(&self) -> AppResult<DataDirectoryStatus>;
    fn schedule_switch(
        &self,
        request: DataDirectorySwitchRequest,
    ) -> AppResult<DataDirectoryStatus>;
    fn schedule_rollback(&self) -> AppResult<DataDirectoryStatus>;
}
