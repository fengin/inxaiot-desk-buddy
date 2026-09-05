use std::collections::{BTreeMap, HashMap};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::application::deployment_executor::{
    DeploymentExecutionSummary, execute_deployment_targets,
};
use crate::application::execution_coordinator::{ExecutionCoordinator, ExecutionLifecyclePort};
use crate::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink,
};
use crate::application::ports::remote_session::{HostKeyPolicy, RemoteAuth, RemoteTarget};
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::deployment_workflow::{
    DeploymentExecutionSnapshot, DeploymentTargetSnapshot,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::release::{ReleaseImage, ReleaseManifest, sha256_file};
use crate::domain::aio::release_profile::inspect_compose_published_ports;
use crate::domain::aio::release_render::ReleaseRenderContext;
use crate::domain::aio::service_check::ServiceCheckReport;
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::formal::runtime_registry::ConnectionHealth;
use crate::infrastructure::agent_asset::effective_agent_asset;
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::client_instance::application_instance_id;
use crate::infrastructure::database::DualMySqlPools;
use crate::infrastructure::deployment_control::{
    finalize_deployment_control, mark_deployment_finalizing_failed, mark_deployment_interrupted,
    start_deployment_control, start_deployment_heartbeat,
};
use crate::infrastructure::deployment_progress::{
    DEPLOYMENT_PROGRESS_TOTAL, DEPLOYMENT_REMOTE_PROGRESS_START, TaskProgressGuard,
    TaskProgressReporter,
};
use crate::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles,
    execute_remote_deployment_observed_with_checkpoint,
};
use crate::infrastructure::device_api::{AioRegistrationPayload, DeviceApiClient};
use crate::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;
use crate::infrastructure::project_context::{map_formal_error, project_database};
use crate::infrastructure::release_archive::create_generated_release_tar_observed;
use crate::infrastructure::release_remote_auth::release_remote_auth;
use crate::infrastructure::release_template::render_release_templates;
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::remote::observed::ObservedConnector;
use crate::infrastructure::service_check_repository::ServiceCheckRepository;

const GLOBAL_REMOTE_NODE_CONCURRENCY: usize = 5;

