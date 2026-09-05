use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink,
};
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::service_check::ServiceCheckReport;
use crate::infrastructure::local_sqlite::task_repository::{
    TargetUpdate, TaskRepository, TaskStepWrite,
};
use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;

pub const DEPLOYMENT_REMOTE_PROGRESS_START: u64 = 10;
pub const DEPLOYMENT_PROGRESS_TOTAL: u64 = 100;

enum ProgressMessage {
    Event(DeploymentProgressEvent),
    Shutdown,
}

pub struct TaskProgressReporter {
    sender: mpsc::UnboundedSender<ProgressMessage>,
    fatal: Arc<Mutex<Option<String>>>,
    progress_range: Option<ProgressRange>,
    service_checks: Mutex<BTreeMap<String, ServiceCheckReport>>,
}

#[derive(Clone, Copy)]
struct ProgressRange {
    start: u64,
    end: u64,
    total: u64,
}

impl DeploymentProgressSink for TaskProgressReporter {
    fn service_check(&self, mac: &str, report: ServiceCheckReport) -> AppResult<()> {
        report.validate()?;
        self.service_checks
            .lock()
            .map_err(|_| AppError::Conflict("服务检查结果锁已损坏".into()))?
            .insert(mac.into(), report);
        Ok(())
    }

    fn emit(&self, mut event: DeploymentProgressEvent) -> AppResult<()> {
        if event.mac.trim().is_empty()
            || event.stage.trim().is_empty()
            || event.message_code.trim().is_empty()
            || (event.progress_total > 0 && event.progress_current > event.progress_total)
        {
            return Err(AppError::InvalidConfig("部署进度事件参数无效".into()));
        }
        if let Some(range) = self.progress_range
            && event.progress_total > 0
        {
            let span = range.end.saturating_sub(range.start);
            event.progress_current = range
                .start
                .saturating_add(span.saturating_mul(event.progress_current) / event.progress_total)
                .min(range.end);
            event.progress_total = range.total;
        }
        if let Some(error) = self
            .fatal
            .lock()
            .map_err(|_| AppError::Conflict("部署进度错误锁已损坏".into()))?
            .clone()
        {
            return Err(AppError::Conflict(format!("部署进度持久化已失败：{error}")));
        }
        self.sender
            .send(ProgressMessage::Event(event))
            .map_err(|_| AppError::Conflict("部署进度管线已关闭".into()))
    }
}

impl TaskProgressReporter {
    pub fn service_checks(&self) -> AppResult<BTreeMap<String, ServiceCheckReport>> {
        self.service_checks
            .lock()
            .map(|reports| reports.clone())
            .map_err(|_| AppError::Conflict("服务检查结果锁已损坏".into()))
    }
}

pub struct TaskProgressGuard {
    pub sink: Arc<TaskProgressReporter>,
    join: JoinHandle<()>,
    fatal: Arc<Mutex<Option<String>>>,
}

impl TaskProgressGuard {
    pub fn start(
        task_id: String,
        repository: TaskRepository,
        pipeline: TaskEventPipeline,
        secret_values: Vec<String>,
    ) -> Self {
        Self::start_with_range(task_id, repository, pipeline, secret_values, None)
    }

    pub fn start_scaled(
        task_id: String,
        repository: TaskRepository,
        pipeline: TaskEventPipeline,
        secret_values: Vec<String>,
        start: u64,
        end: u64,
        total: u64,
    ) -> AppResult<Self> {
        if total == 0 || start > end || end > total {
            return Err(AppError::InvalidConfig("部署进度映射范围无效".into()));
        }
        Ok(Self::start_with_range(
            task_id,
            repository,
            pipeline,
            secret_values,
            Some(ProgressRange { start, end, total }),
        ))
    }

