use std::collections::BTreeMap;
use std::sync::OnceLock;
use std::time::Duration;

use sqlx::MySqlPool;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::application::agent_protocol::{inspect_services_request, parse_service_check_report_at};
use crate::application::ports::remote_command::{NoopRemoteOutputSink, RemoteCommandExecutor};
use crate::application::ports::remote_session::{RemoteConnection, RemoteTarget};
use crate::application::ports::service_inspection::{
    ServiceInspectionPort, ServiceInspectionSubmission,
};
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::service_check::ServiceCheckReport;
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::formal::release_profile_repository::ReleaseProfileRepository;
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::infrastructure::aio_assets_service::{get_aio_node_detail, project_operator};
use crate::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use crate::infrastructure::local_sqlite::task_repository::{
    CreateTask, TargetUpdate, TaskStepWrite,
};
use crate::infrastructure::project_context::{map_formal_error, project_database};
use crate::infrastructure::release_remote_auth::release_remote_auth;
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::remote::observed::ObservedConnector;
use crate::infrastructure::service_check_repository::ServiceCheckRepository;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::runtime::task_queue::TaskEnvelope;

pub const SERVICE_INSPECTION_OPERATION: &str = "service_inspection";
const INSPECTION_WORK_TOTAL: u64 = 4;

pub struct ServiceInspectionService<'a> {
    state: &'a FormalAppState,
}

impl<'a> ServiceInspectionService<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl ServiceInspectionPort for ServiceInspectionService<'_> {
    async fn submit(&self, project_id: &str, mac: &str) -> AppResult<ServiceInspectionSubmission> {
        // 防止双击或多个窗口同时为同一台机器创建检查。
        static SUBMISSION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let _guard = SUBMISSION_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        project_operator(self.state, project_id).await?;
        let mac = MacAddress::parse(mac)?.normalized().to_string();
        let node = get_aio_node_detail(self.state, project_id, &mac)
            .await?
            .node;
        if !node.conflicts.is_empty() {
            return Err(AppError::Conflict(
                "请先解决一体机资产冲突，再检查服务".into(),
            ));
        }
        let active = self.state.task_repository.list_active().await?;
        for task in active
            .iter()
            .filter(|task| task.local_project_id == project_id)
        {
            if self
                .state
                .task_repository
                .targets(&task.id)
                .await?
                .iter()
                .any(|target| target.resource_type == "aio" && target.resource_key == mac)
            {
                if task.operation_type == SERVICE_INSPECTION_OPERATION {
                    return Ok(ServiceInspectionSubmission {
                        task_id: task.id.clone(),
                    });
                }
                return Err(AppError::Conflict(
                    "该一体机正在执行任务，请完成后再检查服务".into(),
                ));
            }
        }
        let pools = project_database(self.state, project_id).await?;
        ensure_node_idle(&pools.workbench, &mac).await?;
        let task_id = uuid::Uuid::now_v7().to_string();
        self.state
            .task_repository
            .create(CreateTask {
                id: task_id.clone(),
                local_project_id: project_id.into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: SERVICE_INSPECTION_OPERATION.into(),
                name: format!("检查服务 · {}", node.name),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: self
                    .state
                    .paths
                    .project_task_log_path(project_id, &task_id)
                    .map_err(map_formal_error)?
                    .to_string_lossy()
                    .into_owned(),
                targets: vec![("aio".into(), mac.clone())],
            })
            .await?;
        let queued = async {
            let mut previous = TaskState::Draft;
            for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
                self.state
                    .task_repository
                    .transition(&task_id, previous, next, None, None)
                    .await?;
                previous = next;
            }
            self.state
                .task_repository
                .initialize_target_progress(&task_id, "等待服务检查", INSPECTION_WORK_TOTAL)
                .await?;
            inspection_event(
                self.state,
                &task_id,
                &mac,
                "等待服务检查",
                0,
                TaskEventLevel::Info,
                "SERVICE_INSPECTION_QUEUED",
                "服务检查已进入本机任务队列",
            )
            .await?;
            self.state
                .task_queue
                .enqueue(TaskEnvelope {
                    local_task_id: task_id.clone(),
                    local_project_id: project_id.into(),
                    domain_type: "aio".into(),
                    operation_type: SERVICE_INSPECTION_OPERATION.into(),
                    resource_keys: vec![mac.clone()],
                    priority: 0,
                    payload_ref: None,
                    payload_sha256: None,
                })
                .await
        }
        .await;
        if let Err(error) = queued {
            finish_inspection(
                self.state,
                &task_id,
                &mac,
                TaskState::Failed,
                Some(&error.to_string()),
            )
            .await?;
            return Err(error);
        }
        Ok(ServiceInspectionSubmission { task_id })
    }
}