fn global_remote_node_slots() -> Arc<Semaphore> {
    static SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SLOTS
        .get_or_init(|| Arc::new(Semaphore::new(GLOBAL_REMOTE_NODE_CONCURRENCY)))
        .clone()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchDeploymentInput {
    pub snapshot: DeploymentExecutionSnapshot,
}

pub async fn run_submitted_deployment(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    input: LaunchDeploymentInput,
    cancellation: tokio_util::sync::CancellationToken,
) -> AppResult<DeploymentExecutionSummary> {
    let cancellation_observer = cancellation.clone();
    let result = launch_deployment_inner(
        state,
        local_project_id,
        local_task_id,
        input,
        Some(cancellation),
    )
    .await;
    if let Err(error) = &result {
        converge_submission_failure(
            state,
            local_task_id,
            error,
            cancellation_observer.is_cancelled(),
        )
        .await;
    }
    result
}

async fn launch_deployment_inner(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    input: LaunchDeploymentInput,
    registered_cancellation: Option<tokio_util::sync::CancellationToken>,
) -> AppResult<DeploymentExecutionSummary> {
    let snapshot = input.snapshot;
    snapshot.validate(local_project_id)?;
    let mut plan_input = snapshot.plan.clone();
    normalize_target_macs(&mut plan_input)?;
    let initial_plan = DeploymentPlan::build(plan_input.clone())?;
    let target_macs = initial_plan.target_macs.clone();
    plan_input.target_macs = target_macs.clone();
    let cancellation = match registered_cancellation {
        Some(cancellation) => cancellation,
        None => state.job_supervisor.register(local_task_id).await?,
    };
    begin_deployment_preparation(state, local_task_id, &target_macs).await?;
    check_deployment_cancelled(&cancellation)?;
    let preparation_guard = TaskProgressGuard::start(
        local_task_id.into(),
        state.task_repository.clone(),
        state.task_event_pipeline.clone(),
        Vec::new(),
    );
    let preparation_progress = preparation_guard.sink.clone();
    let preparation = prepare_deployment(
        state,
        local_project_id,
        local_task_id,
        snapshot,
        plan_input,
        &cancellation,
        preparation_progress.clone(),
    )
    .await;
    let progress_result = preparation_guard.stop().await;
    let prepared = match (preparation, progress_result) {
        (Err(error), _) => {
            record_preparation_failure(state, local_task_id, &error).await;
            return Err(error);
        }
        (Ok(_), Err(error)) => {
            record_preparation_failure(state, local_task_id, &error).await;
            return Err(error);
        }
        (Ok(prepared), Ok(())) => prepared,
    };
    let PreparedDeployment {
        operator,
        instance_id,
        pools,
        plan,
        prepared,
        secret_values,
    } = prepared;
    let lifecycle = AioExecutionLifecycle {
        state,
        local_project_id,
        local_task_id,
        operator: &operator,
        instance_id: &instance_id,
        pools,
        plan,
        prepared,
        secret_values,
        cancellation: cancellation.clone(),
    };
    ExecutionCoordinator.run(&lifecycle, cancellation).await
}

#[allow(clippy::too_many_arguments)]
async fn prepare_deployment(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    snapshot: DeploymentExecutionSnapshot,
    mut plan_input: DeploymentPlanInput,
    cancellation: &CancellationToken,
    progress: Arc<TaskProgressReporter>,
) -> AppResult<PreparedDeployment> {
    check_deployment_cancelled(cancellation)?;
    let target_macs = plan_input.target_macs.clone();
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_config",
        1,
        TaskEventLevel::Info,
        "DEPLOYMENT_PREPARATION_STARTED",
        "部署任务已开始，正在读取项目和发布配置",
    )?;
    let operator = project_operator(state, local_project_id).await?;
    let instance_id = application_instance_id().to_string();
    let pools = project_database(state, local_project_id).await?;
    let local_projects =
        LocalProjectRepository::new(state.local_store.pool().clone(), state.secret_store.clone());
    let connection = local_projects
        .connection_secrets(local_project_id)
        .await
        .map_err(map_formal_error)?;
    let profile = ReleaseProfileRepository::new(pools.workbench.clone())
        .get("default")
        .await
        .map_err(map_formal_error)?;
    check_deployment_cancelled(cancellation)?;
    if profile.credentials_reset_required {
        return Err(AppError::InvalidConfig(
            "发布凭据加密格式已更新，请重新填写发布参数".into(),
        ));
    }
    if profile.version != snapshot.profile_version {
        return Err(AppError::Conflict(format!(
            "发布参数已从v{}变更为v{}，任务未执行；请重新预检",
            snapshot.profile_version, profile.version
        )));
    }
    let agent = effective_agent_asset(profile.agent_script.as_ref())?;
    let published_ports = inspect_compose_published_ports(
        &profile.values.compose_template,
        &profile.values.env_template,
    )?;
    state
        .task_event_pipeline
        .register_secrets(release_secret_values(&connection.db_password, &profile))?;
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_config",
        2,
        TaskEventLevel::Info,
        "DEPLOYMENT_CONFIG_READY",
        "项目连接、发布参数和部署模板读取完成",
    )?;
    let task_dir = state
        .paths
        .project_task_dir(local_project_id, local_task_id)
        .map_err(map_formal_error)?;
    std::fs::create_dir_all(&task_dir).map_err(|error| AppError::io("创建部署任务目录", &error))?;
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_artifact",
        2,
        TaskEventLevel::Info,
        "LOCAL_ARTIFACT_STARTED",
        "正在生成本次任务的镜像快照",
    )?;
    let artifact_task_dir = task_dir.clone();
    let expected_fingerprint = snapshot.artifact_fingerprint.clone();
    let artifact_cancellation = cancellation.clone();
    let artifact_targets = target_macs.clone();
    let artifact_progress = progress.clone();
    let (plan_input, release_manifest) = tokio::task::spawn_blocking(move || {
        let mut last_progress = 2_u64;
        let mut last_reported_bytes = 0_u64;
        let release_manifest = materialize_artifact_snapshot(
            &mut plan_input,
            &artifact_task_dir,
            &expected_fingerprint,
            &artifact_cancellation,
            |processed, total, service| {
                let current = 2_u64
                    .saturating_add(4_u64.saturating_mul(processed) / total.max(1))
                    .min(6);
                if current > last_progress
                    || (processed == total && last_reported_bytes != total)
                    || processed.saturating_sub(last_reported_bytes) >= 32 * 1024 * 1024
                {
                    last_progress = current;
                    last_reported_bytes = processed;
                    if let Some(target) = artifact_targets.first() {
                        let stage = format!(
                            "复制镜像 {service} {}/{}",
                            display_bytes(processed),
                            display_bytes(total)
                        );
                        emit_target_preparation_progress(
                            artifact_progress.as_ref(),
                            target,
                            &stage,
                            current,
                            TaskEventLevel::Info,
                            "LOCAL_ARTIFACT_PROGRESS",
                            &stage,
                        )?;
                    }
                }
                Ok(())
            },
        )?;
        Ok::<_, AppError>((plan_input, release_manifest))
    })
    .await
    .map_err(|_| AppError::Io {
        operation: "等待发布物快照任务",
    })??;
    check_deployment_cancelled(cancellation)?;
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_artifact",
        6,
        TaskEventLevel::Info,
        "LOCAL_ARTIFACT_COMPLETED",
        "本次任务的镜像快照已生成并完成完整性校验",
    )?;
    let plan = DeploymentPlan::build(plan_input)?;
    let release_fingerprint = snapshot.artifact_fingerprint.clone();
    let nodes = snapshot
        .targets
        .into_iter()
        .map(|target| (target.node.mac_normalized.clone(), target))
        .collect::<HashMap<String, DeploymentTargetSnapshot>>();
    let agent_path = task_dir.join("edge-node-agent.sh");
    std::fs::write(&agent_path, &agent.content)
        .map_err(|error| AppError::io("写入内嵌Agent", &error))?;
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_release",
        6,
        TaskEventLevel::Info,
        "LOCAL_RELEASE_ARCHIVE_STARTED",
        "正在生成待上传的发布包",
    )?;
    let archive_plan = plan.clone();
    let archive_task_dir = task_dir.clone();
    let archive_cancellation = cancellation.clone();
    let archive_targets = target_macs.clone();
    let archive_progress = progress.clone();
    let shared_artifact = tokio::task::spawn_blocking(move || {
        let mut last_progress = 6_u64;
        let mut last_reported_bytes = 0_u64;
        prepare_shared_artifact(
            &archive_plan,
            &archive_task_dir,
            &archive_cancellation,
            |processed, total| {
                let current = 6_u64
                    .saturating_add(2_u64.saturating_mul(processed) / total.max(1))
                    .min(8);
                if current > last_progress
                    || (processed == total && last_reported_bytes != total)
                    || processed.saturating_sub(last_reported_bytes) >= 32 * 1024 * 1024
                {
                    last_progress = current;
                    last_reported_bytes = processed;
                    if let Some(target) = archive_targets.first() {
                        let stage = format!(
                            "生成发布包 {}/{}",
                            display_bytes(processed),
                            display_bytes(total)
                        );
                        emit_target_preparation_progress(
                            archive_progress.as_ref(),
                            target,
                            &stage,
                            current,
                            TaskEventLevel::Info,
                            "LOCAL_ARCHIVE_PROGRESS",
                            &stage,
                        )?;
                    }
                }
                Ok(())
            },
        )
    })
    .await
    .map_err(|_| AppError::Io {
        operation: "等待生成发布包任务",
    })??;
    check_deployment_cancelled(cancellation)?;
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_release",
        8,
        TaskEventLevel::Info,
        "LOCAL_RELEASE_ARCHIVE_COMPLETED",
        "待上传发布包生成完成",
    )?;
    let mut prepared = HashMap::new();
    for mac in &plan.target_macs {
        check_deployment_cancelled(cancellation)?;
        let target_snapshot = nodes
            .get(mac)
            .ok_or_else(|| AppError::NotFound(format!("部署目标快照不存在：{mac}")))?;
        let node = &target_snapshot.node;
        emit_target_preparation_progress(
            progress.as_ref(),
            mac,
            "prepare_target",
            8,
            TaskEventLevel::Info,
            "TARGET_RENDER_STARTED",
            &format!("正在为 {}（{}）渲染部署配置", node.name, node.ip),
        )?;
        let target = RemoteTarget {
            host: target_snapshot.ssh_host.clone(),
            port: target_snapshot.ssh_port,
            connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
        };
        let node_dir = task_dir.join(mac);
        std::fs::create_dir_all(&node_dir)
            .map_err(|error| AppError::io("创建节点渲染目录", &error))?;
        let (env, host_info, compose) = if let Some(manifest) = release_manifest.as_ref() {
            let context = render_context(node, &profile, manifest);
            let rendered = render_release_templates(
                &profile.values.env_template,
                &profile.values.host_info_template,
                &profile.values.compose_template,
                &context,
            )?;
            let env = node_dir.join(".env");
            let host = node_dir.join("host-info.json");
            let compose = node_dir.join("docker-compose.yml");
            std::fs::write(&env, rendered.env)
                .map_err(|error| AppError::io("写入节点env", &error))?;
            std::fs::write(&host, rendered.host_info_json)
                .map_err(|error| AppError::io("写入节点host-info", &error))?;
            std::fs::write(&compose, rendered.compose_runtime)
                .map_err(|error| AppError::io("写入节点Compose", &error))?;
            (Some(env), Some(host), Some(compose))
        } else {
            (None, None, None)
        };
        let auth = release_remote_auth(&profile.credentials)?;
        prepared.insert(
            mac.clone(),
            PreparedNode {
                target,
                auth,
                files: RemoteDeploymentFiles {
                    local_agent: agent_path.clone(),
                    local_artifact: shared_artifact.clone(),
                    local_env: env,
                    local_host_info: host_info,
                    local_compose: compose,
                },
                config: RemoteDeploymentConfig {
                    operation_id: String::new(),
                    release_fingerprint: release_fingerprint.clone(),
                    mac_normalized: mac.clone(),
                    data_root: profile.values.aio_data_root.clone(),
                    deploy_root: profile.values.aio_deploy_root.clone(),
                    platform_api_host: profile.values.platform_host.clone(),
                    platform_api_port: profile.values.platform_api_port,
                    platform_mqtt_host: profile.values.platform_mqtt_host.clone(),
                    platform_mqtt_port: profile.values.platform_mqtt_port,
                    allow_existing_ports: plan.mode == DeploymentMode::FullUpgrade,
                    published_ports: published_ports.clone(),
                },
                registration: AioRegistrationPayload {
                    name: node.name.clone(),
                    ip: node.ip.clone(),
                    mac: node.mac_normalized.clone(),
                    platform_ip: profile.values.platform_host.clone(),
                    platform_port: profile.values.platform_api_port.to_string(),
                    auth_key: profile.credentials.platform_auth_key.clone(),
                    building_id: node
                        .building_id
                        .as_deref()
                        .and_then(|value| value.parse().ok()),
                    addr_alias: node.addr_alias.clone(),
                },
            },
        );
        emit_target_preparation_progress(
            progress.as_ref(),
            mac,
            "prepare_target",
            9,
            TaskEventLevel::Info,
            "TARGET_RENDER_COMPLETED",
            &format!("{}（{}）部署配置渲染完成", node.name, node.ip),
        )?;
    }
    emit_preparation_progress(
        progress.as_ref(),
        &target_macs,
        "prepare_lease",
        9,
        TaskEventLevel::Info,
        "DEPLOYMENT_LOCAL_PREPARATION_COMPLETED",
        "本地发布文件准备完成，等待创建共享操作和申请执行租约",
    )?;
    Ok(PreparedDeployment {
        operator,
        instance_id,
        pools,
        plan,
        prepared,
        secret_values: release_secret_values(&connection.db_password, &profile),
    })
}