    fn start_with_range(
        task_id: String,
        repository: TaskRepository,
        pipeline: TaskEventPipeline,
        secret_values: Vec<String>,
        progress_range: Option<ProgressRange>,
    ) -> Self {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let fatal = Arc::new(Mutex::new(None));
        let worker_fatal = fatal.clone();
        let pipeline = pipeline.with_additional_secrets(secret_values);
        let join = tokio::spawn(async move {
            while let Some(message) = receiver.recv().await {
                match message {
                    ProgressMessage::Event(event) => {
                        if let Err(error) =
                            persist_event(&task_id, &repository, &pipeline, event).await
                        {
                            if let Ok(mut fatal) = worker_fatal.lock() {
                                *fatal = Some(error.to_string());
                            }
                            break;
                        }
                    }
                    ProgressMessage::Shutdown => break,
                }
            }
        });
        Self {
            sink: Arc::new(TaskProgressReporter {
                sender,
                fatal: fatal.clone(),
                progress_range,
                service_checks: Mutex::new(BTreeMap::new()),
            }),
            join,
            fatal,
        }
    }

    pub async fn stop(self) -> AppResult<()> {
        let _ = self.sink.sender.send(ProgressMessage::Shutdown);
        self.join
            .await
            .map_err(|_| AppError::Conflict("部署进度工作线程异常结束".into()))?;
        match self
            .fatal
            .lock()
            .map_err(|_| AppError::Conflict("部署进度错误锁已损坏".into()))?
            .clone()
        {
            Some(error) => Err(AppError::Conflict(format!("部署进度持久化失败：{error}"))),
            None => Ok(()),
        }
    }
}

