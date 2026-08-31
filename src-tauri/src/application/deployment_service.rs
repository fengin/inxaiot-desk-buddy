use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::application::aio_assets::{application_instance_id, project_operator};
use crate::application::deployment_control::{
    finalize_deployment_control, mark_deployment_finalizing_failed, mark_deployment_interrupted,
    start_deployment_control, start_deployment_heartbeat,
};
use crate::application::deployment_executor::{
    DeploymentExecutionSummary, execute_deployment_targets,
};
use crate::application::execution_coordinator::{ExecutionCoordinator, ExecutionLifecyclePort};
use crate::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink,
};
use crate::application::ports::remote_session::{HostKeyPolicy, RemoteAuth, RemoteTarget};
use crate::application::project_context::{map_formal_error, project_database};
use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::release::{
    ReleaseManifest, inspect_image_archive, inspect_release_directory,
};
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel};
use crate::formal::app_state::FormalAppState;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::formal::runtime_registry::ConnectionHealth;
use crate::infrastructure::agent_asset::AGENT_SOURCE;
use crate::infrastructure::database::DualMySqlPools;
use crate::infrastructure::deployment_progress::TaskProgressGuard;
use crate::infrastructure::deployment_progress::TaskProgressReporter;
use crate::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles,
    execute_remote_deployment_observed_with_checkpoint,
};
use crate::infrastructure::device_api::{AioRegistrationPayload, DeviceApiClient};
use crate::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use crate::infrastructure::release_archive::create_release_tar;
use crate::infrastructure::release_template::{ReleaseRenderContext, render_release_templates};
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::workbench_aio::WorkbenchAioRepository;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchDeploymentInput {
    pub plan: DeploymentPlanInput,
}

pub async fn launch_deployment(
    state: &FormalAppState,
    local_project_id: &str,
    input: LaunchDeploymentInput,
) -> AppResult<DeploymentExecutionSummary> {
    let local_task_id = uuid::Uuid::now_v7().to_string();
    let result =
        launch_deployment_inner(state, local_project_id, &local_task_id, input, None).await;
    state.job_supervisor.finish(&local_task_id).await;
    result
}

pub fn prepare_launch_input(
    mut input: LaunchDeploymentInput,
) -> AppResult<(LaunchDeploymentInput, DeploymentPlan)> {
    normalize_target_macs(&mut input.plan)?;
    let _ = prepare_artifact(&mut input.plan)?;
    let plan = DeploymentPlan::build(input.plan.clone())?;
    Ok((input, plan))
}

pub async fn run_submitted_deployment(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    input: LaunchDeploymentInput,
    cancellation: tokio_util::sync::CancellationToken,
) -> AppResult<DeploymentExecutionSummary> {
    let result = launch_deployment_inner(
        state,
        local_project_id,
        local_task_id,
        input,
        Some(cancellation),
    )
    .await;
    if let Err(error) = &result {
        converge_submission_failure(state, local_task_id, error).await;
    }
    result
}