async fn begin_deployment_preparation(
    state: &FormalAppState,
    local_task_id: &str,
    target_macs: &[String],
) -> AppResult<()> {
    let task = state.task_repository.get(local_task_id).await?;
    match task.state {
        TaskState::Queued => {
            state
                .task_repository
                .transition(
                    local_task_id,
                    TaskState::Queued,
                    TaskState::Running,
                    None,
                    None,
                )
                .await?;
        }
        TaskState::Running => {}
        state => {
            return Err(AppError::Conflict(format!(
                "部署任务状态不允许开始准备：{}",
                state.as_str()
            )));
        }
    }
    for mac in target_macs {
        state
            .task_repository
            .update_target(
                local_task_id,
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: mac.clone(),
                    state: TargetState::Running,
                    stage: "prepare_config".into(),
                    progress_current: 0,
                    progress_total: DEPLOYMENT_PROGRESS_TOTAL,
                    fencing_token: None,
                    message_code: Some("DEPLOYMENT_PREPARATION_STARTED".into()),
                    message_params_json: None,
                },
            )
            .await?;
    }
    Ok(())
}

fn emit_preparation_progress(
    progress: &TaskProgressReporter,
    target_macs: &[String],
    stage: &str,
    current: u64,
    level: TaskEventLevel,
    message_code: &str,
    message: &str,
) -> AppResult<()> {
    for mac in target_macs {
        emit_target_preparation_progress(
            progress,
            mac,
            stage,
            current,
            level,
            message_code,
            message,
        )?;
    }
    Ok(())
}

fn emit_target_preparation_progress(
    progress: &TaskProgressReporter,
    mac: &str,
    stage: &str,
    current: u64,
    level: TaskEventLevel,
    message_code: &str,
    message: &str,
) -> AppResult<()> {
    progress.emit(DeploymentProgressEvent {
        mac: mac.into(),
        stage: stage.into(),
        step_code: None,
        step_state: None,
        target_state: TargetState::Running,
        progress_current: current,
        progress_total: DEPLOYMENT_PROGRESS_TOTAL,
        level,
        message_code: message_code.into(),
        message: Some(message.into()),
    })
}

fn check_deployment_cancelled(cancellation: &CancellationToken) -> AppResult<()> {
    if cancellation.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn display_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    if bytes >= GIB {
        format!("{}.{:01} GB", bytes / GIB, bytes % GIB * 10 / GIB)
    } else if bytes >= MIB {
        format!("{} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{} KB", bytes / KIB)
    } else {
        format!("{bytes} B")
    }
}

async fn record_preparation_failure(state: &FormalAppState, local_task_id: &str, error: &AppError) {
    let cancelled = matches!(error, AppError::Cancelled);
    let _ = state
        .task_event_pipeline
        .emit(
            local_task_id,
            TaskEventInput {
                resource_type: None,
                resource_key: None,
                stage: "准备部署任务".into(),
                status: if cancelled { "cancelled" } else { "failed" }.into(),
                progress_current: None,
                progress_total: None,
                level: if cancelled {
                    TaskEventLevel::Warn
                } else {
                    TaskEventLevel::Error
                },
                message_code: if cancelled {
                    "DEPLOYMENT_PREPARATION_CANCELLED"
                } else {
                    "DEPLOYMENT_PREPARATION_FAILED"
                }
                .into(),
                message_params: BTreeMap::new(),
                message: Some(if cancelled {
                    "部署任务准备已取消".into()
                } else {
                    format!("部署任务准备失败：{error}")
                }),
            },
        )
        .await;
}

async fn record_deployment_control_event(
    state: &FormalAppState,
    local_task_id: &str,
    stage: &str,
    status: &str,
    level: TaskEventLevel,
    message_code: &str,
    message: &str,
) -> AppResult<()> {
    state
        .task_event_pipeline
        .emit(
            local_task_id,
            TaskEventInput {
                resource_type: None,
                resource_key: None,
                stage: stage.into(),
                status: status.into(),
                progress_current: None,
                progress_total: None,
                level,
                message_code: message_code.into(),
                message_params: BTreeMap::new(),
                message: Some(message.into()),
            },
        )
        .await
        .map(|_| ())
}

fn normalize_target_macs(input: &mut DeploymentPlanInput) -> AppResult<()> {
    input.target_macs = input
        .target_macs
        .iter()
        .map(|mac| MacAddress::parse(mac).map(|value| value.normalized().to_string()))
        .collect::<AppResult<Vec<_>>>()?;
    Ok(())
}

fn release_secret_values(database_password: &str, profile: &ReleaseProfileRecord) -> Vec<String> {
    let mut values = vec![
        database_password.to_string(),
        profile.credentials.platform_auth_key.clone(),
        profile.credentials.platform_mqtt_user.clone(),
        profile.credentials.platform_mqtt_password.clone(),
        profile.credentials.aio_mqtt_user.clone(),
        profile.credentials.aio_mqtt_password.clone(),
        profile.credentials.ssh_user.clone(),
    ];
    values.extend(profile.credentials.ssh_password.clone());
    values.extend(profile.credentials.ssh_private_key.clone());
    values
}