async fn persist_event(
    task_id: &str,
    repository: &TaskRepository,
    pipeline: &TaskEventPipeline,
    event: DeploymentProgressEvent,
) -> AppResult<()> {
    let mut event = event;
    let transient_progress = matches!(
        event.message_code.as_str(),
        "SFTP_UPLOAD_PROGRESS" | "LOCAL_ARTIFACT_PROGRESS" | "LOCAL_ARCHIVE_PROGRESS"
    );
    event.message_code = pipeline.redact_text(&event.message_code);
    event.message = event
        .message
        .take()
        .map(|message| pipeline.redact_text(&message));
    repository
        .update_target(
            task_id,
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: event.mac.clone(),
                state: event.target_state,
                stage: event.stage.clone(),
                progress_current: event.progress_current,
                progress_total: event.progress_total,
                fencing_token: None,
                message_code: Some(event.message_code.clone()),
                message_params_json: None,
            },
        )
        .await?;
    if !transient_progress
        && let (Some(step_code), Some(step_state)) = (event.step_code.as_deref(), event.step_state)
    {
        repository
            .save_step(
                task_id,
                TaskStepWrite {
                    id: format!("{task_id}:{}:{step_code}", event.mac),
                    resource_type: Some("aio".into()),
                    resource_key: Some(event.mac.clone()),
                    step_code: step_code.into(),
                    state: step_state,
                    error_code: matches!(
                        step_state,
                        crate::domain::common::task::StepState::Failed
                            | crate::domain::common::task::StepState::Interrupted
                    )
                    .then_some(event.message_code.clone()),
                    message: event.message.clone(),
                },
            )
            .await?;
    }
    let input = TaskEventInput {
        resource_type: Some("aio".into()),
        resource_key: Some(event.mac),
        stage: event.stage,
        status: event
            .step_state
            .map(|state| state.as_str())
            .unwrap_or_else(|| event.target_state.as_str())
            .into(),
        progress_current: Some(event.progress_current),
        progress_total: Some(event.progress_total),
        level: event.level,
        message_code: event.message_code,
        message_params: BTreeMap::new(),
        message: event.message,
    };
    if transient_progress {
        pipeline.emit_transient(task_id, input).await?;
    } else {
        pipeline.emit(task_id, input).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ProgressMessage, ProgressRange, TaskProgressGuard, TaskProgressReporter};
    use crate::application::ports::deployment_progress::{
        DeploymentProgressEvent, DeploymentProgressSink,
    };
    use crate::domain::common::task::{StepState, TargetState, TaskEventLevel, TaskState};
    use crate::formal::local_store::LocalStore;
    use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TaskRepository};
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;

    #[test]
    fn reporter_rejects_invalid_progress_before_queueing() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let reporter = TaskProgressReporter {
            sender,
            fatal: Default::default(),
            progress_range: None,
            service_checks: Default::default(),
        };
        let error = reporter
            .emit(DeploymentProgressEvent {
                mac: "001122334455".into(),
                stage: "upload".into(),
                step_code: None,
                step_state: None,
                target_state: TargetState::Running,
                progress_current: 101,
                progress_total: 100,
                level: TaskEventLevel::Info,
                message_code: "UPLOAD_PROGRESS".into(),
                message: None,
            })
            .expect_err("progress over total must fail");
        assert!(error.to_string().contains("参数无效"));
    }

    #[test]
    fn reporter_maps_remote_progress_into_reserved_range() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let reporter = TaskProgressReporter {
            sender,
            fatal: Default::default(),
            progress_range: Some(ProgressRange {
                start: 10,
                end: 100,
                total: 100,
            }),
            service_checks: Default::default(),
        };
        reporter
            .emit(DeploymentProgressEvent {
                mac: "001122334455".into(),
                stage: "upload".into(),
                step_code: None,
                step_state: None,
                target_state: TargetState::Running,
                progress_current: 50,
                progress_total: 100,
                level: TaskEventLevel::Info,
                message_code: "UPLOAD_PROGRESS".into(),
                message: None,
            })
            .expect("mapped progress");
        let ProgressMessage::Event(event) = receiver.try_recv().expect("queued event") else {
            panic!("expected progress event");
        };
        assert_eq!(event.progress_current, 55);
        assert_eq!(event.progress_total, 100);
    }

    #[tokio::test]
    async fn reporter_persists_target_step_event_and_redacts_dynamic_secret() {
        let directory = tempfile::tempdir().expect("tempdir");
        let store = LocalStore::open(&directory.path().join("local.db"))
            .await
            .expect("store");
        let repository = TaskRepository::new(store.pool().clone());
        let log_path = directory.path().join("events.jsonl");
        sqlx::query(concat!(
            "INSERT INTO local_project ",
            "(id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, ",
            "db_password_secret_ref, created_at, updated_at) VALUES ",
            "('project-progress', 'project-progress', 'http://platform.test', 'db.test', 3306, ",
            "'user', 'business', 'workbench', 'secret-ref', '1', '1')"
        ))
        .execute(store.pool())
        .await
        .expect("project fixture");
        repository
            .create(CreateTask {
                id: "task-progress".into(),
                local_project_id: "project-progress".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "service_upgrade".into(),
                name: "progress".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: log_path.to_string_lossy().into_owned(),
                targets: vec![("aio".into(), "001122334455".into())],
            })
            .await
            .expect("task");
        let mut state = TaskState::Draft;
        for next in [
            TaskState::Checking,
            TaskState::Ready,
            TaskState::Queued,
            TaskState::Running,
        ] {
            repository
                .transition("task-progress", state, next, None, None)
                .await
                .expect("transition");
            state = next;
        }
        let event_bus = TaskEventBus::new(16).expect("bus");
        let mut events = event_bus.subscribe();
        let pipeline = TaskEventPipeline::new(
            repository.clone(),
            event_bus,
            SensitiveValueRedactor::default(),
        );
        let guard = TaskProgressGuard::start(
            "task-progress".into(),
            repository.clone(),
            pipeline,
            vec!["stage75b-secret".into()],
        );
        guard
            .sink
            .emit(DeploymentProgressEvent {
                mac: "001122334455".into(),
                stage: "upload".into(),
                step_code: Some("upload".into()),
                step_state: Some(StepState::Running),
                target_state: TargetState::Running,
                progress_current: 42,
                progress_total: 100,
                level: TaskEventLevel::Info,
                message_code: "SFTP_UPLOAD_PROGRESS".into(),
                message: Some("output=stage75b-secret".into()),
            })
            .expect("emit");
        guard.stop().await.expect("stop");

        let target = repository
            .targets("task-progress")
            .await
            .expect("targets")
            .remove(0);
        assert_eq!(target.stage, "upload");
        assert_eq!(target.progress_current, 42);
        assert!(
            repository
                .steps("task-progress")
                .await
                .expect("steps")
                .is_empty()
        );
        let event = events.recv().await.expect("transient progress event");
        assert_eq!(event.message_code, "SFTP_UPLOAD_PROGRESS");
        assert_eq!(event.message.as_deref(), Some("output=[REDACTED]"));
        assert!(!log_path.exists());
        store.close().await;
    }
}
