use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::application::agent_protocol::{
    agent_exec_request, parse_agent_events, parse_service_check_report_at,
};
use crate::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink, NoopDeploymentProgressSink,
};
use crate::application::ports::file_transfer::{
    FileTransferService, TransferProgress, TransferProgressSink, UploadRequest,
    validate_remote_path,
};
use crate::application::ports::remote_command::{
    ExecRequest, RemoteCommandExecutor, RemoteOutputChunk, RemoteOutputSink, RemoteOutputStream,
};
use crate::application::ports::remote_session::{
    HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{AgentInvocation, DeploymentMode, DeploymentPlan};
use crate::domain::aio::release::sha256_file;
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel};

#[derive(Clone, Debug)]
pub struct RemoteDeploymentFiles {
    pub local_agent: PathBuf,
    pub local_artifact: PathBuf,
    pub local_env: Option<PathBuf>,
    pub local_host_info: Option<PathBuf>,
    pub local_compose: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct RemoteDeploymentConfig {
    pub operation_id: String,
    pub release_fingerprint: String,
    pub mac_normalized: String,
    pub data_root: String,
    pub deploy_root: String,
    pub platform_api_host: String,
    pub platform_api_port: u16,
    pub platform_mqtt_host: String,
    pub platform_mqtt_port: u16,
    pub allow_existing_ports: bool,
    pub published_ports: Vec<u16>,
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_remote_deployment<C>(
    connector: &C,
    target: &RemoteTarget,
    auth: &RemoteAuth,
    host_key_policy: HostKeyPolicy,
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
) -> AppResult<()>
where
    C: RemoteConnector,
{
    execute_remote_deployment_with_checkpoint(
        connector,
        target,
        auth,
        host_key_policy,
        plan,
        files,
        config,
        cancellation,
        || async { Ok(()) },
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_remote_deployment_with_checkpoint<C, F, Fut>(
    connector: &C,
    target: &RemoteTarget,
    auth: &RemoteAuth,
    host_key_policy: HostKeyPolicy,
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
    checkpoint: F,
) -> AppResult<()>
where
    C: RemoteConnector,
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
{
    execute_remote_deployment_observed_with_checkpoint(
        connector,
        target,
        auth,
        host_key_policy,
        plan,
        files,
        config,
        cancellation,
        checkpoint,
        &NoopDeploymentProgressSink,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_remote_deployment_observed_with_checkpoint<C, F, Fut>(
    connector: &C,
    target: &RemoteTarget,
    auth: &RemoteAuth,
    host_key_policy: HostKeyPolicy,
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
    checkpoint: F,
    progress: &dyn DeploymentProgressSink,
) -> AppResult<()>
where
    C: RemoteConnector,
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
{
    validate_files(plan.mode, files)?;
    emit_progress(
        progress,
        config,
        "ssh_connect",
        None,
        None,
        TargetState::Running,
        0,
        TaskEventLevel::Info,
        "SSH_CONNECTING",
        Some(format!("连接{}:{}", target.host, target.port)),
    )?;
    checkpoint().await?;
    let session = match connector.connect(target, auth, host_key_policy).await {
        Ok(session) => {
            emit_progress(
                progress,
                config,
                "ssh_connected",
                None,
                None,
                TargetState::Running,
                1,
                TaskEventLevel::Info,
                "SSH_CONNECTED",
                Some("SSH认证通过，连接已建立".into()),
            )?;
            session
        }
        Err(error) => {
            let _ = emit_progress(
                progress,
                config,
                "ssh_connect",
                None,
                None,
                TargetState::Failed,
                0,
                TaskEventLevel::Error,
                "SSH_CONNECT_FAILED",
                Some(error.to_string()),
            );
            return Err(error);
        }
    };
    let result = execute_connected_deployment_observed_with_checkpoint(
        &session,
        plan,
        files,
        config,
        cancellation,
        checkpoint,
        progress,
    )
    .await;
    let disconnect = session.disconnect().await;
    match (result, disconnect) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

pub async fn execute_connected_deployment(
    session: &(impl RemoteConnection + ?Sized),
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
) -> AppResult<()> {
    execute_connected_deployment_with_checkpoint(
        session,
        plan,
        files,
        config,
        cancellation,
        || async { Ok(()) },
    )
    .await
}

pub async fn execute_connected_deployment_with_checkpoint<F, Fut>(
    session: &(impl RemoteConnection + ?Sized),
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
    checkpoint: F,
) -> AppResult<()>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
{
    execute_connected_deployment_observed_with_checkpoint(
        session,
        plan,
        files,
        config,
        cancellation,
        checkpoint,
        &NoopDeploymentProgressSink,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_connected_deployment_observed_with_checkpoint<F, Fut>(
    session: &(impl RemoteConnection + ?Sized),
    plan: &DeploymentPlan,
    files: &RemoteDeploymentFiles,
    config: &RemoteDeploymentConfig,
    cancellation: &CancellationToken,
    checkpoint: F,
    progress: &dyn DeploymentProgressSink,
) -> AppResult<()>
where
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
{
    let staging = format!(
        "{}/.inxaiot-desk-buddy/{}/{}",
        config.data_root.trim_end_matches('/'),
        config.operation_id,
        config.mac_normalized
    );
    validate_remote_path(&staging)?;
    if config.data_root.trim() == "/" || config.deploy_root.trim() == "/" {
        return Err(AppError::InvalidConfig(
            "远端数据目录和部署目录不能是根目录".into(),
        ));
    }
    let remote_agent = format!("{staging}/edge-node-agent.sh");
    let remote_artifact = if plan.mode == DeploymentMode::ServiceUpgrade {
        format!("{staging}/service-image.tar")
    } else {
        format!("{staging}/release.tar")
    };
    let remote_env = format!("{staging}/.env");
    let remote_host_info = format!("{staging}/host-info.json");
    let remote_compose = format!("{staging}/docker-compose.yml");
    let agent_bytes = local_file_size(&files.local_agent)?;
    let artifact_bytes = local_file_size(&files.local_artifact)?;
    let env_bytes = files
        .local_env
        .as_deref()
        .map(local_file_size)
        .transpose()?
        .unwrap_or_default();
    let host_info_bytes = files
        .local_host_info
        .as_deref()
        .map(local_file_size)
        .transpose()?
        .unwrap_or_default();
    let compose_bytes = files
        .local_compose
        .as_deref()
        .map(local_file_size)
        .transpose()?
        .unwrap_or_default();
    let upload_total = agent_bytes
        .saturating_add(artifact_bytes)
        .saturating_add(env_bytes)
        .saturating_add(host_info_bytes)
        .saturating_add(compose_bytes)
        .max(1);
    let mut uploaded_base = 0_u64;
    let execution: AppResult<()> = async {
        emit_progress(
            progress,
            config,
            "prepare_remote",
            None,
            None,
            TargetState::Running,
            1,
            TaskEventLevel::Info,
            "REMOTE_STAGING_PREPARE",
            Some(staging.clone()),
        )?;
        checkpoint().await?;
        run_command_observed(
            session,
            ExecRequest {
                program: "mkdir".into(),
                args: vec!["-p".into(), staging.clone()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(30),
                inactivity_timeout: Duration::from_secs(15),
            },
            cancellation,
            progress,
            config,
            "prepare_remote",
            None,
            1,
        )
        .await?;
        checkpoint().await?;
        upload_observed(
            session,
            &config.operation_id,
            &files.local_agent,
            &remote_agent,
            cancellation,
            progress,
            config,
            uploaded_base,
            upload_total,
        )
        .await?;
        uploaded_base = uploaded_base.saturating_add(agent_bytes);
        checkpoint().await?;
        upload_observed(
            session,
            &config.operation_id,
            &files.local_artifact,
            &remote_artifact,
            cancellation,
            progress,
            config,
            uploaded_base,
            upload_total,
        )
        .await?;
        uploaded_base = uploaded_base.saturating_add(artifact_bytes);
        if let Some(env) = &files.local_env {
            checkpoint().await?;
            upload_observed(
                session,
                &config.operation_id,
                env,
                &remote_env,
                cancellation,
                progress,
                config,
                uploaded_base,
                upload_total,
            )
            .await?;
            uploaded_base = uploaded_base.saturating_add(env_bytes);
        }
        if let Some(host_info) = &files.local_host_info {
            checkpoint().await?;
            upload_observed(
                session,
                &config.operation_id,
                host_info,
                &remote_host_info,
                cancellation,
                progress,
                config,
                uploaded_base,
                upload_total,
            )
            .await?;
            uploaded_base = uploaded_base.saturating_add(host_info_bytes);
        }
        if let Some(compose) = &files.local_compose {
            checkpoint().await?;
            upload_observed(
                session,
                &config.operation_id,
                compose,
                &remote_compose,
                cancellation,
                progress,
                config,
                uploaded_base,
                upload_total,
            )
            .await?;
            uploaded_base = uploaded_base.saturating_add(compose_bytes);
        }
        emit_progress(
            progress,
            config,
            "upload",
            Some("upload"),
            Some(StepState::Succeeded),
            TargetState::Running,
            35,
            TaskEventLevel::Info,
            "SFTP_UPLOAD_COMPLETED",
            Some(format!("{uploaded_base}字节上传并校验完成")),
        )?;
        checkpoint().await?;
        run_command_observed(
            session,
            ExecRequest {
                program: "chmod".into(),
                args: vec!["700".into(), remote_agent.clone()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(30),
                inactivity_timeout: Duration::from_secs(15),
            },
            cancellation,
            progress,
            config,
            "prepare_agent",
            Some("upload"),
            35,
        )
        .await?;
        let invocations = plan.agent_invocations();
        let invocation_count = u64::try_from(invocations.len()).unwrap_or(1).max(1);
        for (index, invocation) in invocations.into_iter().enumerate() {
            checkpoint().await?;
            let action = invocation.action.clone();
            let step_code = action.replace('-', "_");
            let index = u64::try_from(index).unwrap_or_default();
            let action_start =
                35_u64.saturating_add(63_u64.saturating_mul(index) / invocation_count);
            let action_end =
                35_u64.saturating_add(63_u64.saturating_mul(index + 1) / invocation_count);
            emit_progress(
                progress,
                config,
                &step_code,
                Some(&step_code),
                Some(StepState::Running),
                TargetState::Running,
                action_start,
                TaskEventLevel::Info,
                "AGENT_ACTION_STARTED",
                Some(format!("Agent开始执行{action}")),
            )?;
            let mut environment = invocation.environment;
            environment.extend([
                (
                    "SERVICE_CHECK_SOURCE".into(),
                    deployment_check_source(plan.mode).into(),
                ),
                ("DATA_ROOT".into(), config.data_root.clone()),
                ("DEPLOY_ROOT".into(), config.deploy_root.clone()),
                ("REMOTE_PACKAGE".into(), remote_artifact.clone()),
                ("REMOTE_IMAGE".into(), remote_artifact.clone()),
                ("REMOTE_ENV".into(), remote_env.clone()),
                ("REMOTE_HOST_INFO".into(), remote_host_info.clone()),
                ("REMOTE_COMPOSE".into(), remote_compose.clone()),
                ("TASK_ID".into(), config.operation_id.clone()),
                (
                    "RELEASE_FINGERPRINT".into(),
                    config.release_fingerprint.clone(),
                ),
                ("PLATFORM_API_HOST".into(), config.platform_api_host.clone()),
                (
                    "PLATFORM_API_PORT".into(),
                    config.platform_api_port.to_string(),
                ),
                (
                    "PLATFORM_MQTT_HOST".into(),
                    config.platform_mqtt_host.clone(),
                ),
                (
                    "PLATFORM_MQTT_PORT".into(),
                    config.platform_mqtt_port.to_string(),
                ),
            ]);
            environment.insert(
                "PORTS".into(),
                config
                    .published_ports
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            if plan.mode == DeploymentMode::FullUpgrade || config.allow_existing_ports {
                environment.insert("ALLOW_EXISTING_PORTS".into(), "true".into());
            }
            let output = match run_command_observed(
                session,
                agent_exec_request(
                    &remote_agent,
                    AgentInvocation {
                        action: invocation.action,
                        environment,
                    },
                )?,
                cancellation,
                progress,
                config,
                &step_code,
                Some(&step_code),
                action_start,
            )
            .await
            {
                Ok(output) => output,
                Err(error) => {
                    let cancelled = matches!(&error, AppError::Cancelled);
                    let _ = emit_progress(
                        progress,
                        config,
                        &step_code,
                        Some(&step_code),
                        Some(if cancelled {
                            StepState::Cancelled
                        } else {
                            StepState::Failed
                        }),
                        if cancelled {
                            TargetState::Cancelled
                        } else {
                            TargetState::Failed
                        },
                        action_start,
                        if cancelled {
                            TaskEventLevel::Warn
                        } else {
                            TaskEventLevel::Error
                        },
                        if cancelled {
                            "AGENT_ACTION_CANCELLED"
                        } else {
                            "AGENT_ACTION_FAILED"
                        },
                        Some(error.to_string()),
                    );
                    return Err(agent_action_error(&action, error));
                }
            };
            let events = match parse_agent_events(&output) {
                Ok(events) => events,
                Err(error) => {
                    let _ = emit_progress(
                        progress,
                        config,
                        &step_code,
                        Some(&step_code),
                        Some(StepState::Failed),
                        TargetState::Failed,
                        action_start,
                        TaskEventLevel::Error,
                        "AGENT_EVENT_INVALID",
                        Some(error.to_string()),
                    );
                    return Err(error);
                }
            };
            let mut agent_failed = None;
            for event in events {
                let failed = matches!(event.status.as_str(), "failed" | "failure" | "error")
                    || event.code.is_some_and(|code| code != 0);
                let succeeded =
                    matches!(event.status.as_str(), "success" | "succeeded" | "completed");
                let step_state = if failed {
                    Some(StepState::Failed)
                } else if succeeded {
                    Some(StepState::Succeeded)
                } else if event.status == "running" {
                    Some(StepState::Running)
                } else {
                    None
                };
                let overall = if succeeded {
                    action_end
                } else {
                    action_start.saturating_add(action_end.saturating_sub(action_start) / 2)
                };
                emit_progress(
                    progress,
                    config,
                    &step_code,
                    Some(&step_code),
                    step_state,
                    if failed {
                        TargetState::Failed
                    } else {
                        TargetState::Running
                    },
                    overall,
                    if failed {
                        TaskEventLevel::Error
                    } else {
                        TaskEventLevel::Info
                    },
                    "AGENT_EVENT",
                    Some(format!("{}：{}", event.step, event.message)),
                )?;
                if failed {
                    agent_failed = Some(event.message);
                }
            }
            if let Some(message) = agent_failed {
                return Err(AppError::Conflict(format!(
                    "Agent action {action} 返回失败：{message}"
                )));
            }
            emit_progress(
                progress,
                config,
                &step_code,
                Some(&step_code),
                Some(StepState::Succeeded),
                TargetState::Running,
                action_end,
                TaskEventLevel::Info,
                "AGENT_ACTION_COMPLETED",
                Some(format!("Agent完成{action}")),
            )?;
        }
        Ok(())
    }
    .await;
    let cleanup = run_command_observed(
        session,
        ExecRequest {
            program: "rm".into(),
            args: vec!["-rf".into(), staging],
            env: BTreeMap::new(),
            stdin: None,
            total_timeout: Duration::from_secs(60),
            inactivity_timeout: Duration::from_secs(30),
        },
        &CancellationToken::new(),
        progress,
        config,
        "cleanup",
        None,
        99,
    )
    .await;
    let cleanup_failed = cleanup.is_err();
    match (execution.as_ref(), cleanup) {
        (Ok(_), Ok(_)) => emit_progress(
            progress,
            config,
            "remote_complete",
            None,
            None,
            TargetState::Running,
            100,
            TaskEventLevel::Info,
            "REMOTE_DEPLOYMENT_COMPLETED",
            Some("远端部署步骤完成且临时目录已清理".into()),
        )?,
        (Ok(_), Err(error)) | (Err(_), Err(error)) => emit_progress(
            progress,
            config,
            "cleanup_warning",
            None,
            None,
            TargetState::Running,
            100,
            TaskEventLevel::Warn,
            "REMOTE_CLEANUP_FAILED",
            Some(error.to_string()),
        )?,
        (Err(_), Ok(_)) => emit_progress(
            progress,
            config,
            "cleanup",
            None,
            None,
            TargetState::Running,
            100,
            TaskEventLevel::Info,
            "REMOTE_CLEANUP_COMPLETED",
            Some("远端执行未完成，本次临时目录已精确清理".into()),
        )?,
    }
    if execution.is_ok() && cleanup_failed {
        return Err(AppError::Conflict(
            "远端部署已执行但敏感staging清理失败，任务不能标记成功".into(),
        ));
    }
    execution
}

struct TransferProgressBridge<'a> {
    progress: &'a dyn DeploymentProgressSink,
    config: &'a RemoteDeploymentConfig,
    base_bytes: u64,
    total_bytes: u64,
    high_watermark: AtomicU64,
}

impl TransferProgressSink for TransferProgressBridge<'_> {
    fn emit(&self, transfer: TransferProgress) -> AppResult<()> {
        let transferred = self
            .high_watermark
            .fetch_max(transfer.transferred, Ordering::Relaxed)
            .max(transfer.transferred);
        let completed = self.base_bytes.saturating_add(transferred);
        let total = self.total_bytes.max(1);
        let overall = 2_u64.saturating_add(completed.saturating_mul(33) / total);
        emit_progress(
            self.progress,
            self.config,
            "upload",
            Some("upload"),
            Some(StepState::Running),
            TargetState::Running,
            overall.min(35),
            TaskEventLevel::Info,
            "SFTP_UPLOAD_PROGRESS",
            Some(format!("{completed}/{total}字节")),
        )
    }
}

struct RemoteOutputBridge<'a> {
    progress: &'a dyn DeploymentProgressSink,
    config: &'a RemoteDeploymentConfig,
    stage: &'a str,
    step_code: Option<&'a str>,
    overall: u64,
}

impl RemoteOutputSink for RemoteOutputBridge<'_> {
    fn emit(&self, chunk: RemoteOutputChunk) -> AppResult<()> {
        let (level, code) = match chunk.stream {
            RemoteOutputStream::Stdout => (TaskEventLevel::Info, "SSH_STDOUT"),
            RemoteOutputStream::Stderr => (TaskEventLevel::Warn, "SSH_STDERR"),
        };
        emit_progress(
            self.progress,
            self.config,
            self.stage,
            self.step_code,
            None,
            TargetState::Running,
            self.overall,
            level,
            code,
            Some(chunk.text),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_progress(
    progress: &dyn DeploymentProgressSink,
    config: &RemoteDeploymentConfig,
    stage: &str,
    step_code: Option<&str>,
    step_state: Option<StepState>,
    target_state: TargetState,
    overall: u64,
    level: TaskEventLevel,
    message_code: &str,
    message: Option<String>,
) -> AppResult<()> {
    progress.emit(DeploymentProgressEvent {
        mac: config.mac_normalized.clone(),
        stage: stage.into(),
        step_code: step_code.map(str::to_string),
        step_state,
        target_state,
        progress_current: overall.min(100),
        progress_total: 100,
        level,
        message_code: message_code.into(),
        message,
    })
}

#[allow(clippy::too_many_arguments)]
async fn upload_observed(
    session: &(impl FileTransferService + ?Sized),
    operation_id: &str,
    local: &Path,
    remote: &str,
    cancellation: &CancellationToken,
    progress: &dyn DeploymentProgressSink,
    config: &RemoteDeploymentConfig,
    base_bytes: u64,
    total_bytes: u64,
) -> AppResult<()> {
    let file_size = local_file_size(local)?;
    let file_name = local
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("发布文件");
    let start_progress = 2_u64.saturating_add(base_bytes.saturating_mul(33) / total_bytes.max(1));
    emit_progress(
        progress,
        config,
        "upload",
        Some("upload"),
        Some(StepState::Running),
        TargetState::Running,
        start_progress,
        TaskEventLevel::Info,
        "SFTP_UPLOAD_STARTED",
        Some(format!("开始上传{file_name}（{file_size}字节）")),
    )?;
    let bridge = TransferProgressBridge {
        progress,
        config,
        base_bytes,
        total_bytes,
        high_watermark: AtomicU64::new(0),
    };
    let request = UploadRequest {
        operation_id: operation_id.into(),
        local_path: local.to_path_buf(),
        remote_path: remote.into(),
        expected_sha256: Some(sha256_file(local)?),
        overwrite: true,
        chunk_size: 1024 * 1024,
        inactivity_timeout: Duration::from_secs(60),
        minimum_bytes_per_second: 64 * 1024,
        minimum_total_timeout: Duration::from_secs(120),
    };
    let mut attempt = 1_u8;
    let result = loop {
        match session.upload(&request, cancellation, &bridge).await {
            Ok(()) => break Ok(()),
            Err(error)
                if attempt < 3 && matches!(error, AppError::Sftp { .. } | AppError::Ssh { .. }) =>
            {
                attempt += 1;
                let retry_progress = 2_u64.saturating_add(
                    base_bytes
                        .saturating_add(bridge.high_watermark.load(Ordering::Relaxed))
                        .saturating_mul(33)
                        / total_bytes.max(1),
                );
                emit_progress(
                    progress,
                    config,
                    "upload",
                    Some("upload"),
                    Some(StepState::Running),
                    TargetState::Running,
                    retry_progress.min(35),
                    TaskEventLevel::Warn,
                    "SFTP_UPLOAD_RETRY",
                    Some(format!(
                        "{file_name}上传通道异常，正在进行第{attempt}次尝试"
                    )),
                )?;
                tokio::select! {
                    _ = cancellation.cancelled() => break Err(AppError::Cancelled),
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {}
                }
            }
            Err(error) => break Err(error),
        }
    };
    match &result {
        Ok(()) => emit_progress(
            progress,
            config,
            "upload",
            None,
            None,
            TargetState::Running,
            2_u64.saturating_add(
                base_bytes.saturating_add(file_size).saturating_mul(33) / total_bytes.max(1),
            ),
            TaskEventLevel::Info,
            "SFTP_UPLOAD_FILE_COMPLETED",
            Some(format!("{file_name}上传并校验完成")),
        )?,
        Err(error) => {
            let _ = emit_progress(
                progress,
                config,
                "upload",
                Some("upload"),
                Some(StepState::Failed),
                TargetState::Failed,
                start_progress,
                TaskEventLevel::Error,
                "SFTP_UPLOAD_FAILED",
                Some(error.to_string()),
            );
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
async fn run_command_observed(
    session: &(impl RemoteCommandExecutor + ?Sized),
    request: ExecRequest,
    cancellation: &CancellationToken,
    progress: &dyn DeploymentProgressSink,
    config: &RemoteDeploymentConfig,
    stage: &str,
    step_code: Option<&str>,
    overall: u64,
) -> AppResult<String> {
    let output = RemoteOutputBridge {
        progress,
        config,
        stage,
        step_code,
        overall,
    };
    let observed_started_at = time::OffsetDateTime::now_utc();
    let result = session.run(&request, cancellation, &output).await?;
    if result
        .stdout
        .lines()
        .any(|line| line.contains("\"service_observation\""))
    {
        let observation: AppResult<()> = (|| {
            let observed_started_at = observed_started_at
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| AppError::InvalidConfig("生成服务检查开始时间失败".into()))?;
            let observed_checked_at = time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| AppError::InvalidConfig("生成服务检查结束时间失败".into()))?;
            let report = parse_service_check_report_at(
                &result.stdout,
                &observed_started_at,
                &observed_checked_at,
            )
            .unwrap_or_else(|error| {
                crate::domain::aio::service_check::ServiceCheckReport {
                    started_at: observed_started_at,
                    checked_at: observed_checked_at,
                    source: request
                        .env
                        .get("SERVICE_CHECK_SOURCE")
                        .cloned()
                        .unwrap_or_else(|| "manual".into()),
                    scope: if request.env.contains_key("SERVICE_NAME") {
                        "service"
                    } else {
                        "all"
                    }
                    .into(),
                    service_name: request.env.get("SERVICE_NAME").cloned(),
                    expected_services: Vec::new(),
                    services: Vec::new(),
                    state: "failed".into(),
                    error: Some(format!(
                        "真实服务检查报告无法解析，保留上次有效记录：{error}"
                    )),
                }
            });
            let abnormal = report
                .services
                .iter()
                .filter(|service| service.state != "normal")
                .count();
            let message = if report.state == "failed" {
                format!(
                    "服务检查失败：{}",
                    report.error.as_deref().unwrap_or("未取得运行事实")
                )
            } else if abnormal > 0 {
                format!("服务检查发现{abnormal}项异常、版本不符或状态未知，请查看一体机服务详情")
            } else {
                format!("已检查{}项服务，运行事实均正常", report.services.len())
            };
            let warning = report.state == "failed" || abnormal > 0;
            progress.service_check(&config.mac_normalized, report)?;
            emit_progress(
                progress,
                config,
                stage,
                None,
                None,
                TargetState::Running,
                overall,
                if warning {
                    TaskEventLevel::Warn
                } else {
                    TaskEventLevel::Info
                },
                "SERVICE_CHECK_OBSERVED",
                Some(message),
            )?;
            Ok(())
        })();
        if let Err(error) = observation {
            // 服务观测是旁路信息，采集/缓存/提示失败不得改写Agent原退出结果。
            let _ = emit_progress(
                progress,
                config,
                stage,
                None,
                None,
                TargetState::Running,
                overall,
                TaskEventLevel::Warn,
                "SERVICE_CHECK_OBSERVATION_FAILED",
                Some(format!("服务检查结果未能保存在本机：{error}")),
            );
        }
    }
    if result.exit_status != 0 {
        return Err(remote_command_exit_error(
            result.exit_status,
            &result.stdout,
        ));
    }
    Ok(result.stdout)
}

pub fn deployment_check_source(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::FirstDeploy => "first_deploy",
        DeploymentMode::FullUpgrade => "full_upgrade",
        DeploymentMode::ServiceUpgrade => "service_upgrade",
    }
}

fn remote_command_exit_error(exit_status: u32, stdout: &str) -> AppError {
    let detail = parse_agent_events(stdout).ok().and_then(|events| {
        events.into_iter().rev().find_map(|event| {
            let failed = matches!(event.status.as_str(), "failed" | "failure" | "error")
                || event.code.is_some_and(|code| code != 0);
            (failed && !event.message.trim().is_empty())
                .then(|| localized_agent_failure(event.message.trim()))
        })
    });
    AppError::Conflict(match detail {
        Some(detail) => format!("远端命令退出码={exit_status}：{detail}"),
        None => format!("远端命令退出码={exit_status}，详细输出见脱敏任务日志"),
    })
}

fn localized_agent_failure(message: &str) -> String {
    if message == "current release does not exist" {
        return "当前一体机不存在可升级的已部署版本，请先执行首次部署".into();
    }
    if let Some(file) = message.strip_prefix("required backup file is missing: ") {
        return format!("当前发布缺少备份所需文件：{file}");
    }
    if message == "host-info.json is missing" {
        return "当前一体机缺少host-info.json，无法执行升级备份".into();
    }
    message.into()
}

fn agent_action_error(action: &str, error: AppError) -> AppError {
    match error {
        AppError::Cancelled => AppError::Cancelled,
        error => AppError::Conflict(format!("Agent action {action} 失败：{error}")),
    }
}

fn local_file_size(path: &Path) -> AppResult<u64> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len())
        .map_err(|error| AppError::io("读取上传文件大小", &error))
}

fn validate_files(mode: DeploymentMode, files: &RemoteDeploymentFiles) -> AppResult<()> {
    if !files.local_agent.is_file() || !files.local_artifact.is_file() {
        return Err(AppError::InvalidConfig("Agent或发布物文件不存在".into()));
    }
    if mode != DeploymentMode::ServiceUpgrade
        && (files.local_env.as_ref().is_none_or(|path| !path.is_file())
            || files
                .local_host_info
                .as_ref()
                .is_none_or(|path| !path.is_file())
            || files
                .local_compose
                .as_ref()
                .is_none_or(|path| !path.is_file()))
    {
        return Err(AppError::InvalidConfig(
            "完整部署缺少.env、host-info或实际Compose文件".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{agent_action_error, remote_command_exit_error};

    #[test]
    fn agent_action_cancellation_is_not_converted_to_failure() {
        let error = agent_action_error("service-check", super::AppError::Cancelled);
        assert!(matches!(error, super::AppError::Cancelled));
    }

    #[tokio::test]
    async fn observation_parse_cache_and_notice_failures_do_not_change_command_outcome() {
        use crate::application::ports::remote_command::{RemoteCommandResult, RemoteOutputSink};
        use crate::domain::aio::service_check::ServiceCheckReport;
        struct MalformedObservation(u32);
        impl super::RemoteCommandExecutor for MalformedObservation {
            async fn run(
                &self,
                _: &super::ExecRequest,
                _: &super::CancellationToken,
                _: &dyn RemoteOutputSink,
            ) -> crate::core::error::AppResult<RemoteCommandResult> {
                Ok(RemoteCommandResult {
                    exit_status: self.0,
                    stdout:
                        "{\"step\":\"service_observation\",\"status\":\"info\",\"report\":{}}\n"
                            .into(),
                    stderr: String::new(),
                    duration_ms: 1,
                })
            }
        }
        struct CaptureFailure(bool);
        impl super::DeploymentProgressSink for CaptureFailure {
            fn emit(&self, _: super::DeploymentProgressEvent) -> crate::core::error::AppResult<()> {
                Err(super::AppError::Conflict("观测提示失败".into()))
            }
            fn service_check(
                &self,
                _: &str,
                report: ServiceCheckReport,
            ) -> crate::core::error::AppResult<()> {
                assert_eq!(report.state, "failed");
                assert!(report.services.is_empty());
                if self.0 {
                    Err(super::AppError::Conflict("观测缓存失败".into()))
                } else {
                    Ok(())
                }
            }
        }
        let config = super::RemoteDeploymentConfig {
            operation_id: "operation".into(),
            release_fingerprint: "fingerprint".into(),
            mac_normalized: "001122334455".into(),
            data_root: "/opt/data".into(),
            deploy_root: "/opt/data/deploy".into(),
            platform_api_host: "platform.test".into(),
            platform_api_port: 80,
            platform_mqtt_host: "platform.test".into(),
            platform_mqtt_port: 1883,
            allow_existing_ports: false,
            published_ports: Vec::new(),
        };
        for exit_status in [0, 42] {
            for reject_cache in [false, true] {
                let request = crate::application::agent_protocol::inspect_services_request(
                    "agent",
                    &config.deploy_root,
                    "manual",
                    None,
                )
                .unwrap();
                let result = super::run_command_observed(
                    &MalformedObservation(exit_status),
                    request,
                    &super::CancellationToken::new(),
                    &CaptureFailure(reject_cache),
                    &config,
                    "health",
                    None,
                    90,
                )
                .await;
                if exit_status == 0 {
                    assert!(result.is_ok());
                } else {
                    assert!(result.unwrap_err().to_string().contains("退出码=42"));
                }
            }
        }
    }

    #[tokio::test]
    async fn failed_upgrade_keeps_the_rollback_observation_and_the_failure_result() {
        use crate::application::ports::remote_command::{RemoteCommandResult, RemoteOutputSink};
        use crate::domain::aio::service_check::ServiceCheckReport;
        use std::sync::Mutex;

        struct FailedUpgrade;
        impl super::RemoteCommandExecutor for FailedUpgrade {
            async fn run(
                &self,
                _request: &super::ExecRequest,
                _cancellation: &super::CancellationToken,
                _output: &dyn RemoteOutputSink,
            ) -> crate::core::error::AppResult<RemoteCommandResult> {
                Ok(RemoteCommandResult {
                    exit_status: 86, stderr: String::new(), duration_ms: 1,
                    stdout: concat!(
                        "{\"step\":\"service_upgrade\",\"status\":\"failed\",\"message\":\"service health failed; previous image restored\"}\n",
                        "{\"step\":\"service_observation\",\"status\":\"info\",\"report\":{",
                        "\"startedAt\":\"2026-09-05T01:00:00Z\",\"checkedAt\":\"2026-09-05T01:00:01Z\",",
                        "\"source\":\"rollback\",\"scope\":\"service\",\"serviceName\":\"edge\",\"expectedServices\":[\"edge\",\"rule\"],",
                        "\"services\":[{\"serviceName\":\"edge\",\"state\":\"normal\",\"runtimeState\":\"running\",",
                        "\"expectedImage\":\"edge:old\",\"actualImage\":\"edge:old\",\"imageId\":\"sha256:old\",",
                        "\"checkedAt\":\"2026-09-05T01:00:01Z\",\"source\":\"rollback\"}],\"state\":\"succeeded\"}}\n"
                    ).into(),
                })
            }
        }
        #[derive(Default)]
        struct Capture(Mutex<Option<ServiceCheckReport>>);
        impl super::DeploymentProgressSink for Capture {
            fn emit(&self, _: super::DeploymentProgressEvent) -> crate::core::error::AppResult<()> {
                Ok(())
            }
            fn service_check(
                &self,
                _: &str,
                report: ServiceCheckReport,
            ) -> crate::core::error::AppResult<()> {
                *self.0.lock().unwrap() = Some(report);
                Ok(())
            }
        }
        let capture = Capture::default();
        let config = super::RemoteDeploymentConfig {
            operation_id: "operation".into(),
            release_fingerprint: "fingerprint".into(),
            mac_normalized: "001122334455".into(),
            data_root: "/opt/data".into(),
            deploy_root: "/opt/data/deploy".into(),
            platform_api_host: "platform.test".into(),
            platform_api_port: 80,
            platform_mqtt_host: "platform.test".into(),
            platform_mqtt_port: 1883,
            allow_existing_ports: false,
            published_ports: Vec::new(),
        };
        let request = crate::application::agent_protocol::inspect_services_request(
            "agent",
            &config.deploy_root,
            "rollback",
            Some("edge"),
        )
        .unwrap();
        let result = super::run_command_observed(
            &FailedUpgrade,
            request,
            &super::CancellationToken::new(),
            &capture,
            &config,
            "service_upgrade",
            None,
            90,
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("退出码=86"));
        let report = capture.0.lock().unwrap().clone().unwrap();
        assert_eq!(report.source, "rollback");
        assert_eq!(report.services.len(), 1);
        assert_eq!(report.services[0].actual_image.as_deref(), Some("edge:old"));
        assert_ne!(report.started_at, "2026-09-05T01:00:00Z");
        assert_eq!(report.services[0].checked_at, report.checked_at);
    }

    #[test]
    fn failed_agent_event_is_exposed_as_a_readable_remote_error() {
        let error = remote_command_exit_error(
            41,
            concat!(
                "{\"protocolVersion\":\"1\"}\n",
                "{\"step\":\"retention\",\"status\":\"success\",\"message\":\"done\"}\n",
                "{\"step\":\"backup\",\"status\":\"failed\",\"message\":\"current release does not exist\"}\n"
            ),
        );

        let message = error.to_string();
        assert!(message.contains("退出码=41"));
        assert!(message.contains("当前一体机不存在可升级的已部署版本"));
        assert!(!message.contains("current release does not exist"));
    }

    #[test]
    fn unstructured_remote_failure_keeps_the_log_fallback() {
        let error = remote_command_exit_error(9, "plain command output");

        assert!(error.to_string().contains("详细输出见脱敏任务日志"));
    }
}