async fn converge_submission_failure(
    state: &FormalAppState,
    local_task_id: &str,
    error: &AppError,
    cancellation_requested: bool,
) {
    let message = error.to_string();
    let task_before_convergence = state.task_repository.get(local_task_id).await.ok();
    let cancelled = cancellation_requested
        || matches!(error, AppError::Cancelled)
        || task_before_convergence
            .as_ref()
            .is_some_and(|task| matches!(task.state, TaskState::Cancelling | TaskState::Cancelled));
    let target_state = if cancelled {
        TargetState::Cancelled
    } else {
        TargetState::Failed
    };
    let target_stage = if cancelled {
        "cancelled"
    } else {
        "preparation_failed"
    };
    let message_code = if cancelled {
        "DEPLOYMENT_CANCELLED"
    } else {
        "DEPLOYMENT_PREPARATION_FAILED"
    };
    if let Ok(targets) = state.task_repository.targets(local_task_id).await {
        for target in targets {
            if matches!(
                target.state,
                TargetState::Pending | TargetState::Running | TargetState::Unknown
            ) {
                let _ = state
                    .task_repository
                    .update_target(
                        local_task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: target_state,
                            stage: target_stage.into(),
                            progress_current: target
                                .progress_current
                                .min(target.progress_total.max(100)),
                            progress_total: target.progress_total.max(100),
                            fencing_token: target.fencing_token,
                            message_code: Some(message_code.into()),
                            message_params_json: None,
                        },
                    )
                    .await;
            }
        }
    }
    let Ok(task) = state.task_repository.get(local_task_id).await else {
        return;
    };
    match task.state {
        TaskState::Draft => {
            if state
                .task_repository
                .transition(
                    local_task_id,
                    TaskState::Draft,
                    TaskState::Checking,
                    None,
                    None,
                )
                .await
                .is_ok()
            {
                let _ = state
                    .task_repository
                    .transition(
                        local_task_id,
                        TaskState::Checking,
                        TaskState::CheckFailed,
                        Some("DEPLOYMENT_SUBMISSION_FAILED"),
                        Some(&message),
                    )
                    .await;
            }
        }
        TaskState::Checking => {
            let _ = state
                .task_repository
                .transition(
                    local_task_id,
                    TaskState::Checking,
                    TaskState::CheckFailed,
                    Some("DEPLOYMENT_SUBMISSION_FAILED"),
                    Some(&message),
                )
                .await;
        }
        TaskState::Queued => {
            if cancelled {
                let _ = state
                    .task_repository
                    .transition(
                        local_task_id,
                        TaskState::Queued,
                        TaskState::Cancelled,
                        Some(message_code),
                        Some(&message),
                    )
                    .await;
            } else if state
                .task_repository
                .transition(
                    local_task_id,
                    TaskState::Queued,
                    TaskState::Running,
                    None,
                    None,
                )
                .await
                .is_ok()
            {
                let _ = state
                    .task_repository
                    .transition(
                        local_task_id,
                        TaskState::Running,
                        TaskState::Failed,
                        Some(message_code),
                        Some(&message),
                    )
                    .await;
            }
        }
        TaskState::Running | TaskState::Cancelling => {
            let _ = state
                .task_repository
                .transition(
                    local_task_id,
                    task.state,
                    if cancelled {
                        TaskState::Cancelled
                    } else {
                        TaskState::Failed
                    },
                    Some(message_code),
                    Some(&message),
                )
                .await;
        }
        _ => {}
    }
}

async fn validate_fencing(
    pool: &sqlx::MySqlPool,
    lease: &crate::formal::resource_lease_repository::LeaseGrant,
) -> AppResult<()> {
    if ResourceLeaseRepository::new(pool.clone())
        .validate_fencing(lease)
        .await
        .map_err(map_formal_error)?
    {
        Ok(())
    } else {
        Err(AppError::Conflict(format!(
            "资源租约已失效，停止远端步骤：{}/{}",
            lease.resource_type, lease.resource_key
        )))
    }
}

#[derive(Clone)]
struct PreparedNode {
    target: RemoteTarget,
    auth: RemoteAuth,
    files: RemoteDeploymentFiles,
    config: RemoteDeploymentConfig,
    registration: AioRegistrationPayload,
}

struct PreparedDeployment {
    operator: String,
    instance_id: String,
    pools: Arc<DualMySqlPools>,
    plan: DeploymentPlan,
    prepared: HashMap<String, PreparedNode>,
    secret_values: Vec<String>,
}

struct AioExecutionLifecycle<'a> {
    state: &'a FormalAppState,
    local_project_id: &'a str,
    local_task_id: &'a str,
    operator: &'a str,
    instance_id: &'a str,
    pools: Arc<DualMySqlPools>,
    plan: DeploymentPlan,
    prepared: HashMap<String, PreparedNode>,
    secret_values: Vec<String>,
    cancellation: CancellationToken,
}