pub async fn execute_service_inspection(
    state: &FormalAppState,
    envelope: TaskEnvelope,
    cancellation: CancellationToken,
) -> AppResult<()> {
    let task = state.task_repository.get(&envelope.local_task_id).await?;
    if task.state.is_terminal() {
        return Ok(());
    }
    let mac = envelope.resource_keys.first().cloned().unwrap_or_default();
    let started_at = now();
    let result = inspect_node(state, &envelope, &cancellation, &started_at).await;
    match result {
        Ok(report) => {
            let issues = report
                .services
                .iter()
                .filter(|item| item.state != "normal")
                .count();
            let message = if issues == 0 {
                format!("服务检查完成，{}项正常", report.services.len())
            } else {
                format!("服务检查完成，{issues}项存在异常、版本偏差或尚未就绪，请查看一体机详情")
            };
            finish_inspection(
                state,
                &task.id,
                &mac,
                if issues == 0 {
                    TaskState::Succeeded
                } else {
                    TaskState::Failed
                },
                Some(&message),
            )
            .await?;
        }
        Err(error) => {
            let cancelled = cancellation.is_cancelled() || matches!(error, AppError::Cancelled);
            let message = state.task_event_pipeline.redact_text(&error.to_string());
            if !cancelled
                && let Ok(pools) = project_database(state, &envelope.local_project_id).await
            {
                // 部署中的暂态不可覆盖上次有效服务检查。
                if ensure_node_idle(&pools.workbench, &mac).await.is_ok() {
                    let report = ServiceCheckReport {
                        started_at,
                        checked_at: now(),
                        source: "manual".into(),
                        scope: "all".into(),
                        service_name: None,
                        expected_services: vec![],
                        services: vec![],
                        state: "failed".into(),
                        error: Some(message.clone()),
                    };
                    if let Err(save_error) = ServiceCheckRepository::new(
                        state.local_store.pool().clone(),
                        &envelope.local_project_id,
                    )
                    .save_report(&mac, &report)
                    .await
                    {
                        tracing::warn!(error = ?crate::core::log_safety::safe_error(&save_error), "persist failed service inspection failed");
                    }
                }
            }
            finish_inspection(
                state,
                &task.id,
                &mac,
                if cancelled {
                    TaskState::Cancelled
                } else {
                    TaskState::Failed
                },
                Some(&message),
            )
            .await?;
        }
    }
    let terminal = state.task_repository.get(&task.id).await?;
    TaskDataLifecycle::new(&state.paths).finalize_task(
        &task.local_project_id,
        &task.id,
        terminal.state,
    )?;
    Ok(())
}

