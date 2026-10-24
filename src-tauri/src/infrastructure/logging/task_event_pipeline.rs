use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use time::OffsetDateTime;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::application::ports::task_log::TaskLogStore;
use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskEvent;
use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
use crate::infrastructure::logging::task_log::JsonlTaskLogStore;
use crate::runtime::event_bus::TaskEventBus;

#[derive(Clone)]
pub struct TaskEventPipeline {
    repository: TaskRepository,
    log_store: JsonlTaskLogStore,
    event_bus: TaskEventBus,
    redactor: SensitiveValueRedactor,
    task_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>,
}

impl TaskEventPipeline {
    pub fn new(
        repository: TaskRepository,
        event_bus: TaskEventBus,
        redactor: SensitiveValueRedactor,
    ) -> Self {
        Self {
            repository,
            log_store: JsonlTaskLogStore,
            event_bus,
            redactor,
            task_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn event_bus(&self) -> &TaskEventBus {
        &self.event_bus
    }

    pub fn log_store(&self) -> &JsonlTaskLogStore {
        &self.log_store
    }

    pub fn with_additional_secrets(&self, values: impl IntoIterator<Item = String>) -> Self {
        Self {
            repository: self.repository.clone(),
            log_store: self.log_store.clone(),
            event_bus: self.event_bus.clone(),
            redactor: self.redactor.with_additional_values(values),
            task_locks: self.task_locks.clone(),
        }
    }

    pub fn register_secrets(&self, values: impl IntoIterator<Item = String>) -> AppResult<()> {
        self.redactor.register_values(values)
    }

    pub fn redactor(&self) -> &SensitiveValueRedactor {
        &self.redactor
    }

    pub fn redact_text(&self, value: &str) -> String {
        self.redactor.redact_text(value)
    }

    pub async fn emit_transient(
        &self,
        task_id: &str,
        input: TaskEventInput,
    ) -> AppResult<TaskEvent> {
        self.emit_inner(task_id, input, false, None).await
    }

    /// 取消信号已经生效时，仅限制进入事件锁的等待，避免取消接口被日志锁拖住。
    /// 取得锁后的序号和 JSONL 写入必须完整结束，不能超时丢弃半条日志。
    pub async fn emit_with_lock_timeout(
        &self,
        task_id: &str,
        input: TaskEventInput,
        lock_wait: Duration,
    ) -> AppResult<TaskEvent> {
        self.emit_inner(task_id, input, true, Some(lock_wait)).await
    }

    fn task_lock(&self, task_id: &str) -> AppResult<Arc<AsyncMutex<()>>> {
        let mut locks = self
            .task_locks
            .lock()
            .map_err(|_| AppError::Conflict("任务事件锁已损坏".into()))?;
        Ok(locks
            .entry(task_id.into())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone())
    }

    #[cfg(test)]
    pub(crate) async fn hold_task_lock_for_test(
        &self,
        task_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        self.task_lock(task_id).expect("test task lock").lock_owned().await
    }

    async fn emit_inner(
        &self,
        task_id: &str,
        input: TaskEventInput,
        persist_to_log: bool,
        lock_wait: Option<Duration>,
    ) -> AppResult<TaskEvent> {
        input.validate()?;
        let task_lock = self.task_lock(task_id)?;
        let _guard = match lock_wait {
            Some(limit) => tokio::time::timeout(limit, task_lock.lock())
                .await
                .map_err(|_| AppError::timeout("等待任务事件日志锁"))?,
            None => task_lock.lock().await,
        };
        let task = self.repository.get(task_id).await?;
        let sequence = self.repository.reserve_sequence(task_id).await?;
        let mut event = TaskEvent {
            event_id: Uuid::now_v7().to_string(),
            local_task_id: task.id,
            operation_record_id: task.remote_operation_record_id,
            sequence,
            local_project_id: task.local_project_id,
            domain_type: task.domain_type,
            resource_type: input.resource_type,
            resource_key: input.resource_key,
            stage: input.stage,
            status: input.status,
            progress_current: input.progress_current,
            progress_total: input.progress_total,
            level: input.level,
            message_code: input.message_code,
            message_params: input.message_params,
            message: input.message,
            timestamp: OffsetDateTime::now_utc().unix_timestamp_nanos().to_string(),
        };
        self.redactor.redact_event(&mut event);
        if persist_to_log {
            self.log_store
                .append(Path::new(&task.log_path), &event)
                .await?;
        }
        self.event_bus.publish(event.clone());
        Ok(event)
    }
}

impl TaskEventSink for TaskEventPipeline {
    async fn emit(&self, task_id: &str, input: TaskEventInput) -> AppResult<TaskEvent> {
        self.emit_inner(task_id, input, true, None).await
    }
}

#[cfg(test)]
#[path = "task_event_pipeline_tests.rs"]
mod tests;