impl ExecutionLifecyclePort for AioExecutionLifecycle<'_> {
    type Handle = crate::infrastructure::deployment_control::DeploymentControlHandle;
    type Summary = DeploymentExecutionSummary;
    type Heartbeat = crate::infrastructure::deployment_control::DeploymentHeartbeatGuard;

    async fn start(&self) -> AppResult<Self::Handle> {
        record_deployment_control_event(
            self.state,
            self.local_task_id,
            "prepare_lease",
            "running",
            TaskEventLevel::Info,
            "DEPLOYMENT_LEASE_STARTED",
            "正在创建共享部署操作并申请一体机执行租约",
        )
        .await?;
        let start = start_deployment_control(
            self.state,
            self.local_project_id,
            self.local_task_id,
            self.plan.clone(),
            self.operator,
            self.instance_id,
            &self.cancellation,
        )
        .await;
        match &start {
            Ok(_) => {
                let _ = record_deployment_control_event(
                    self.state,
                    self.local_task_id,
                    "lease_acquired",
                    "succeeded",
                    TaskEventLevel::Info,
                    "DEPLOYMENT_LEASE_COMPLETED",
                    "共享部署操作和一体机执行租约已就绪",
                )
                .await;
            }
            Err(error) => {
                let cancelled = self
                    .state
                    .task_repository
                    .get(self.local_task_id)
                    .await
                    .is_ok_and(|task| {
                        matches!(task.state, TaskState::Cancelling | TaskState::Cancelled)
                    });
                let (stage, status, level, message_code, message) = if cancelled {
                    (
                        "cancelled",
                        "cancelled",
                        TaskEventLevel::Warn,
                        "DEPLOYMENT_START_CANCELLED",
                        "部署任务在创建共享操作或申请执行租约期间已取消".to_string(),
                    )
                } else {
                    (
                        "prepare_lease_failed",
                        "failed",
                        TaskEventLevel::Error,
                        "DEPLOYMENT_LEASE_FAILED",
                        format!("创建共享部署操作或申请执行租约失败：{error}"),
                    )
                };
                let _ = record_deployment_control_event(
                    self.state,
                    self.local_task_id,
                    stage,
                    status,
                    level,
                    message_code,
                    &message,
                )
                .await;
            }
        }
        start
    }

    fn start_heartbeat(
        &self,
        handle: &Self::Handle,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Self::Heartbeat {
        start_deployment_heartbeat(
            self.pools.workbench.clone(),
            handle.operation_id.clone(),
            handle.operation_version,
            handle.leases.clone(),
            cancellation,
        )
    }

    async fn execute(
        &self,
        handle: &Self::Handle,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> AppResult<Self::Summary> {
        let check_started_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| AppError::InvalidConfig("生成服务检查时间失败".into()))?;
        let mut prepared = self.prepared.clone();
        for node in prepared.values_mut() {
            node.config.operation_id = handle.operation_id.clone();
        }
        let progress_guard = TaskProgressGuard::start_scaled(
            handle.local_task_id.clone(),
            self.state.task_repository.clone(),
            self.state.task_event_pipeline.clone(),
            self.secret_values.clone(),
            DEPLOYMENT_REMOTE_PROGRESS_START,
            DEPLOYMENT_PROGRESS_TOTAL,
            DEPLOYMENT_PROGRESS_TOTAL,
        )?;
        let progress_sink = progress_guard.sink.clone();
        let execution = execute_prepared_targets(
            self.plan.clone(),
            self.pools.workbench.clone(),
            prepared,
            handle,
            cancellation,
            progress_sink.clone(),
            ObservedConnector::new(
                RusshConnector::default(),
                HostKeyRepository::new(self.state.local_store.pool().clone()),
                self.local_project_id,
            ),
        )
        .await;
        let progress = progress_guard.stop().await;
        let reports = progress_sink.service_checks().unwrap_or_default();
        let check_repository = ServiceCheckRepository::new(
            self.state.local_store.pool().clone(),
            self.local_project_id,
        );
        for mac in &self.plan.target_macs {
            let report = match reports.get(mac) {
                Some(report) => report.clone(),
                None => ServiceCheckReport {
                    started_at: check_started_at.clone(),
                    checked_at: time::OffsetDateTime::now_utc()
                        .format(&time::format_description::well_known::Rfc3339)
                        .map_err(|_| AppError::InvalidConfig("生成服务检查时间失败".into()))?,
                    source: crate::infrastructure::deployment_remote::deployment_check_source(
                        self.plan.mode,
                    )
                    .into(),
                    scope: if self.plan.mode == DeploymentMode::ServiceUpgrade {
                        "service"
                    } else {
                        "all"
                    }
                    .into(),
                    service_name: self.plan.service_name.clone(),
                    expected_services: Vec::new(),
                    services: Vec::new(),
                    state: "failed".into(),
                    error: Some("本次部署未取得真实服务检查报告，保留上次有效检查记录".into()),
                },
            };
            if let Err(error) = check_repository.save_report(mac, &report).await {
                let _ = self
                    .state
                    .task_event_pipeline
                    .emit(
                        &handle.local_task_id,
                        TaskEventInput {
                            resource_type: Some("aio".into()),
                            resource_key: Some(mac.clone()),
                            stage: "service_check".into(),
                            status: "warning".into(),
                            progress_current: None,
                            progress_total: None,
                            level: TaskEventLevel::Warn,
                            message_code: "SERVICE_CHECK_SAVE_FAILED".into(),
                            message_params: BTreeMap::new(),
                            message: Some(format!("服务检查结果保存到本机失败：{error}")),
                        },
                    )
                    .await;
            }
        }
        match (execution, progress) {
            (Ok(summary), Ok(())) => Ok(summary),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }

    async fn stop_heartbeat(
        &self,
        heartbeat: Self::Heartbeat,
        handle: &mut Self::Handle,
    ) -> AppResult<()> {
        match heartbeat.stop().await {
            Ok(version) => {
                handle.operation_version = version;
                Ok(())
            }
            Err(error) => {
                let _ = self
                    .state
                    .runtime_registry
                    .close(self.local_project_id)
                    .await;
                if let Ok(runtime) = self
                    .state
                    .runtime_registry
                    .open(self.local_project_id)
                    .await
                {
                    runtime.set_health(ConnectionHealth::Degraded).await;
                }
                Err(error)
            }
        }
    }

    async fn finalize(&self, handle: &Self::Handle, summary: &Self::Summary) -> AppResult<()> {
        finalize_deployment_control(
            self.state,
            self.local_project_id,
            handle.clone(),
            summary.clone(),
        )
        .await
    }

    async fn mark_interrupted(
        &self,
        handle: &Self::Handle,
        error_code: &str,
        message: &str,
    ) -> AppResult<()> {
        mark_deployment_interrupted(self.state, handle, error_code, message).await
    }

    async fn mark_finalizing_failed(&self, handle: &Self::Handle, message: &str) -> AppResult<()> {
        mark_deployment_finalizing_failed(self.state, handle, message).await
    }
}

async fn execute_prepared_targets(
    plan: DeploymentPlan,
    workbench_pool: sqlx::MySqlPool,
    prepared: HashMap<String, PreparedNode>,
    handle: &crate::infrastructure::deployment_control::DeploymentControlHandle,
    cancellation: tokio_util::sync::CancellationToken,
    progress_sink: Arc<TaskProgressReporter>,
    connector: ObservedConnector<RusshConnector>,
) -> AppResult<DeploymentExecutionSummary> {
    let lease_by_mac = Arc::new(
        handle
            .leases
            .iter()
            .cloned()
            .map(|lease| (lease.resource_key.clone(), lease))
            .collect::<HashMap<_, _>>(),
    );
    let prepared = Arc::new(prepared);
    let global_slots = global_remote_node_slots();
    let plan_for_worker = plan.clone();
    execute_deployment_targets(plan, cancellation.clone(), {
        let prepared = prepared.clone();
        let lease_by_mac = lease_by_mac.clone();
        let progress_sink = progress_sink.clone();
        move |mac, cancellation| {
            let prepared = prepared.clone();
            let lease_by_mac = lease_by_mac.clone();
            let workbench_pool = workbench_pool.clone();
            let connector = connector.clone();
            let global_slots = global_slots.clone();
            let plan = plan_for_worker.clone();
            let progress_sink = progress_sink.clone();
            async move {
                let _global_permit = tokio::select! {
                    _ = cancellation.cancelled() => return Err(AppError::Cancelled),
                    permit = global_slots.acquire_owned() => {
                        permit.map_err(|_| AppError::Conflict("全局远端节点并发控制器已关闭".into()))?
                    }
                };
                let execution = async {
                    let node = prepared
                        .get(&mac)
                        .ok_or_else(|| AppError::NotFound(format!("缺少节点执行上下文：{mac}")))?;
                    let lease = lease_by_mac
                        .get(&mac)
                        .cloned()
                        .ok_or_else(|| AppError::Conflict(format!("缺少节点租约：{mac}")))?;
                    validate_fencing(&workbench_pool, &lease).await?;
                    let change_progress = progress_sink.clone();
                    let change_mac = mac.clone();
                    let connector = connector.with_change_handler(Arc::new(move |observation| {
                        change_progress.emit(DeploymentProgressEvent {
                            mac: change_mac.clone(),
                            stage: "ssh_connect".into(), step_code: None, step_state: None,
                            target_state: TargetState::Running, progress_current: 0, progress_total: 100,
                            level: TaskEventLevel::Warn,
                            message_code: "SSH_HOST_KEY_CHANGED".into(),
                            message: Some(observation.message()),
                        })
                    }));
                    execute_remote_deployment_observed_with_checkpoint(
                        &connector,
                        &node.target,
                        &node.auth,
                        HostKeyPolicy::Capture,
                        &plan,
                        &node.files,
                        &node.config,
                        &cancellation,
                        {
                            let workbench_pool = workbench_pool.clone();
                            let lease = lease.clone();
                            move || {
                                let workbench_pool = workbench_pool.clone();
                                let lease = lease.clone();
                                async move { validate_fencing(&workbench_pool, &lease).await }
                            }
                        },
                        progress_sink.as_ref(),
                    )
                    .await?;
                    validate_fencing(&workbench_pool, &lease).await?;
                    if plan.mode == DeploymentMode::FirstDeploy {
                        progress_sink.emit(DeploymentProgressEvent {
                            mac: mac.clone(),
                            stage: "register".into(),
                            step_code: Some("register".into()),
                            step_state: Some(StepState::Running),
                            target_state: TargetState::Running,
                            progress_current: 99,
                            progress_total: 100,
                            level: TaskEventLevel::Info,
                            message_code: "DEVICE_REGISTRATION_STARTED".into(),
                            message: Some("调用一体机本地注册接口".into()),
                        })?;
                        DeviceApiClient::new(
                            &format!("http://{}:6002", node.target.host),
                            &node.registration.auth_key,
                        )?
                        .register_if_missing_with_retry(
                            &node.registration,
                            30,
                            Duration::from_secs(2),
                        )
                        .await?;
                        progress_sink.emit(DeploymentProgressEvent {
                            mac: mac.clone(),
                            stage: "register".into(),
                            step_code: Some("register".into()),
                            step_state: Some(StepState::Succeeded),
                            target_state: TargetState::Running,
                            progress_current: 100,
                            progress_total: 100,
                            level: TaskEventLevel::Info,
                            message_code: "DEVICE_REGISTRATION_COMPLETED".into(),
                            message: Some("一体机本地注册接口确认完成".into()),
                        })?;
                    }
                    progress_sink.emit(DeploymentProgressEvent {
                        mac: mac.clone(),
                        stage: "finalizing".into(),
                        step_code: None,
                        step_state: None,
                        target_state: TargetState::Running,
                        progress_current: 100,
                        progress_total: 100,
                        level: TaskEventLevel::Info,
                        message_code: "TARGET_REMOTE_COMPLETED".into(),
                        message: Some("远端步骤完成，正在写入最终结果".into()),
                    })?;
                    Ok(())
                }
                .await;
                if let Err(error) = &execution {
                    let cancelled = matches!(error, AppError::Cancelled);
                    let _ = progress_sink.emit(DeploymentProgressEvent {
                        mac: mac.clone(),
                        stage: if cancelled {
                            "cancelled".into()
                        } else {
                            "failed".into()
                        },
                        step_code: None,
                        step_state: None,
                        target_state: if cancelled {
                            TargetState::Cancelled
                        } else {
                            TargetState::Failed
                        },
                        progress_current: 100,
                        progress_total: 100,
                        level: if cancelled {
                            TaskEventLevel::Warn
                        } else {
                            TaskEventLevel::Error
                        },
                        message_code: if cancelled {
                            "TARGET_CANCELLED".into()
                        } else {
                            "TARGET_EXECUTION_FAILED".into()
                        },
                        message: Some(error.to_string()),
                    });
                }
                execution
            }
        }
    })
    .await
}

fn materialize_artifact_snapshot(
    input: &mut DeploymentPlanInput,
    task_dir: &Path,
    expected_fingerprint: &str,
    cancellation: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64, &str) -> AppResult<()>,
) -> AppResult<Option<ReleaseManifest>> {
    input
        .image_files
        .sort_by(|left, right| left.service_name.cmp(&right.service_name));
    let total_bytes = input.image_files.iter().try_fold(0_u64, |total, image| {
        let source = PathBuf::from(&image.file_path);
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| AppError::io("读取镜像文件属性", &error))?;
        if metadata.file_type().is_symlink() {
            return Err(AppError::InvalidConfig("镜像文件不允许使用符号链接".into()));
        }
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(AppError::InvalidConfig(format!(
                "服务 {} 的镜像文件为空或不是文件",
                image.service_name
            )));
        }
        Ok(total.saturating_add(metadata.len()))
    })?;
    let mut copied_bytes = 0_u64;
    let mut fingerprint_entries = Vec::with_capacity(input.image_files.len());
    if input.mode == DeploymentMode::ServiceUpgrade {
        let source = PathBuf::from(&input.image_files[0].file_path);
        let temporary = task_dir.join("service-image.tar.part");
        let destination = task_dir.join("service-image.tar");
        if temporary.exists() || destination.exists() {
            return Err(AppError::Conflict("任务发布物快照已存在".into()));
        }
        let service = input.image_files[0].service_name.clone();
        let sha256 = copy_snapshot_file(
            &source,
            &temporary,
            cancellation,
            &mut copied_bytes,
            total_bytes,
            &service,
            &mut on_progress,
        )?;
        fingerprint_entries.push((service, input.image_files[0].image_tag.clone(), sha256));
        verify_snapshot_fingerprint(&fingerprint_entries, expected_fingerprint)?;
        std::fs::rename(&temporary, &destination)
            .map_err(|error| AppError::io("发布单服镜像快照", &error))?;
        input.artifact_path = destination.to_string_lossy().into_owned();
        input.image_files[0].file_path = input.artifact_path.clone();
        return Ok(None);
    }

    let temporary = task_dir.join("generated-release.part");
    let destination = task_dir.join("generated-release");
    if temporary.exists() || destination.exists() {
        return Err(AppError::Conflict("任务Release快照已存在".into()));
    }
    std::fs::create_dir_all(temporary.join("images"))
        .map_err(|error| AppError::io("创建内部发布包目录", &error))?;
    let mut manifest_images = Vec::with_capacity(input.image_files.len());
    let mut checksum_entries = Vec::with_capacity(input.image_files.len() * 2);
    for image in &mut input.image_files {
        check_deployment_cancelled(cancellation)?;
        let source = PathBuf::from(&image.file_path);
        let file_name = format!("{}.tar", image.service_name);
        let target = temporary.join("images").join(&file_name);
        let sha256 = copy_snapshot_file(
            &source,
            &target,
            cancellation,
            &mut copied_bytes,
            total_bytes,
            &image.service_name,
            &mut on_progress,
        )?;
        fingerprint_entries.push((
            image.service_name.clone(),
            image.image_tag.clone(),
            sha256.clone(),
        ));
        checksum_entries.push(format!("{sha256}  images/{file_name}"));
        let tag_name = format!("{}.tag", image.service_name);
        let tag_target = temporary.join("images").join(&tag_name);
        std::fs::write(&tag_target, format!("{}\n", image.image_tag))
            .map_err(|error| AppError::io("写入内部镜像标签", &error))?;
        checksum_entries.push(format!("{}  images/{tag_name}", sha256_file(&tag_target)?));
        image.file_path = target.to_string_lossy().into_owned();
        manifest_images.push(ReleaseImage {
            service: image.service_name.clone(),
            image: image.image_tag.clone(),
            archive: format!("images/{file_name}"),
            sha256,
        });
    }
    if let Err(error) = verify_snapshot_fingerprint(&fingerprint_entries, expected_fingerprint) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    let manifest = ReleaseManifest {
        schema_version: 1,
        version: input.artifact_version.clone(),
        images: manifest_images,
    };
    std::fs::write(
        temporary.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|_| AppError::InvalidConfig("生成内部发布清单失败".into()))?,
    )
    .map_err(|error| AppError::io("写入内部发布清单", &error))?;
    std::fs::create_dir_all(temporary.join("checksums"))
        .map_err(|error| AppError::io("创建内部校验目录", &error))?;
    let checksums = checksum_entries.join("\n");
    std::fs::write(
        temporary.join("checksums").join("sha256.txt"),
        format!("{checksums}\n"),
    )
    .map_err(|error| AppError::io("写入内部镜像校验清单", &error))?;
    std::fs::rename(&temporary, &destination)
        .map_err(|error| AppError::io("发布Release快照", &error))?;
    input.artifact_path = destination.to_string_lossy().into_owned();
    for image in &mut input.image_files {
        image.file_path = destination
            .join("images")
            .join(format!("{}.tar", image.service_name))
            .to_string_lossy()
            .into_owned();
    }
    Ok(Some(manifest))
}