async fn inspect_node(
    state: &FormalAppState,
    envelope: &TaskEnvelope,
    cancellation: &CancellationToken,
    started_at: &str,
) -> AppResult<ServiceCheckReport> {
    if cancellation.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let task_id = &envelope.local_task_id;
    let task = state.task_repository.get(task_id).await?;
    if envelope.resource_keys.len() != 1
        || task.local_project_id != envelope.local_project_id
        || task.operation_type != SERVICE_INSPECTION_OPERATION
        || envelope.payload_ref.is_some()
    {
        return Err(AppError::InvalidConfig("服务检查任务范围无效".into()));
    }
    let mac = MacAddress::parse(&envelope.resource_keys[0])?
        .normalized()
        .to_string();
    let targets = state.task_repository.targets(task_id).await?;
    if targets.len() != 1 || targets[0].resource_key != mac {
        return Err(AppError::Conflict("服务检查任务目标不一致".into()));
    }
    state
        .task_repository
        .transition(task_id, TaskState::Queued, TaskState::Running, None, None)
        .await?;
    inspection_event(
        state,
        task_id,
        &mac,
        "读取检查配置",
        0,
        TaskEventLevel::Info,
        "SERVICE_INSPECTION_STARTED",
        "正在读取一体机地址与SSH连接配置",
    )
    .await?;
    project_operator(state, &envelope.local_project_id).await?;
    let detail = get_aio_node_detail(state, &envelope.local_project_id, &mac).await?;
    let pools = project_database(state, &envelope.local_project_id).await?;
    ensure_node_idle(&pools.workbench, &mac).await?;
    let profile = ReleaseProfileRepository::new(pools.workbench.clone())
        .get("default")
        .await
        .map_err(map_formal_error)?;
    state.task_event_pipeline.register_secrets(
        [
            profile.credentials.ssh_password.clone(),
            profile.credentials.ssh_private_key.clone(),
            Some(profile.credentials.platform_auth_key.clone()),
            Some(profile.credentials.platform_mqtt_password.clone()),
            Some(profile.credentials.aio_mqtt_password.clone()),
        ]
        .into_iter()
        .flatten(),
    )?;
    let auth = release_remote_auth(&profile.credentials)?;
    inspection_event(
        state,
        task_id,
        &mac,
        "连接一体机",
        1,
        TaskEventLevel::Info,
        "SERVICE_INSPECTION_CONNECTING",
        "配置读取完成，正在连接一体机",
    )
    .await?;
    let connector = ObservedConnector::new(
        RusshConnector::default(),
        HostKeyRepository::new(state.local_store.pool().clone()),
        &envelope.local_project_id,
    );
    let target = RemoteTarget {
        host: detail.node.ip,
        port: profile.values.ssh_port,
        connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
    };
    let (session, observation) = tokio::select! {
        _ = cancellation.cancelled() => return Err(AppError::Cancelled),
        result = connector.connect_observed(&target, &auth) => result?,
    };
    let result = async {
        if observation.changed() {
            inspection_event(
                state,
                task_id,
                &mac,
                "连接一体机",
                1,
                TaskEventLevel::Warn,
                "SSH_HOST_KEY_CHANGED",
                &observation.message(),
            )
            .await?;
        }
        inspection_event(
            state,
            task_id,
            &mac,
            "读取服务状态",
            2,
            TaskEventLevel::Info,
            "SERVICE_INSPECTION_CONNECTED",
            "SSH认证通过，正在读取生效服务、容器状态和实际镜像",
        )
        .await?;
        // 使用内嵌的只读采集能力，通过stdin执行，不上传脚本或改变远端文件。
        let request = inspect_services_request(
            include_str!("../../resources/agent/edge-node-agent.sh"),
            &profile.values.aio_deploy_root,
            "manual",
            None,
        )?;
        let output = session
            .run(&request, cancellation, &NoopRemoteOutputSink)
            .await?;
        let mut report = parse_service_check_report_at(&output.stdout, started_at, &now())?;
        report.started_at = started_at.into();
        report.checked_at = now();
        report.source = "manual".into();
        for service in &mut report.services {
            service.checked_at = report.checked_at.clone();
            service.source = report.source.clone();
        }
        if output.exit_status != 0 || report.state == "failed" {
            return Err(AppError::Conflict(
                report
                    .error
                    .unwrap_or_else(|| "未能完整读取一体机服务状态".into()),
            ));
        }
        inspection_event(
            state,
            task_id,
            &mac,
            "保存检查结果",
            3,
            TaskEventLevel::Info,
            "SERVICE_INSPECTION_OBSERVED",
            &format!(
                "已读取{}个服务，正在保存本机检查结果",
                report.services.len()
            ),
        )
        .await?;
        for service in &report.services {
            let line = format!(
                "{}：{}；运行状态={}；实际镜像={}{}",
                service.service_name,
                if service.state == "normal" {
                    "正常"
                } else {
                    "需关注"
                },
                service.runtime_state,
                service.actual_image.as_deref().unwrap_or("未取得"),
                service
                    .message
                    .as_ref()
                    .map(|message| format!("；{message}"))
                    .unwrap_or_default()
            );
            inspection_event(
                state,
                task_id,
                &mac,
                "保存检查结果",
                3,
                if service.state == "normal" {
                    TaskEventLevel::Info
                } else {
                    TaskEventLevel::Warn
                },
                "SERVICE_INSPECTION_ITEM",
                &line,
            )
            .await?;
        }
        if cancellation.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        ensure_node_idle(&pools.workbench, &mac).await?;
        ServiceCheckRepository::new(state.local_store.pool().clone(), &envelope.local_project_id)
            .save_report(&mac, &report)
            .await?;
        Ok(report)
    }
    .await;
    if let Err(error) = session.disconnect().await {
        tracing::warn!(error = ?crate::core::log_safety::safe_error(&error), "disconnect service inspection session failed");
    }
    result
}

