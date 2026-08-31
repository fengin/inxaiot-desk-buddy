use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::application::agent_protocol::{agent_exec_request, parse_agent_events};
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
                Some("SSH认证与HostKey固定校验通过".into()),
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
                    let _ = emit_progress(
                        progress,
                        config,
                        &step_code,
                        Some(&step_code),
                        Some(StepState::Failed),
                        TargetState::Failed,
                        action_start,
                        TaskEventLevel::Error,
                        "AGENT_ACTION_FAILED",
                        Some(error.to_string()),
                    );
                    return Err(AppError::Conflict(format!(
                        "Agent action {action} 失败：{error}"
                    )));
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
}

impl TransferProgressSink for TransferProgressBridge<'_> {
    fn emit(&self, transfer: TransferProgress) -> AppResult<()> {
        let completed = self.base_bytes.saturating_add(transfer.transferred);
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
    let bridge = TransferProgressBridge {
        progress,
        config,
        base_bytes,
        total_bytes,
    };
    let result = session
        .upload(
            &UploadRequest {
                operation_id: operation_id.into(),
                local_path: local.to_path_buf(),
                remote_path: remote.into(),
                expected_sha256: Some(sha256_file(local)?),
                overwrite: true,
                chunk_size: 1024 * 1024,
                inactivity_timeout: Duration::from_secs(60),
                minimum_bytes_per_second: 64 * 1024,
                minimum_total_timeout: Duration::from_secs(120),
            },
            cancellation,
            &bridge,
        )
        .await;
    if let Err(error) = &result {
        let _ = emit_progress(
            progress,
            config,
            "upload",
            Some("upload"),
            Some(StepState::Failed),
            TargetState::Failed,
            2_u64.saturating_add(base_bytes.saturating_mul(33) / total_bytes.max(1)),
            TaskEventLevel::Error,
            "SFTP_UPLOAD_FAILED",
            Some(error.to_string()),
        );
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
    let result = session.run(&request, cancellation, &output).await?;
    if result.exit_status != 0 {
        return Err(AppError::Conflict(format!(
            "远端命令退出码={}，详细输出见脱敏任务日志",
            result.exit_status
        )));
    }
    Ok(result.stdout)
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