#[allow(clippy::too_many_arguments)]
fn copy_snapshot_file(
    source: &Path,
    destination: &Path,
    cancellation: &CancellationToken,
    copied_bytes: &mut u64,
    total_bytes: u64,
    service: &str,
    on_progress: &mut impl FnMut(u64, u64, &str) -> AppResult<()>,
) -> AppResult<String> {
    let source_file =
        std::fs::File::open(source).map_err(|error| AppError::io("打开镜像快照源文件", &error))?;
    let destination_file = std::fs::File::create(destination)
        .map_err(|error| AppError::io("创建镜像任务快照", &error))?;
    let mut reader = BufReader::with_capacity(1024 * 1024, source_file);
    let mut writer = BufWriter::with_capacity(1024 * 1024, destination_file);
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        check_deployment_cancelled(cancellation)?;
        let count = reader
            .read(&mut buffer)
            .map_err(|error| AppError::io("读取镜像快照源文件", &error))?;
        if count == 0 {
            break;
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|error| AppError::io("写入镜像任务快照", &error))?;
        digest.update(&buffer[..count]);
        *copied_bytes = copied_bytes.saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
        on_progress(*copied_bytes, total_bytes.max(1), service)?;
    }
    writer
        .flush()
        .map_err(|error| AppError::io("完成镜像任务快照", &error))?;
    Ok(hex::encode(digest.finalize()))
}