async fn launch_deployment_inner(
    state: &FormalAppState,
    local_project_id: &str,
    local_task_id: &str,
    mut input: LaunchDeploymentInput,
    registered_cancellation: Option<tokio_util::sync::CancellationToken>,
) -> AppResult<DeploymentExecutionSummary> {
    normalize_target_macs(&mut input.plan)?;
    let release_manifest = prepare_artifact(&mut input.plan)?;
    let plan = DeploymentPlan::build(input.plan)?;
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
        .get(&connection.db_password, "default")
        .await
        .map_err(map_formal_error)?;
    state
        .task_event_pipeline
        .register_secrets(release_secret_values(&connection.db_password, &profile))?;
    let nodes = WorkbenchAioRepository::new(pools.workbench.clone())
        .list_snapshots()
        .await?
        .into_iter()
        .map(|node| (node.mac_normalized.clone(), node))
        .collect::<HashMap<_, _>>();
    let task_dir = state
        .paths
        .project_task_dir(local_project_id, local_task_id)
        .map_err(map_formal_error)?;
    std::fs::create_dir_all(&task_dir).map_err(|error| AppError::io("创建部署任务目录", &error))?;
    let agent_path = task_dir.join("edge-node-agent.sh");
    std::fs::write(&agent_path, AGENT_SOURCE)
        .map_err(|error| AppError::io("写入内嵌Agent", &error))?;
    let shared_artifact = prepare_shared_artifact(&plan, &task_dir)?;
    let release_templates = load_release_templates(&plan, release_manifest.as_ref())?;
    let host_keys = HostKeyRepository::new(state.local_store.pool().clone());
    let mut prepared = HashMap::new();
    for mac in &plan.target_macs {
        let node = nodes
            .get(mac)
            .ok_or_else(|| AppError::NotFound(format!("部署目标不存在：{mac}")))?;
        let target = RemoteTarget {
            host: node.ip.clone(),
            port: profile.values.ssh_port,
            connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
        };
        let host_key = host_keys
            .get(local_project_id, &target)
            .await?
            .ok_or_else(|| AppError::Conflict(format!("SSH主机指纹尚未确认：{}", node.ip)))?;
        let node_dir = task_dir.join(mac);
        std::fs::create_dir_all(&node_dir)
            .map_err(|error| AppError::io("创建节点渲染目录", &error))?;
        let (env, host_info) =
            if let Some((host_template, compose_template, manifest)) = release_templates.as_ref() {
                let context = render_context(node, &profile, manifest);
                let rendered = render_release_templates(
                    &profile.values.env_template,
                    host_template,
                    &if profile.values.compose_template.is_empty() {
                        compose_template.clone()
                    } else {
                        profile.values.compose_template.clone()
                    },
                    &context,
                )?;
                let env = node_dir.join(".env");
                let host = node_dir.join("host-info.json");
                std::fs::write(&env, rendered.env)
                    .map_err(|error| AppError::io("写入节点env", &error))?;
                std::fs::write(&host, rendered.host_info_json)
                    .map_err(|error| AppError::io("写入节点host-info", &error))?;
                (Some(env), Some(host))
            } else {
                (None, None)
            };
        let auth = if let Some(private_key) = &profile.credentials.ssh_private_key {
            RemoteAuth::PrivateKey {
                username: profile.credentials.ssh_user.clone(),
                private_key: SecretValue::new(private_key.clone()),
                passphrase: None,
            }
        } else {
            RemoteAuth::Password {
                username: profile.credentials.ssh_user.clone(),
                password: SecretValue::new(
                    profile
                        .credentials
                        .ssh_password
                        .clone()
                        .ok_or_else(|| AppError::InvalidConfig("SSH凭据为空".into()))?,
                ),
            }
        };
        prepared.insert(
            mac.clone(),
            PreparedNode {
                target,
                auth,
                policy: HostKeyPolicy::Require(host_key.identity),
                files: RemoteDeploymentFiles {
                    local_agent: agent_path.clone(),
                    local_artifact: shared_artifact.clone(),
                    local_env: env,
                    local_host_info: host_info,
                },
                config: RemoteDeploymentConfig {
                    operation_id: String::new(),
                    mac_normalized: mac.clone(),
                    data_root: profile.values.aio_data_root.clone(),
                    deploy_root: profile.values.aio_deploy_root.clone(),
                    platform_api_host: profile.values.platform_host.clone(),
                    platform_api_port: profile.values.platform_api_port,
                    platform_mqtt_host: profile.values.platform_mqtt_host.clone(),
                    platform_mqtt_port: profile.values.platform_mqtt_port,
                    allow_existing_ports: plan.mode == DeploymentMode::FullUpgrade,
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
    }
    let cancellation = match registered_cancellation {
        Some(cancellation) => cancellation,
        None => state.job_supervisor.register(local_task_id).await?,
    };
    let lifecycle = AioExecutionLifecycle {
        state,
        local_project_id,
        local_task_id,
        operator: &operator,
        instance_id: &instance_id,
        pools,
        plan,
        prepared,
        secret_values: release_secret_values(&connection.db_password, &profile),
    };
    ExecutionCoordinator.run(&lifecycle, cancellation).await
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
) {
    use crate::domain::common::task::{TargetState, TaskState};
    use crate::infrastructure::local_sqlite::task_repository::TargetUpdate;

    let message = error.to_string();
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
                            state: TargetState::Failed,
                            stage: "submission_failed".into(),
                            progress_current: target
                                .progress_current
                                .min(target.progress_total.max(100)),
                            progress_total: target.progress_total.max(100),
                            fencing_token: target.fencing_token,
                            message_code: Some("DEPLOYMENT_SUBMISSION_FAILED".into()),
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
        TaskState::Running | TaskState::Cancelling => {
            let _ = state
                .task_repository
                .transition(
                    local_task_id,
                    task.state,
                    TaskState::Failed,
                    Some("DEPLOYMENT_EXECUTION_FAILED"),
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
    policy: HostKeyPolicy,
    files: RemoteDeploymentFiles,
    config: RemoteDeploymentConfig,
    registration: AioRegistrationPayload,
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
}

impl ExecutionLifecyclePort for AioExecutionLifecycle<'_> {
    type Handle = crate::application::deployment_control::DeploymentControlHandle;
    type Summary = DeploymentExecutionSummary;
    type Heartbeat = crate::application::deployment_control::DeploymentHeartbeatGuard;

    async fn start(&self) -> AppResult<Self::Handle> {
        start_deployment_control(
            self.state,
            self.local_project_id,
            self.local_task_id,
            self.plan.clone(),
            self.operator,
            self.instance_id,
        )
        .await
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
        let mut prepared = self.prepared.clone();
        for node in prepared.values_mut() {
            node.config.operation_id = handle.operation_id.clone();
        }
        let progress_guard = TaskProgressGuard::start(
            handle.local_task_id.clone(),
            self.state.task_repository.clone(),
            self.state.task_event_pipeline.clone(),
            self.secret_values.clone(),
        );
        let progress_sink = progress_guard.sink.clone();
        let execution = execute_prepared_targets(
            self.plan.clone(),
            self.pools.workbench.clone(),
            prepared,
            handle,
            cancellation,
            progress_sink,
        )
        .await;
        let progress = progress_guard.stop().await;
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
    handle: &crate::application::deployment_control::DeploymentControlHandle,
    cancellation: tokio_util::sync::CancellationToken,
    progress_sink: Arc<TaskProgressReporter>,
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
    let connector = Arc::new(RusshConnector::default());
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
            let plan = plan_for_worker.clone();
            let progress_sink = progress_sink.clone();
            async move {
                let execution = async {
                    let node = prepared
                        .get(&mac)
                        .ok_or_else(|| AppError::NotFound(format!("缺少节点执行上下文：{mac}")))?;
                    let lease = lease_by_mac
                        .get(&mac)
                        .cloned()
                        .ok_or_else(|| AppError::Conflict(format!("缺少节点租约：{mac}")))?;
                    validate_fencing(&workbench_pool, &lease).await?;
                    execute_remote_deployment_observed_with_checkpoint(
                        connector.as_ref(),
                        &node.target,
                        &node.auth,
                        node.policy.clone(),
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

fn prepare_artifact(input: &mut DeploymentPlanInput) -> AppResult<Option<ReleaseManifest>> {
    if input.mode == DeploymentMode::ServiceUpgrade {
        let image = inspect_image_archive(Path::new(&input.artifact_path))?;
        let tag = input
            .image_name
            .clone()
            .filter(|expected| image.repo_tags.contains(expected))
            .or_else(|| image.repo_tags.first().cloned())
            .ok_or_else(|| AppError::InvalidConfig("单镜像没有可用RepoTag".into()))?;
        input.artifact_name = input
            .service_name
            .clone()
            .unwrap_or_else(|| "service".into());
        input.artifact_version = tag
            .rsplit_once(':')
            .map(|(_, version)| version.to_string())
            .unwrap_or_else(|| tag.clone());
        input.image_name = Some(tag);
        input.images = BTreeMap::from([(
            input
                .service_name
                .clone()
                .unwrap_or_else(|| "service".into()),
            input.image_name.clone().unwrap_or_default(),
        )]);
        Ok(None)
    } else {
        let validation = inspect_release_directory(Path::new(&input.artifact_path))?;
        if !validation.valid {
            return Err(AppError::InvalidConfig(validation.errors.join("；")));
        }
        let manifest = validation
            .manifest
            .ok_or_else(|| AppError::InvalidConfig("Release缺少manifest".into()))?;
        input.artifact_name = "Release".into();
        input.artifact_version = manifest.version.clone();
        input.images = manifest
            .images
            .iter()
            .map(|image| (image.service.clone(), image.image.clone()))
            .collect();
        Ok(Some(manifest))
    }
}

fn prepare_shared_artifact(plan: &DeploymentPlan, task_dir: &Path) -> AppResult<PathBuf> {
    if plan.mode == DeploymentMode::ServiceUpgrade {
        return Ok(PathBuf::from(&plan.artifact_path));
    }
    let output = task_dir.join("release.tar");
    create_release_tar(Path::new(&plan.artifact_path), &output)
}

fn load_release_templates(
    plan: &DeploymentPlan,
    manifest: Option<&ReleaseManifest>,
) -> AppResult<Option<(String, String, ReleaseManifest)>> {
    let Some(manifest) = manifest else {
        return Ok(None);
    };
    let root = Path::new(&plan.artifact_path);
    let host = std::fs::read_to_string(root.join(&manifest.templates.host_info))
        .map_err(|error| AppError::io("读取host-info模板", &error))?;
    let compose = std::fs::read_to_string(root.join(&manifest.compose_file))
        .map_err(|error| AppError::io("读取Compose模板", &error))?;
    Ok(Some((host, compose, manifest.clone())))
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
