use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::{TaskRecord, TaskRecoveryConflict};
use crate::formal::app_state::FormalAppState;

#[derive(Clone, Debug)]
pub enum TaskRecoveryOutcome {
    Completed,
    TakeoverRequired(Vec<TaskRecoveryConflict>),
}

pub type TaskRecoveryFuture<'a> =
    Pin<Box<dyn Future<Output = AppResult<TaskRecoveryOutcome>> + Send + 'a>>;
pub type TaskRecoveryHandler =
    for<'a> fn(&'a FormalAppState, &'a TaskRecord, bool) -> TaskRecoveryFuture<'a>;

#[derive(Clone, Default)]
pub struct TaskRecoveryRegistry {
    handlers: Arc<RwLock<HashMap<(String, String), TaskRecoveryHandler>>>,
}

impl TaskRecoveryRegistry {
    pub fn register(
        &self,
        domain: &str,
        operation: &str,
        handler: TaskRecoveryHandler,
    ) -> AppResult<()> {
        if domain.trim().is_empty() || operation.trim().is_empty() {
            return Err(AppError::InvalidConfig(
                "结果处理业务和操作类型不能为空".into(),
            ));
        }
        let mut handlers = self
            .handlers
            .write()
            .map_err(|_| AppError::Conflict("结果处理注册表不可用".into()))?;
        let key = (domain.to_string(), operation.to_string());
        if handlers.contains_key(&key) {
            return Err(AppError::Conflict(format!(
                "结果处理方法已注册：{domain}/{operation}"
            )));
        }
        handlers.insert(key, handler);
        Ok(())
    }

    pub async fn recover(
        &self,
        state: &FormalAppState,
        task: &TaskRecord,
        takeover: bool,
    ) -> AppResult<TaskRecoveryOutcome> {
        let handler = self
            .handlers
            .read()
            .map_err(|_| AppError::Conflict("结果处理注册表不可用".into()))?
            .get(&(task.domain_type.clone(), task.operation_type.clone()))
            .copied()
            .ok_or_else(|| {
                AppError::NotFound(format!(
                    "尚未支持此操作的结果处理：{}/{}",
                    task.domain_type, task.operation_type
                ))
            })?;
        handler(state, task, takeover).await
    }
}