fn verify_snapshot_fingerprint(
    entries: &[(String, String, String)],
    expected_fingerprint: &str,
) -> AppResult<()> {
    let mut entries = entries.to_vec();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (service, image_tag, sha256) in entries {
        digest.update(service.as_bytes());
        digest.update([0]);
        digest.update(image_tag.as_bytes());
        digest.update([0]);
        digest.update(sha256.as_bytes());
        digest.update([0]);
    }
    if hex::encode(digest.finalize()).eq_ignore_ascii_case(expected_fingerprint) {
        Ok(())
    } else {
        Err(AppError::Integrity {
            operation: "校验镜像任务快照",
        })
    }
}

fn prepare_shared_artifact(
    plan: &DeploymentPlan,
    task_dir: &Path,
    cancellation: &CancellationToken,
    mut on_progress: impl FnMut(u64, u64) -> AppResult<()>,
) -> AppResult<PathBuf> {
    if plan.mode == DeploymentMode::ServiceUpgrade {
        on_progress(1, 1)?;
        return Ok(PathBuf::from(&plan.artifact_path));
    }
    let output = task_dir.join("release.tar");
    create_generated_release_tar_observed(
        Path::new(&plan.artifact_path),
        &output,
        cancellation,
        on_progress,
    )
}

fn render_context(
    node: &crate::domain::aio::inventory::WorkbenchNodeSnapshot,
    profile: &crate::formal::release_profile_repository::ReleaseProfileRecord,
    manifest: &ReleaseManifest,
) -> ReleaseRenderContext {
    ReleaseRenderContext {
        release_version: manifest.version.clone(),
        platform_host: profile.values.platform_host.clone(),
        platform_api_port: profile.values.platform_api_port,
        platform_username: String::new(),
        platform_password: String::new(),
        platform_mqtt_host: profile.values.platform_mqtt_host.clone(),
        platform_mqtt_port: profile.values.platform_mqtt_port,
        platform_mqtt_user: profile.credentials.platform_mqtt_user.clone(),
        platform_mqtt_password: profile.credentials.platform_mqtt_password.clone(),
        local_mqtt_user: profile.credentials.aio_mqtt_user.clone(),
        local_mqtt_password: profile.credentials.aio_mqtt_password.clone(),
        auth_key: profile.credentials.platform_auth_key.clone(),
        node_name: node.name.clone(),
        node_ip: node.ip.clone(),
        node_mac: node.mac_normalized.clone(),
        node_building_id: node.building_id.clone().unwrap_or_default(),
        node_region_id: node.region_id.clone().unwrap_or_default(),
        node_addr_alias: node.addr_alias.clone().unwrap_or_default(),
        node_floor: node.floor.clone().unwrap_or_default(),
        node_location: node.location.clone().unwrap_or_default(),
        node_remark: node.remark.clone().unwrap_or_default(),
        images: manifest
            .images
            .iter()
            .map(|image| (image.service.clone(), image.image.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Cursor;
    use std::sync::Arc;

    use crate::domain::aio::deployment::{
        DeploymentImageInput, DeploymentMode, DeploymentPlanInput,
    };
    use crate::domain::aio::release::inspect_deployment_images;
    use crate::domain::common::task::{TaskEventLevel, TaskState};
    use crate::formal::app_state::FormalAppState;
    use crate::formal::config::AppPaths;
    use crate::formal::job_supervisor::JobSupervisor;
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::deployment_progress::TaskProgressGuard;
    use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TaskRepository};
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

    use super::{
        begin_deployment_preparation, converge_submission_failure, emit_preparation_progress,
        materialize_artifact_snapshot,
    };

    async fn local_state() -> (tempfile::TempDir, FormalAppState) {
        let temp = tempfile::tempdir().expect("temporary app data");
        let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
        paths.ensure().expect("app directories");
        let local_store = LocalStore::open(&paths.local_db)
            .await
            .expect("local store");
        sqlx::query(
            "INSERT INTO local_project \
             (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
              db_password_secret_ref, created_at, updated_at) VALUES \
             ('project', 'Project', 'http://platform.test', 'db.test', 3306, 'user', \
              'business', 'workbench', 'secret-ref', '1', '1')",
        )
        .execute(local_store.pool())
        .await
        .expect("project fixture");
        let task_repository = TaskRepository::new(local_store.pool().clone());
        let task_event_bus = TaskEventBus::new(32).expect("task event bus");
        let task_event_pipeline = TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus.clone(),
            SensitiveValueRedactor::default(),
        );
        let job_supervisor = JobSupervisor::default();
        let task_handler_registry = TaskHandlerRegistry::default();
        let task_queue =
            TaskQueue::start(8, 1, task_handler_registry.clone(), job_supervisor.clone())
                .await
                .expect("task queue");
        (
            temp,
            FormalAppState {
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
            },
        )
    }

    fn docker_image_tar(path: &std::path::Path, tag: &str) {
        let file = std::fs::File::create(path).expect("create image tar");
        let mut builder = tar::Builder::new(file);
        let manifest =
            serde_json::to_vec(&serde_json::json!([{"RepoTags": [tag]}])).expect("docker manifest");
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, "manifest.json", Cursor::new(manifest))
            .expect("append manifest");
        builder.finish().expect("finish image tar");
    }

    #[tokio::test]
    async fn queued_task_becomes_running_and_logs_preparation_before_remote_execution() {
        let (_temp, state) = local_state().await;
        let task_id = "deployment-preparation";
        let log_path = state
            .paths
            .project_task_log_path("project", task_id)
            .expect("task log path");
        state
            .task_repository
            .create(CreateTask {
                id: task_id.into(),
                local_project_id: "project".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                name: "整包升级".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: log_path.to_string_lossy().into_owned(),
                targets: vec![("aio".into(), "001122334455".into())],
            })
            .await
            .expect("task");
        let mut current = TaskState::Draft;
        for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
            state
                .task_repository
                .transition(task_id, current, next, None, None)
                .await
                .expect("task transition");
            current = next;
        }

        begin_deployment_preparation(&state, task_id, &["001122334455".into()])
            .await
            .expect("begin preparation");
        let guard = TaskProgressGuard::start(
            task_id.into(),
            state.task_repository.clone(),
            state.task_event_pipeline.clone(),
            Vec::new(),
        );
        emit_preparation_progress(
            guard.sink.as_ref(),
            &["001122334455".into()],
            "prepare_config",
            1,
            TaskEventLevel::Info,
            "DEPLOYMENT_PREPARATION_STARTED",
            "部署任务已开始，正在读取项目和发布配置",
        )
        .expect("preparation event");
        guard.stop().await.expect("persist preparation event");

        let task = state
            .task_repository
            .get(task_id)
            .await
            .expect("task state");
        assert_eq!(task.state, TaskState::Running);
        assert!(task.started_at.is_some());
        let target = state
            .task_repository
            .targets(task_id)
            .await
            .expect("targets")
            .remove(0);
        assert_eq!(target.stage, "prepare_config");
        assert_eq!(target.progress_current, 1);
        assert_eq!(target.progress_total, 100);
        let log = tokio::fs::read_to_string(log_path).await.expect("task log");
        assert!(log.contains("DEPLOYMENT_PREPARATION_STARTED"));
        assert!(log.contains("正在读取项目和发布配置"));
    }

    #[tokio::test]
    async fn cancelling_during_control_start_converges_task_and_target_as_cancelled() {
        let (_temp, state) = local_state().await;
        let task_id = "control-start-cancel";
        let log_path = state
            .paths
            .project_task_log_path("project", task_id)
            .expect("task log path");
        state
            .task_repository
            .create(CreateTask {
                id: task_id.into(),
                local_project_id: "project".into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "full_upgrade".into(),
                name: "整包升级".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: log_path.to_string_lossy().into_owned(),
                targets: vec![("aio".into(), "001122334455".into())],
            })
            .await
            .expect("task");
        let mut current = TaskState::Draft;
        for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
            state
                .task_repository
                .transition(task_id, current, next, None, None)
                .await
                .expect("task transition");
            current = next;
        }
        begin_deployment_preparation(&state, task_id, &["001122334455".into()])
            .await
            .expect("begin preparation");
        state
            .task_repository
            .update_target(
                task_id,
                crate::infrastructure::local_sqlite::task_repository::TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: "001122334455".into(),
                    state: crate::domain::common::task::TargetState::Running,
                    stage: "prepare_lease".into(),
                    progress_current: 9,
                    progress_total: 100,
                    fencing_token: None,
                    message_code: None,
                    message_params_json: None,
                },
            )
            .await
            .expect("lease preparation progress");
        state
            .task_repository
            .transition(
                task_id,
                TaskState::Running,
                TaskState::Cancelling,
                None,
                None,
            )
            .await
            .expect("request cancellation");

        converge_submission_failure(
            &state,
            task_id,
            &crate::core::error::AppError::Conflict("任务状态已变化".into()),
            false,
        )
        .await;

        assert_eq!(
            state
                .task_repository
                .get(task_id)
                .await
                .expect("task")
                .state,
            TaskState::Cancelled
        );
        let target = state
            .task_repository
            .targets(task_id)
            .await
            .expect("targets")
            .remove(0);
        assert_eq!(
            target.state,
            crate::domain::common::task::TargetState::Cancelled
        );
        assert_eq!(target.stage, "cancelled");
        assert_eq!(target.progress_current, 9);
    }

    #[test]
    fn full_deployment_generates_internal_manifest_and_checksums_from_images() {
        let temp = tempfile::tempdir().expect("temp");
        let first = temp.path().join("first.tar");
        let second = temp.path().join("second.tar");
        docker_image_tar(&first, "repo/first:1");
        docker_image_tar(&second, "repo/second:2");
        let images = vec![
            DeploymentImageInput {
                service_name: "first".into(),
                file_path: first.to_string_lossy().into_owned(),
                image_tag: "repo/first:1".into(),
            },
            DeploymentImageInput {
                service_name: "second".into(),
                file_path: second.to_string_lossy().into_owned(),
                image_tag: "repo/second:2".into(),
            },
        ];
        let fingerprint = inspect_deployment_images(&images)
            .expect("inspect images")
            .fingerprint;
        let mut input = DeploymentPlanInput {
            mode: DeploymentMode::FullUpgrade,
            target_macs: vec!["001122334455".into()],
            image_files: images,
            artifact_path: String::new(),
            artifact_name: "2个服务镜像".into(),
            artifact_version: "bundle-test".into(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: BTreeMap::new(),
            batch_size: 1,
            concurrency: 1,
        };
        let task_dir = temp.path().join("task");
        std::fs::create_dir_all(&task_dir).expect("task dir");
        let mut progress = Vec::new();
        let manifest = materialize_artifact_snapshot(
            &mut input,
            &task_dir,
            &fingerprint,
            &tokio_util::sync::CancellationToken::new(),
            |current, total, service| {
                progress.push((current, total, service.to_string()));
                Ok(())
            },
        )
        .expect("materialize")
        .expect("full manifest");
        assert_eq!(manifest.images.len(), 2);
        assert!(task_dir.join("generated-release/manifest.json").is_file());
        let checksums =
            std::fs::read_to_string(task_dir.join("generated-release/checksums/sha256.txt"))
                .expect("checksums");
        assert!(checksums.contains("images/first.tar"));
        assert!(checksums.contains("images/second.tar"));
        assert_eq!(
            std::fs::read_to_string(task_dir.join("generated-release/images/first.tag"))
                .expect("first tag"),
            "repo/first:1\n"
        );
        assert!(checksums.contains("images/first.tag"));
        assert!(checksums.contains("images/second.tag"));
        assert!(
            input
                .image_files
                .iter()
                .all(|image| std::path::Path::new(&image.file_path).is_file())
        );
        assert!(!progress.is_empty());
        assert_eq!(
            progress.last().map(|item| item.0),
            progress.last().map(|item| item.1)
        );
        assert!(progress.windows(2).all(|items| items[0].0 <= items[1].0));
    }

    #[test]
    fn service_upgrade_copies_and_verifies_the_selected_image_snapshot() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("device-edge.tar");
        docker_image_tar(&source, "repo/device-edge:1");
        let images = vec![DeploymentImageInput {
            service_name: "device-edge".into(),
            file_path: source.to_string_lossy().into_owned(),
            image_tag: "repo/device-edge:1".into(),
        }];
        let fingerprint = inspect_deployment_images(&images)
            .expect("inspect image")
            .fingerprint;
        let mut input = DeploymentPlanInput {
            mode: DeploymentMode::ServiceUpgrade,
            target_macs: vec!["001122334455".into()],
            image_files: images,
            artifact_path: String::new(),
            artifact_name: "device-edge.tar".into(),
            artifact_version: "device-edge-test".into(),
            service_name: Some("device-edge".into()),
            image_name: Some("repo/device-edge:1".into()),
            service_image_environment_variable: Some("DEVICE_EDGE_IMAGE".into()),
            images: BTreeMap::new(),
            batch_size: 1,
            concurrency: 1,
        };
        let task_dir = temp.path().join("task-service");
        std::fs::create_dir_all(&task_dir).expect("task dir");

        let manifest = materialize_artifact_snapshot(
            &mut input,
            &task_dir,
            &fingerprint,
            &tokio_util::sync::CancellationToken::new(),
            |_, _, _| Ok(()),
        )
        .expect("materialize service image");

        assert!(manifest.is_none());
        assert_eq!(input.artifact_path, input.image_files[0].file_path);
        assert!(task_dir.join("service-image.tar").is_file());
        assert_eq!(
            inspect_deployment_images(&input.image_files)
                .expect("inspect snapshot")
                .fingerprint,
            fingerprint
        );
    }
}
