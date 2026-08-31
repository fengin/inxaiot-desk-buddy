use tokio::sync::broadcast;

use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskEvent;

#[derive(Clone, Debug)]
pub struct TaskEventBus {
    sender: broadcast::Sender<TaskEvent>,
}

impl TaskEventBus {
    pub fn new(capacity: usize) -> AppResult<Self> {
        if capacity == 0 {
            return Err(AppError::InvalidConfig("任务事件总线容量必须大于零".into()));
        }
        let (sender, _) = broadcast::channel(capacity);
        Ok(Self { sender })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: TaskEvent) -> usize {
        self.sender.send(event).unwrap_or_default()
    }
}
