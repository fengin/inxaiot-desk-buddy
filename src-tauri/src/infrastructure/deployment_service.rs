use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use walkdir::WalkDir;

use crate::application::deployment_executor::{
    DeploymentExecutionSummary, execute_deployment_targets,
};
use crate::application::execution_coordinator::{ExecutionCoordinator, ExecutionLifecyclePort};
use crate::application::ports::deployment_progress::{
    DeploymentProgressEvent, DeploymentProgressSink,
};
use crate::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteTarget,
};
use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::deployment_workflow::{
    DeploymentExecutionSnapshot, DeploymentTargetSnapshot,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::release::{
    ReleaseManifest, inspect_image_archive, inspect_release_directory, sha256_file,
};
use crate::domain::aio::release_render::ReleaseRenderContext;
use crate::domain::common::task::{StepState, TargetState, TaskEventLevel};
use crate::formal::app_state::FormalAppState;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::release_master_key::ReleaseMasterKeyManager;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::formal::runtime_registry::ConnectionHealth;
use crate::infrastructure::agent_asset::AGENT_SOURCE;
use crate::infrastructure::aio_assets_service::{application_instance_id, project_operator};
use crate::infrastructure::database::DualMySqlPools;
use crate::infrastructure::deployment_control::{
    finalize_deployment_control, mark_deployment_finalizing_failed, mark_deployment_interrupted,
    start_deployment_control, start_deployment_heartbeat,
};
use crate::infrastructure::deployment_progress::TaskProgressGuard;
use crate::infrastructure::deployment_progress::TaskProgressReporter;
use crate::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles,
    execute_remote_deployment_observed_with_checkpoint,
};
use crate::infrastructure::device_api::{AioRegistrationPayload, DeviceApiClient};
use crate::infrastructure::project_context::{map_formal_error, project_database};
use crate::infrastructure::release_archive::create_release_tar;
use crate::infrastructure::release_template::render_release_templates;
use crate::infrastructure::remote::RusshConnector;

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
    input: LaunchDeploymentInput,
    registered_cancellation: Option<tokio_util::sync::CancellationToken>,
) -> AppResult<DeploymentExecutionSummary> {
    let snapshot = input.snapshot;
    snapshot.validate(local_project_id)?;
    let mut plan_input = snapshot.plan.clone();
    normalize_target_macs(&mut plan_input)?;
    DeploymentPlan::build(plan_input.clone())?;
    let operator = project_operator(state, local_project_id).await?;
    let instance_id = application_instance_id().to_string();
    let pools = project_database(state, local_project_id).await?;
    let local_projects =
        LocalProjectRepository::new(state.local_store.pool().clone(), state.secret_store.clone());
    let connection = local_projects
        .connection_secrets(local_project_id)
        .await
        .map_err(map_formal_error)?;
    let profile = ReleaseMasterKeyManager::with_local_registry(
        state.secret_store.clone(),
        state.local_store.pool().clone(),
    )
    .load_profile(
        &ReleaseProfileRepository::new(pools.workbench.clone()),
        local_project_id,
        "default",
    )
    .await
    .map_err(map_formal_error)?;
    if profile.version != snapshot.profile_version {
        return Err(AppError::Conflict(format!(
            "发布参数已从v{}变更为v{}，任务未执行；请重新预检",
            snapshot.profile_version, profile.version
        )));
    }
    state
        .task_event_pipeline
        .register_secrets(release_secret_values(&connection.db_password, &profile))?;
    let task_dir = state
        .paths
        .project_task_dir(local_project_id, local_task_id)
        .map_err(map_formal_error)?;
    std::fs::create_dir_all(&task_dir).map_err(|error| AppError::io("创建部署任务目录", &error))?;
    let artifact_task_dir = task_dir.clone();
    let expected_fingerprint = snapshot.artifact_fingerprint.clone();
    let (plan_input, release_manifest) = tokio::task::spawn_blocking(move || {
        materialize_artifact_snapshot(&mut plan_input, &artifact_task_dir, &expected_fingerprint)?;
        let (release_manifest, actual_fingerprint) = prepare_artifact(&mut plan_input)?;
        if !actual_fingerprint.eq_ignore_ascii_case(&expected_fingerprint) {
            return Err(AppError::Integrity {
                operation: "校验部署发布物快照",
            });
        }
        Ok::<_, AppError>((plan_input, release_manifest))
    })
    .await
    .map_err(|_| AppError::Io {
        operation: "等待发布物快照任务",
    })??;
    let plan = DeploymentPlan::build(plan_input)?;
    let release_fingerprint = snapshot.artifact_fingerprint.clone();
    let nodes = snapshot
        .targets
        .into_iter()
        .map(|target| (target.node.mac_normalized.clone(), target))
        .collect::<HashMap<String, DeploymentTargetSnapshot>>();
    let agent_path = task_dir.join("edge-node-agent.sh");
    std::fs::write(&agent_path, AGENT_SOURCE)
        .map_err(|error| AppError::io("写入内嵌Agent", &error))?;
    let shared_artifact = prepare_shared_artifact(&plan, &task_dir)?;
    let release_templates = load_release_templates(&plan, release_manifest.as_ref())?;
    let mut prepared = HashMap::new();
    for mac in &plan.target_macs {
        let target_snapshot = nodes
            .get(mac)
            .ok_or_else(|| AppError::NotFound(format!("部署目标快照不存在：{mac}")))?;
        let node = &target_snapshot.node;
        let target = RemoteTarget {
            host: target_snapshot.ssh_host.clone(),
            port: target_snapshot.ssh_port,
            connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
        };
        let node_dir = task_dir.join(mac);
        std::fs::create_dir_all(&node_dir)
            .map_err(|error| AppError::io("创建节点渲染目录", &error))?;
        let (env, host_info, compose) =
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
                let compose = node_dir.join("docker-compose.yml");
                std::fs::write(&env, rendered.env)
                    .map_err(|error| AppError::io("写入节点env", &error))?;
                std::fs::write(&host, rendered.host_info_json)
                    .map_err(|error| AppError::io("写入节点host-info", &error))?;
                std::fs::write(&compose, rendered.compose_preview)
                    .map_err(|error| AppError::io("写入节点Compose", &error))?;
                (Some(env), Some(host), Some(compose))
            } else {
                (None, None, None)
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
                policy: HostKeyPolicy::Require(HostKeyIdentity {
                    algorithm: target_snapshot.host_key_algorithm.clone(),
                    fingerprint: target_snapshot.host_key_fingerprint.clone(),
                }),
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
    type Handle = crate::infrastructure::deployment_control::DeploymentControlHandle;
    type Summary = DeploymentExecutionSummary;
    type Heartbeat = crate::infrastructure::deployment_control::DeploymentHeartbeatGuard;

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
    handle: &crate::infrastructure::deployment_control::DeploymentControlHandle,
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

fn prepare_artifact(
    input: &mut DeploymentPlanInput,
) -> AppResult<(Option<ReleaseManifest>, String)> {
    if input.mode == DeploymentMode::ServiceUpgrade {
        let image = inspect_image_archive(Path::new(&input.artifact_path))?;
        let fingerprint = sha256_file(Path::new(&input.artifact_path))?;
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
        Ok((None, fingerprint))
    } else {
        let validation = inspect_release_directory(Path::new(&input.artifact_path))?;
        if !validation.valid {
            return Err(AppError::InvalidConfig(validation.errors.join("；")));
        }
        let fingerprint = validation.fingerprint.ok_or(AppError::Integrity {
            operation: "读取Release指纹",
        })?;
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
        Ok((Some(manifest), fingerprint))
    }
}

fn materialize_artifact_snapshot(
    input: &mut DeploymentPlanInput,
    task_dir: &Path,
    expected_fingerprint: &str,
) -> AppResult<()> {
    let source = PathBuf::from(&input.artifact_path)
        .canonicalize()
        .map_err(|error| AppError::io("规范化部署发布物", &error))?;
    if input.mode == DeploymentMode::ServiceUpgrade {
        if std::fs::symlink_metadata(&source)
            .map_err(|error| AppError::io("读取单服镜像属性", &error))?
            .file_type()
            .is_symlink()
        {
            return Err(AppError::InvalidConfig(
                "单服镜像快照不允许使用符号链接".into(),
            ));
        }
        let temporary = task_dir.join("service-image.tar.part");
        let destination = task_dir.join("service-image.tar");
        if temporary.exists() || destination.exists() {
            return Err(AppError::Conflict("任务发布物快照已存在".into()));
        }
        std::fs::copy(&source, &temporary)
            .map_err(|error| AppError::io("复制单服镜像快照", &error))?;
        let actual = sha256_file(&temporary)?;
        if !actual.eq_ignore_ascii_case(expected_fingerprint) {
            let _ = std::fs::remove_file(&temporary);
            return Err(AppError::Integrity {
                operation: "校验单服镜像快照",
            });
        }
        std::fs::rename(&temporary, &destination)
            .map_err(|error| AppError::io("发布单服镜像快照", &error))?;
        input.artifact_path = destination.to_string_lossy().into_owned();
        return Ok(());
    }

    let temporary = task_dir.join("release-snapshot.part");
    let destination = task_dir.join("release-snapshot");
    if temporary.exists() || destination.exists() {
        return Err(AppError::Conflict("任务Release快照已存在".into()));
    }
    std::fs::create_dir(&temporary)
        .map_err(|error| AppError::io("创建Release快照临时目录", &error))?;
    let copy_result = copy_release_directory(&source, &temporary);
    if let Err(error) = copy_result {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(error);
    }
    let validation = inspect_release_directory(&temporary)?;
    let actual = validation.fingerprint.ok_or(AppError::Integrity {
        operation: "读取Release快照指纹",
    })?;
    if !validation.valid || !actual.eq_ignore_ascii_case(expected_fingerprint) {
        let _ = std::fs::remove_dir_all(&temporary);
        return Err(AppError::Integrity {
            operation: "校验Release快照",
        });
    }
    std::fs::rename(&temporary, &destination)
        .map_err(|error| AppError::io("发布Release快照", &error))?;
    input.artifact_path = destination.to_string_lossy().into_owned();
    Ok(())
}

fn copy_release_directory(source: &Path, destination: &Path) -> AppResult<()> {
    if !source.is_dir() {
        return Err(AppError::InvalidConfig("Release快照源不是目录".into()));
    }
    let mut entries = WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::InvalidConfig("遍历Release快照源失败".into()))?;
    entries.sort_by(|left, right| left.path().cmp(right.path()));
    for entry in entries {
        if entry.path() == source {
            continue;
        }
        if entry.file_type().is_symlink() {
            return Err(AppError::InvalidConfig(
                "Release快照不允许包含符号链接".into(),
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(source)
            .map_err(|_| AppError::InvalidConfig("Release快照路径异常".into()))?;
        if relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(AppError::InvalidConfig("Release快照路径不安全".into()));
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| AppError::io("创建Release快照目录", &error))?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| AppError::io("创建Release快照父目录", &error))?;
            }
            std::fs::copy(entry.path(), &target)
                .map_err(|error| AppError::io("复制Release快照文件", &error))?;
        } else {
            return Err(AppError::InvalidConfig(
                "Release快照包含不支持的文件类型".into(),
            ));
        }
    }
    Ok(())
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
