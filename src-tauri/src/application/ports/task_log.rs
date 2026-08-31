use std::path::Path;

use crate::core::error::AppResult;
use crate::domain::common::task::{TaskEvent, TaskEventLevel};

#[derive(Clone, Debug, Default)]
pub struct TaskLogQuery {
    pub levels: Vec<TaskEventLevel>,
    pub keyword: Option<String>,
    pub offset: u64,
    pub limit: u32,
    pub newest_first: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskLogPage {
    pub items: Vec<TaskEvent>,
    pub next_offset: u64,
    pub has_more: bool,
}

#[allow(async_fn_in_trait)]
pub trait TaskLogStore: Send + Sync {
    async fn append(&self, path: &Path, event: &TaskEvent) -> AppResult<()>;
    async fn read_page(&self, path: &Path, query: &TaskLogQuery) -> AppResult<TaskLogPage>;
}