async fn ensure_node_idle(pool: &MySqlPool, mac: &str) -> AppResult<()> {
    if ResourceLeaseRepository::new(pool.clone())
        .active_lease("aio", mac)
        .await
        .map_err(map_formal_error)?
        .is_some()
    {
        return Err(AppError::Conflict(
            "该一体机正在部署或升级，请完成后再检查服务".into(),
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn inspection_event(
    state: &FormalAppState,
    task_id: &str,
    mac: &str,
    stage: &str,
    current: u64,
    level: TaskEventLevel,
    code: &str,
    message: &str,
) -> AppResult<()> {
    let task_state = state.task_repository.get(task_id).await?.state;
    state
        .task_repository
        .update_target(
            task_id,
            TargetUpdate {
                resource_type: "aio".into(),
                resource_key: mac.into(),
                state: if task_state == TaskState::Queued {
                    TargetState::Pending
                } else {
                    TargetState::Running
                },
                stage: stage.into(),
                progress_current: current,
                progress_total: INSPECTION_WORK_TOTAL,
                fencing_token: None,
                message_code: Some(code.into()),
                message_params_json: None,
            },
        )
        .await?;
    state
        .task_event_pipeline
        .emit(
            task_id,
            TaskEventInput {
                resource_type: Some("aio".into()),
                resource_key: Some(mac.into()),
                stage: stage.into(),
                status: "running".into(),
                progress_current: Some(current),
                progress_total: Some(INSPECTION_WORK_TOTAL),
                level,
                message_code: code.into(),
                message_params: BTreeMap::new(),
                message: Some(message.into()),
            },
        )
        .await?;
    Ok(())
}

async fn finish_inspection(
    state: &FormalAppState,
    task_id: &str,
    mac: &str,
    requested_state: TaskState,
    message: Option<&str>,
) -> AppResult<()> {
    for _ in 0..4 {
        let mut task = state.task_repository.get(task_id).await?;
        if task.state.is_terminal() {
            return Ok(());
        }
        let final_state = if task.state == TaskState::Cancelling {
            TaskState::Cancelled
        } else {
            requested_state
        };
        let intermediate = match task.state {
            TaskState::Draft => Some(TaskState::Checking),
            TaskState::Ready => Some(TaskState::Queued),
            TaskState::Queued if final_state != TaskState::Cancelled => Some(TaskState::Running),
            TaskState::Running if final_state == TaskState::Cancelled => {
                Some(TaskState::Cancelling)
            }
            _ => None,
        };
        if let Some(next) = intermediate {
            match state
                .task_repository
                .transition(task_id, task.state, next, None, None)
                .await
            {
                Ok(updated) => task = updated,
                Err(AppError::Conflict(_)) => continue,
                Err(error) => return Err(error),
            }
        }
        let (target_state, step_state, stage) = match final_state {
            TaskState::Succeeded => (TargetState::Succeeded, StepState::Succeeded, "服务检查完成"),
            TaskState::Cancelled => (
                TargetState::Cancelled,
                StepState::Cancelled,
                "服务检查已取消",
            ),
            _ => (TargetState::Failed, StepState::Failed, "服务检查失败"),
        };
        let summary = if final_state == TaskState::Cancelled {
            stage
        } else {
            message.unwrap_or(stage)
        };
        let target = TargetUpdate {
            resource_type: "aio".into(),
            resource_key: mac.into(),
            state: target_state,
            stage: stage.into(),
            progress_current: INSPECTION_WORK_TOTAL,
            progress_total: INSPECTION_WORK_TOTAL,
            fencing_token: None,
            message_code: Some("SERVICE_INSPECTION_FINISHED".into()),
            message_params_json: Some(summary.into()),
        };
        let step = TaskStepWrite {
            id: format!("{task_id}:service_inspection"),
            resource_type: Some("aio".into()),
            resource_key: Some(mac.into()),
            step_code: "service_inspection".into(),
            state: step_state,
            error_code: (final_state == TaskState::Failed)
                .then(|| "SERVICE_INSPECTION_FAILED".into()),
            message: Some(summary.into()),
        };
        match state
            .task_repository
            .finalize_projection(task_id, task.state, final_state, &[target], &[step])
            .await
        {
            Ok(()) => {}
            Err(AppError::Conflict(_)) => continue,
            Err(error) => return Err(error),
        }
        state
            .task_event_pipeline
            .emit(
                task_id,
                TaskEventInput {
                    resource_type: Some("aio".into()),
                    resource_key: Some(mac.into()),
                    stage: stage.into(),
                    status: final_state.as_str().into(),
                    progress_current: Some(INSPECTION_WORK_TOTAL),
                    progress_total: Some(INSPECTION_WORK_TOTAL),
                    level: if final_state == TaskState::Succeeded {
                        TaskEventLevel::Info
                    } else {
                        TaskEventLevel::Warn
                    },
                    message_code: "SERVICE_INSPECTION_FINISHED".into(),
                    message_params: BTreeMap::new(),
                    message: Some(summary.into()),
                },
            )
            .await?;
        return Ok(());
    }
    Err(AppError::Conflict(
        "服务检查任务状态持续变化，未能保存终态".into(),
    ))
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .expect("RFC3339 timestamp")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formal::config::AppPaths;
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::job_supervisor::JobSupervisor;
    use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};
    use std::sync::Arc;

    async fn fixture() -> (tempfile::TempDir, FormalAppState) {
        let temp = tempfile::tempdir().unwrap();
        let paths = AppPaths::from_data_dir(temp.path()).unwrap();
        paths.ensure().unwrap();
        let local_store = LocalStore::open(&paths.local_db).await.unwrap();
        sqlx::query("INSERT INTO local_project (id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES ('project','project','http://test','test',3306,'test','business','workbench','ref','1','1')")
            .execute(local_store.pool()).await.unwrap();
        let task_repository = TaskRepository::new(local_store.pool().clone());
        let task_event_bus = TaskEventBus::new(32).unwrap();
        let task_event_pipeline = TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus.clone(),
            SensitiveValueRedactor::default(),
        );
        let job_supervisor = JobSupervisor::default();
        let task_handler_registry = TaskHandlerRegistry::default();
        let task_queue =
            TaskQueue::start(4, 1, task_handler_registry.clone(), job_supervisor.clone())
                .await
                .unwrap();
        let state = FormalAppState {
            local_store,
            secret_store: Arc::new(MemorySecretStore::default()),
            runtime_registry: ProjectRuntimeRegistry::default(),
            job_supervisor,
            task_handler_registry,
            task_queue,
            task_event_bus,
            task_repository,
            task_event_pipeline,
            paths,
        };
        state
            .task_repository
            .create(CreateTask {
                id: "inspection".into(),
                local_project_id: "project".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: SERVICE_INSPECTION_OPERATION.into(),
                name: "检查服务".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: state
                    .paths
                    .project_task_log_path("project", "inspection")
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                targets: vec![("aio".into(), "001122334455".into())],
            })
            .await
            .unwrap();
        let mut previous = TaskState::Draft;
        for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
            state
                .task_repository
                .transition("inspection", previous, next, None, None)
                .await
                .unwrap();
            previous = next;
        }
        (temp, state)
    }

    #[tokio::test]
    async fn queued_inspection_cancels_without_remote_or_shared_writes() {
        let (_temp, state) = fixture().await;
        inspection_event(
            &state,
            "inspection",
            "001122334455",
            "等待服务检查",
            0,
            TaskEventLevel::Info,
            "SERVICE_INSPECTION_QUEUED",
            "等待检查",
        )
        .await
        .unwrap();
        assert_eq!(
            state.task_repository.targets("inspection").await.unwrap()[0].state,
            TargetState::Pending
        );
        let token = CancellationToken::new();
        token.cancel();
        execute_service_inspection(
            &state,
            TaskEnvelope {
                local_task_id: "inspection".into(),
                local_project_id: "project".into(),
                domain_type: "aio".into(),
                operation_type: SERVICE_INSPECTION_OPERATION.into(),
                resource_keys: vec!["001122334455".into()],
                priority: 0,
                payload_ref: None,
                payload_sha256: None,
            },
            token,
        )
        .await
        .unwrap();
        let task = state.task_repository.get("inspection").await.unwrap();
        assert_eq!(task.state, TaskState::Cancelled);
        assert!(task.remote_operation_record_id.is_none());
        assert_eq!(
            state.task_repository.targets("inspection").await.unwrap()[0].state,
            TargetState::Cancelled
        );
        assert!(
            std::fs::read_to_string(task.log_path)
                .unwrap()
                .contains("SERVICE_INSPECTION_FINISHED")
        );
    }

    #[tokio::test]
    async fn malformed_inspection_target_finishes_with_a_visible_error() {
        let (_temp, state) = fixture().await;
        let result = inspect_node(
            &state,
            &TaskEnvelope {
                local_task_id: "inspection".into(),
                local_project_id: "project".into(),
                domain_type: "aio".into(),
                operation_type: SERVICE_INSPECTION_OPERATION.into(),
                resource_keys: vec!["112233445566".into()],
                priority: 0,
                payload_ref: None,
                payload_sha256: None,
            },
            &CancellationToken::new(),
            &now(),
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("目标不一致"));
        finish_inspection(
            &state,
            "inspection",
            "001122334455",
            TaskState::Failed,
            Some("任务目标不一致"),
        )
        .await
        .unwrap();
        let task = state.task_repository.get("inspection").await.unwrap();
        assert_eq!(task.state, TaskState::Failed);
        assert!(
            std::fs::read_to_string(task.log_path)
                .unwrap()
                .contains("任务目标不一致")
        );
    }
}
