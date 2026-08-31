use std::collections::BTreeMap;

use crate::core::error::AppResult;
use crate::domain::common::task::{TaskEvent, TaskEventLevel};

#[derive(Clone, Debug)]
pub struct TaskEventInput {
    pub resource_type: Option<String>,
    pub resource_key: Option<String>,
    pub stage: String,
    pub status: String,
    pub progress_current: Option<u64>,
    pub progress_total: Option<u64>,
    pub level: TaskEventLevel,
    pub message_code: String,
    pub message_params: BTreeMap<String, String>,
    pub message: Option<String>,
}

impl TaskEventInput {
    pub fn validate(&self) -> AppResult<()> {
        if self.stage.trim().is_empty()
            || self.status.trim().is_empty()
            || self.message_code.trim().is_empty()
            || self
                .progress_current
                .zip(self.progress_total)
                .is_some_and(|(current, total)| total > 0 && current > total)
        {
            return Err(crate::core::error::AppError::InvalidConfig(
                "任务事件参数无效".into(),
            ));
        }
        Ok(())
    }
}

#[allow(async_fn_in_trait)]
pub trait TaskEventSink: Send + Sync {
    async fn emit(&self, task_id: &str, input: TaskEventInput) -> AppResult<TaskEvent>;
}
