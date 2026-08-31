use std::path::Path;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::application::ports::task_log::{TaskLogPage, TaskLogQuery, TaskLogStore};
use crate::core::error::{AppError, AppResult};
use crate::domain::common::task::TaskEvent;

const MAX_TASK_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TASK_LOG_LINE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Default)]
pub struct JsonlTaskLogStore;

impl TaskLogStore for JsonlTaskLogStore {
    async fn append(&self, path: &Path, event: &TaskEvent) -> AppResult<()> {
        validate_path(path)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|error| AppError::io("创建任务日志目录", &error))?;
        }
        let mut line = serde_json::to_vec(event)
            .map_err(|_| AppError::InvalidConfig("任务事件无法序列化".into()))?;
        line.push(b'\n');
        if line.len() > MAX_TASK_LOG_LINE_BYTES {
            return Err(AppError::Conflict("单条任务日志超过64KiB安全上限".into()));
        }
        let existing = tokio::fs::metadata(path)
            .await
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        if existing.saturating_add(line.len() as u64) > MAX_TASK_LOG_BYTES {
            return Err(AppError::Conflict(
                "任务日志超过32MiB安全上限，已停止继续写入".into(),
            ));
        }
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await
            .map_err(|error| AppError::io("打开任务JSONL日志", &error))?;
        file.write_all(&line)
            .await
            .map_err(|error| AppError::io("写入任务JSONL日志", &error))?;
        file.flush()
            .await
            .map_err(|error| AppError::io("刷新任务JSONL日志", &error))
    }

    async fn read_page(&self, path: &Path, query: &TaskLogQuery) -> AppResult<TaskLogPage> {
        validate_path(path)?;
        if query.limit == 0 || query.limit > 1000 {
            return Err(AppError::InvalidConfig("任务日志分页大小无效".into()));
        }
        if !path.exists() {
            return Ok(TaskLogPage {
                items: Vec::new(),
                next_offset: query.offset,
                has_more: false,
            });
        }
        if query.newest_first {
            return read_newest_page(path, query).await;
        }
        let file = tokio::fs::File::open(path)
            .await
            .map_err(|error| AppError::io("打开任务日志读取", &error))?;
        let mut lines = BufReader::new(file).lines();
        let keyword = query.keyword.as_deref().map(str::to_lowercase);
        let mut matched = 0_u64;
        let mut items = Vec::with_capacity(query.limit as usize + 1);
        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|error| AppError::io("读取任务日志行", &error))?
        {
            let Ok(event) = serde_json::from_str::<TaskEvent>(&line) else {
                tracing::warn!(path = %path.display(), "skip malformed task log line");
                continue;
            };
            if !matches_event(&event, query, keyword.as_deref()) {
                continue;
            }
            if matched < query.offset {
                matched += 1;
                continue;
            }
            items.push(event);
            matched += 1;
            if items.len() > query.limit as usize {
                break;
            }
        }
        let has_more = items.len() > query.limit as usize;
        if has_more {
            items.pop();
        }
        Ok(TaskLogPage {
            next_offset: query.offset + items.len() as u64,
            items,
            has_more,
        })
    }
}

async fn read_newest_page(path: &Path, query: &TaskLogQuery) -> AppResult<TaskLogPage> {
    let keyword = query.keyword.as_deref().map(str::to_lowercase);
    let total = count_matching(path, query, keyword.as_deref()).await?;
    let end = total.saturating_sub(query.offset);
    let start = end.saturating_sub(u64::from(query.limit));
    if end == 0 {
        return Ok(TaskLogPage {
            items: Vec::new(),
            next_offset: query.offset,
            has_more: false,
        });
    }
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| AppError::io("打开任务日志读取", &error))?;
    let mut lines = BufReader::new(file).lines();
    let mut matched = 0_u64;
    let mut items = Vec::with_capacity(query.limit as usize);
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| AppError::io("读取任务日志行", &error))?
    {
        let Ok(event) = serde_json::from_str::<TaskEvent>(&line) else {
            continue;
        };
        if !matches_event(&event, query, keyword.as_deref()) {
            continue;
        }
        if matched >= start && matched < end {
            items.push(event);
        }
        matched += 1;
        if matched >= end {
            break;
        }
    }
    Ok(TaskLogPage {
        next_offset: query.offset + items.len() as u64,
        items,
        has_more: start > 0,
    })
}

async fn count_matching(
    path: &Path,
    query: &TaskLogQuery,
    keyword: Option<&str>,
) -> AppResult<u64> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|error| AppError::io("打开任务日志计数", &error))?;
    let mut lines = BufReader::new(file).lines();
    let mut count = 0_u64;
    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|error| AppError::io("读取任务日志计数行", &error))?
    {
        let Ok(event) = serde_json::from_str::<TaskEvent>(&line) else {
            continue;
        };
        if matches_event(&event, query, keyword) {
            count += 1;
        }
    }
    Ok(count)
}

fn matches_event(event: &TaskEvent, query: &TaskLogQuery, keyword: Option<&str>) -> bool {
    (query.levels.is_empty() || query.levels.contains(&event.level))
        && keyword.is_none_or(|keyword| {
            event.message_code.to_lowercase().contains(keyword)
                || event
                    .message
                    .as_deref()
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains(keyword)
                || event
                    .message_params
                    .values()
                    .any(|value| value.to_lowercase().contains(keyword))
        })
}

fn validate_path(path: &Path) -> AppResult<()> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        return Err(AppError::InvalidConfig("任务日志路径必须是绝对路径".into()));
    }
    Ok(())
}
