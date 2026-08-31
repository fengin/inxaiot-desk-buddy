use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::application::ports::task_log::{TaskLogQuery, TaskLogStore};
use crate::domain::common::task::{TargetState, TaskEvent, TaskEventLevel, TaskRecord, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::interface::error::CommandErrorDto;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityTaskDto {
    pub id: String,
    pub project_id: String,
    pub domain_type: String,
    pub operation_type: String,
    pub name: String,
    pub state: String,
    pub stage: String,
    pub progress: Option<u32>,
    pub target_count: u32,
    pub completed_count: u32,
    pub updated_at: String,
    pub cancellable: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLogDto {
    pub id: String,
    pub task_id: String,
    pub sequence: u64,
    pub timestamp: String,
    pub level: String,
    pub source: String,
    pub message: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityLogPageDto {
    pub items: Vec<ActivityLogDto>,
    pub next_offset: u64,
    pub has_more: bool,
}

#[tauri::command]
pub async fn list_local_tasks(
    state: State<'_, FormalAppState>,
    local_project_id: String,
    limit: Option<u32>,
) -> Result<Vec<ActivityTaskDto>, CommandErrorDto> {
    query_activity_tasks(&state, &local_project_id, limit.unwrap_or(100)).await
}

pub async fn query_activity_tasks(
    state: &FormalAppState,
    local_project_id: &str,
    limit: u32,
) -> Result<Vec<ActivityTaskDto>, CommandErrorDto> {
    let tasks = state
        .task_repository
        .list_recent(local_project_id, limit)
        .await
        .map_err(CommandErrorDto::from)?;
    let mut result = Vec::with_capacity(tasks.len());
    for task in tasks {
        result.push(activity_task(state, task).await?);
    }
    Ok(result)
}

#[tauri::command]
pub async fn list_task_logs(
    state: State<'_, FormalAppState>,
    task_id: String,
    levels: Vec<String>,
    keyword: Option<String>,
    offset: u64,
    limit: u32,
    newest_first: bool,
) -> Result<ActivityLogPageDto, CommandErrorDto> {
    query_task_logs(
        &state,
        &task_id,
        &levels,
        keyword,
        offset,
        limit,
        newest_first,
    )
    .await
}

pub async fn query_task_logs(
    state: &FormalAppState,
    task_id: &str,
    levels: &[String],
    keyword: Option<String>,
    offset: u64,
    limit: u32,
    newest_first: bool,
) -> Result<ActivityLogPageDto, CommandErrorDto> {
    let task = state
        .task_repository
        .get(task_id)
        .await
        .map_err(CommandErrorDto::from)?;
    let levels = levels
        .iter()
        .map(|value| parse_level(value))
        .collect::<Result<Vec<_>, _>>()?;
    let page = state
        .task_event_pipeline
        .log_store()
        .read_page(
            Path::new(&task.log_path),
            &TaskLogQuery {
                levels,
                keyword,
                offset,
                limit,
                newest_first,
            },
        )
        .await
        .map_err(CommandErrorDto::from)?;
    Ok(ActivityLogPageDto {
        items: page.items.into_iter().map(activity_log).collect(),
        next_offset: page.next_offset,
        has_more: page.has_more,
    })
}

#[tauri::command]
pub async fn cancel_local_task(
    state: State<'_, FormalAppState>,
    task_id: String,
) -> Result<ActivityTaskDto, CommandErrorDto> {
    request_task_cancel(&state, &task_id).await
}

pub async fn request_task_cancel(
    state: &FormalAppState,
    task_id: &str,
) -> Result<ActivityTaskDto, CommandErrorDto> {
    let task = state
        .task_repository
        .get(task_id)
        .await
        .map_err(CommandErrorDto::from)?;
    let (task, stage, status, message_code, message) = match task.state {
        TaskState::Queued => {
            state
                .task_queue
                .cancel(task_id)
                .await
                .map_err(CommandErrorDto::from)?;
            let task = state
                .task_repository
                .transition(
                    task_id,
                    TaskState::Queued,
                    TaskState::Cancelled,
                    Some("QUEUE_CANCELLED"),
                    Some("用户取消了排队任务，未执行远端步骤"),
                )
                .await
                .map_err(CommandErrorDto::from)?;
            if let Ok(targets) = state.task_repository.targets(task_id).await {
                for target in targets {
                    if target.state == TargetState::Pending {
                        let _ = state
                            .task_repository
                            .update_target(
                                task_id,
                                TargetUpdate {
                                    resource_type: target.resource_type,
                                    resource_key: target.resource_key,
                                    state: TargetState::Cancelled,
                                    stage: "cancelled".into(),
                                    progress_current: 100,
                                    progress_total: 100,
                                    fencing_token: target.fencing_token,
                                    message_code: Some("QUEUE_CANCELLED".into()),
                                    message_params_json: None,
                                },
                            )
                            .await;
                    }
                }
            }
            (
                task,
                "cancelled",
                "cancelled",
                "QUEUE_CANCELLED",
                "用户取消了排队任务，未执行远端步骤",
            )
        }
        TaskState::Running => {
            let task = state
                .task_repository
                .transition(
                    task_id,
                    TaskState::Running,
                    TaskState::Cancelling,
                    None,
                    Some("正在等待当前安全步骤结束"),
                )
                .await
                .map_err(CommandErrorDto::from)?;
            if let Err(error) = state.task_queue.cancel(task_id).await {
                let _ = state
                    .task_repository
                    .transition(
                        task_id,
                        TaskState::Cancelling,
                        TaskState::Interrupted,
                        Some("CANCELLATION_SIGNAL_FAILED"),
                        Some(&error.to_string()),
                    )
                    .await;
                return Err(CommandErrorDto::from(error));
            }
            (
                task,
                "cancelling",
                "cancelling",
                "TASK_CANCELLING",
                "用户请求取消，等待当前安全步骤结束",
            )
        }
        _ => {
            return Err(CommandErrorDto::from(
                crate::core::error::AppError::Conflict(format!(
                    "当前任务状态不允许取消：{}",
                    task.state.as_str()
                )),
            ));
        }
    };
    if let Err(error) = state
        .task_event_pipeline
        .emit(
            task_id,
            TaskEventInput {
                resource_type: None,
                resource_key: None,
                stage: stage.into(),
                status: status.into(),
                progress_current: None,
                progress_total: None,
                level: TaskEventLevel::Warn,
                message_code: message_code.into(),
                message_params: BTreeMap::new(),
                message: Some(message.into()),
            },
        )
        .await
    {
        tracing::error!(task_id, error = ?crate::core::log_safety::safe_error(&error), "persist cancellation event failed");
    }
    if task.state.is_terminal() {
        let _ = TaskDataLifecycle::new(&state.paths).finalize_task(
            &task.local_project_id,
            &task.id,
            task.state,
        );
    }
    activity_task(state, task).await
}

async fn activity_task(
    state: &FormalAppState,
    task: TaskRecord,
) -> Result<ActivityTaskDto, CommandErrorDto> {
    let targets = state
        .task_repository
        .targets(&task.id)
        .await
        .map_err(CommandErrorDto::from)?;
    let target_count = u32::try_from(targets.len()).unwrap_or(u32::MAX);
    let completed_count = u32::try_from(
        targets
            .iter()
            .filter(|target| {
                matches!(
                    target.state,
                    TargetState::Succeeded
                        | TargetState::Failed
                        | TargetState::Cancelled
                        | TargetState::Interrupted
                        | TargetState::Unknown
                )
            })
            .count(),
    )
    .unwrap_or(u32::MAX);
    let progress_current = targets
        .iter()
        .map(|target| target.progress_current)
        .sum::<u64>();
    let progress_total = targets
        .iter()
        .map(|target| target.progress_total)
        .sum::<u64>();
    let progress = (progress_total > 0).then(|| {
        u32::try_from(progress_current.saturating_mul(100) / progress_total)
            .unwrap_or(100)
            .min(100)
    });
    let stage = targets
        .iter()
        .find(|target| target.state == TargetState::Running && !target.stage.is_empty())
        .or_else(|| targets.iter().find(|target| !target.stage.is_empty()))
        .map(|target| target.stage.clone())
        .unwrap_or_else(|| task.state.as_str().into());
    Ok(ActivityTaskDto {
        id: task.id,
        project_id: task.local_project_id,
        domain_type: task.domain_type,
        operation_type: task.operation_type,
        name: task.name,
        state: task.state.as_str().into(),
        stage,
        progress,
        target_count,
        completed_count,
        updated_at: task.updated_at,
        cancellable: matches!(task.state, TaskState::Queued | TaskState::Running),
    })
}

fn activity_log(event: TaskEvent) -> ActivityLogDto {
    let source = event
        .resource_key
        .clone()
        .or(event.resource_type.clone())
        .unwrap_or_else(|| event.stage.clone());
    let message = event.message.clone().unwrap_or_else(|| {
        if event.message_params.is_empty() {
            event.message_code.clone()
        } else {
            format!(
                "{} · {}",
                event.message_code,
                event
                    .message_params
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    });
    ActivityLogDto {
        id: event.event_id,
        task_id: event.local_task_id,
        sequence: event.sequence,
        timestamp: event.timestamp,
        level: match event.level {
            TaskEventLevel::Info => "INFO",
            TaskEventLevel::Warn => "WARN",
            TaskEventLevel::Error => "ERROR",
        }
        .into(),
        source,
        message,
    }
}

fn parse_level(value: &str) -> Result<TaskEventLevel, CommandErrorDto> {
    match value.trim().to_ascii_uppercase().as_str() {
        "INFO" => Ok(TaskEventLevel::Info),
        "WARN" => Ok(TaskEventLevel::Warn),
        "ERROR" => Ok(TaskEventLevel::Error),
        _ => Err(CommandErrorDto::from(
            crate::core::error::AppError::InvalidConfig(format!("未知日志级别：{value}")),
        )),
    }
}
