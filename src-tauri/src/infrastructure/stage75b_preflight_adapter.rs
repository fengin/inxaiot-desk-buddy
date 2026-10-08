use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::application::ports::deployment_workflow::DeploymentPreflightPort;
use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use crate::application::ports::remote_session::{RemoteConnection, RemoteTarget};
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{
    DeploymentMode, DeploymentPlanInput, validate_deployment_batch,
};
use crate::domain::aio::deployment_workflow::{
    DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION, DeploymentExecutionSnapshot, DeploymentPreflightCheck,
    DeploymentPreflightReport, DeploymentTargetSnapshot, PreflightRemediation, PreflightStatus,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::release::{
    ReleaseRuntime, inspect_deployment_images, runtime_version_satisfies, standard_runtime,
    validate_release_version,
};
use crate::domain::aio::release_profile::{
    inspect_compose_published_ports, inspect_compose_services,
};
use crate::domain::aio::release_render::ReleaseRenderContext;
use crate::domain::common::task::{TargetState, TaskEventLevel, TaskState};
use crate::formal::app_state::FormalAppState;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::infrastructure::agent_asset::effective_agent_asset;
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::deployment_preflight_probes::{
    local_endpoint_checks, remote_endpoint_check, remote_environment_checks,
};
use crate::infrastructure::local_sqlite::host_key_repository::{HostKeyRecord, HostKeyRepository};
use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TargetUpdate};
use crate::infrastructure::project_context::{
    map_formal_error, project_aio_database as project_database,
};
use crate::infrastructure::release_remote_auth::release_remote_auth;
use crate::infrastructure::release_template::render_release_templates;
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::remote::observed::ObservedConnector;
use crate::infrastructure::stage75_adapter::stored_release_secret_values;
use crate::infrastructure::task_data_lifecycle::TaskDataLifecycle;
use crate::infrastructure::workbench_aio::WorkbenchAioRepository;

pub struct Stage75BPreflightAdapter<'a> {
    state: &'a FormalAppState,
    task_id: Option<String>,
}

impl<'a> Stage75BPreflightAdapter<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self {
            state,
            task_id: None,
        }
    }

    pub fn tracked(state: &'a FormalAppState, task_id: impl Into<String>) -> Self {
        Self {
            state,
            task_id: Some(task_id.into()),
        }
    }
}

impl Stage75BPreflightAdapter<'_> {
    async fn preflight_inner(
        &self,
        project_id: &str,
        input: &DeploymentPlanInput,
        tracker: Option<&PreflightTaskTracker<'_>>,
    ) -> AppResult<DeploymentPreflightReport> {
        let checked_at = timestamp();
        let mut checks = Vec::new();

        // 项目连接/结构和登录在项目入口处理；这里只保留业务调用的内部前置保护，
        // 不再生成与一体机部署无关的成功检查卡片。
        project_operator(self.state, project_id)
            .await
            .map_err(|_| {
                AppError::Authentication("项目登录已失效，请返回项目入口重新登录".into())
            })?;
        let pools = project_database(self.state, project_id).await.map_err(|error| {
            tracing::warn!(error = %crate::core::log_safety::safe_error(&error), "deployment project unavailable");
            AppError::Conflict("项目连接不可用，请返回项目入口检查数据库连接和结构".into())
        })?;
        let profile = match ReleaseProfileRepository::new(pools.workbench.clone())
            .get("default")
            .await
        {
            Ok(value) => {
                if value.credentials_reset_required {
                    checks.push(failed(
                        "release_profile",
                        "发布参数",
                        None,
                        "发布凭据加密格式已更新，请重新填写发布参数".into(),
                        remediation(
                            "open_release_profile",
                            "重新填写发布参数",
                            Some("/aio/release"),
                            None,
                        ),
                    ));
                    if let Some(tracker) = tracker {
                        tracker
                            .common_finished(CommonWork::Parameters, &checks)
                            .await?;
                    }
                    return Ok(report(checks, input.clone(), None, checked_at));
                }
                if let Err(error) = effective_agent_asset(value.agent_script.as_ref()) {
                    checks.push(failed(
                        "release_agent",
                        "一体机脚本",
                        None,
                        error.to_string(),
                        remediation(
                            "open_release_profile",
                            "检查或更换一体机脚本",
                            Some("/aio/release"),
                            None,
                        ),
                    ));
                    if let Some(tracker) = tracker {
                        tracker
                            .common_finished(CommonWork::Parameters, &checks)
                            .await?;
                    }
                    return Ok(report(checks, input.clone(), None, checked_at));
                }
                checks.push(passed(
                    "release_profile",
                    "发布参数",
                    None,
                    format!("default v{}已从工作台Schema读取", value.version),
                ));
                value
            }
            Err(error) => {
                checks.push(failed(
                    "release_profile",
                    "发布参数",
                    None,
                    map_formal_error(error).to_string(),
                    remediation(
                        "open_release_profile",
                        "维护发布参数",
                        Some("/aio/release"),
                        None,
                    ),
                ));
                if let Some(tracker) = tracker {
                    tracker
                        .common_finished(CommonWork::Parameters, &checks)
                        .await?;
                }
                return Ok(report(checks, input.clone(), None, checked_at));
            }
        };
        self.state
            .task_event_pipeline
            .register_secrets(stored_release_secret_values(&profile.credentials))?;
        let published_ports = match inspect_compose_published_ports(
            &profile.values.compose_template,
            &profile.values.env_template,
        ) {
            Ok(ports) => ports,
            Err(error) => {
                checks.push(failed(
                    "release_profile_ports",
                    "Compose端口",
                    None,
                    error.to_string(),
                    remediation(
                        "open_release_profile",
                        "修改发布参数",
                        Some("/aio/release"),
                        None,
                    ),
                ));
                if let Some(tracker) = tracker {
                    tracker
                        .common_finished(CommonWork::Parameters, &checks)
                        .await?;
                }
                return Ok(report(
                    checks,
                    input.clone(),
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        checks.push(local_endpoint_checks(&profile.values).await);
        let nodes = match WorkbenchAioRepository::new(pools.workbench.clone())
            .list_snapshots()
            .await
        {
            Ok(values) => values
                .into_iter()
                .map(|node| (node.mac_normalized.clone(), node))
                .collect::<HashMap<_, _>>(),
            Err(error) => {
                checks.push(failed(
                    "deployment_targets",
                    "部署目标",
                    None,
                    error.to_string(),
                    remediation("open_aio_nodes", "返回一体机列表", Some("/aio/nodes"), None),
                ));
                if let Some(tracker) = tracker {
                    tracker
                        .common_finished(CommonWork::Parameters, &checks)
                        .await?;
                }
                return Ok(report(
                    checks,
                    input.clone(),
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        if let Some(tracker) = tracker {
            tracker
                .common_finished(CommonWork::Parameters, &checks)
                .await?;
            tracker.images_started(input.image_files.len()).await?;
        }
        let artifact_check_start = checks.len();
        let inspection_input = input.clone();
        let inspection_profile = profile.clone();
        let inspection_time = checked_at.clone();
        let (inspected, artifact_check) = tokio::task::spawn_blocking(move || {
            normalize_and_inspect_artifact(&inspection_input, &inspection_profile, &inspection_time)
        })
        .await
        .map_err(|error| AppError::Conflict(format!("镜像检查任务异常结束：{error}")))?;
        checks.push(artifact_check);
        if let Some(tracker) = tracker {
            tracker
                .common_finished(CommonWork::Images, &checks[artifact_check_start..])
                .await?;
        }
        let inspected = match inspected {
            Ok(value) => value,
            Err(_) => {
                return Ok(report(
                    checks,
                    input.clone(),
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        if let Some(tracker) = tracker {
            tracker.common_started(CommonWork::Batch).await?;
        }
        let batch_check_start = checks.len();
        let (inspected, batch_check) = validate_batch_and_finalize_artifact(inspected);
        checks.push(batch_check);
        if let Some(tracker) = tracker {
            tracker
                .common_finished(CommonWork::Batch, &checks[batch_check_start..])
                .await?;
        }
        let inspected = match inspected {
            Ok(value) => value,
            Err(_) => {
                return Ok(report(
                    checks,
                    input.clone(),
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        let normalized = inspected.normalized;
        let artifact_fingerprint = inspected.fingerprint;
        let runtime = inspected.runtime;
        let leases = ResourceLeaseRepository::new(pools.workbench.clone());
        let connector = ObservedConnector::new(
            RusshConnector::default(),
            HostKeyRepository::new(self.state.local_store.pool().clone()),
            project_id,
        );
        let platform_inventory =
            crate::infrastructure::platform_aio::PlatformAioRepository::new(pools.platform.clone())
                .list_all()
                .await?;
        let mut target_snapshots = Vec::with_capacity(normalized.target_macs.len());
        for mac in &normalized.target_macs {
            let node = nodes.get(mac);
            let name = node.map(|value| value.name.as_str());
            let ip = node.map(|value| value.ip.as_str());
            if let Some(tracker) = tracker {
                tracker.target_started(mac, name, ip).await?;
                tracker
                    .target_work_started(mac, name, ip, TargetWork::Runtime)
                    .await?;
            }
            let target_check_start = checks.len();
            let Some(node) = node else {
                checks.push(failed(
                    "deployment_target",
                    "部署目标",
                    Some(mac),
                    "当前项目工作台资产中不存在此MAC".into(),
                    remediation(
                        "open_aio_nodes",
                        "返回一体机列表",
                        Some("/aio/nodes"),
                        Some(mac),
                    ),
                ));
                if let Some(tracker) = tracker {
                    tracker
                        .target_work_finished(
                            mac,
                            None,
                            None,
                            TargetWork::Runtime,
                            &checks[target_check_start..],
                            None,
                        )
                        .await?;
                    if normalized.mode != DeploymentMode::ServiceUpgrade {
                        tracker
                            .target_work_started(mac, None, None, TargetWork::Render)
                            .await?;
                        tracker
                            .target_work_finished(
                                mac,
                                None,
                                None,
                                TargetWork::Render,
                                &[],
                                Some("一体机资产不存在，无法取得模板渲染参数"),
                            )
                            .await?;
                    }
                    tracker
                        .target_finished(mac, None, None, &checks[target_check_start..])
                        .await?;
                }
                continue;
            };
            let deployment_mac = match platform_inventory.deployment_mac(mac) {
                Ok(value) => value,
                Err(error) => {
                    checks.push(failed(
                        "platform_identity",
                        "平台一体机记录",
                        Some(mac),
                        error.to_string(),
                        remediation(
                            "open_aio_nodes",
                            "返回一体机列表",
                            Some("/aio/nodes"),
                            Some(mac),
                        ),
                    ));
                    if let Some(tracker) = tracker {
                        tracker
                            .target_work_finished(
                                mac,
                                Some(&node.name),
                                Some(&node.ip),
                                TargetWork::Runtime,
                                &checks[target_check_start..],
                                None,
                            )
                            .await?;
                        if normalized.mode != DeploymentMode::ServiceUpgrade {
                            tracker
                                .target_work_finished(
                                    mac,
                                    Some(&node.name),
                                    Some(&node.ip),
                                    TargetWork::Render,
                                    &[],
                                    Some("平台一体机记录重复，未继续渲染部署配置"),
                                )
                                .await?;
                        }
                        tracker
                            .target_finished(
                                mac,
                                Some(&node.name),
                                Some(&node.ip),
                                &checks[target_check_start..],
                            )
                            .await?;
                    }
                    continue;
                }
            };
            checks.push(passed(
                "deployment_target",
                "部署目标",
                Some(mac),
                format!("{} · {}", node.name, node.ip),
            ));
            match leases
                .active_lease("aio", mac)
                .await
                .map_err(map_formal_error)?
            {
                Some(lease) => checks.push(failed(
                    "resource_lease",
                    "资源租约",
                    Some(mac),
                    format!(
                        "这台一体机正在执行其他任务，请等待完成后重试（操作{}）",
                        lease.operation_id
                    ),
                    remediation("open_history", "查看占用操作", None, Some(mac)),
                )),
                None => checks.push(passed(
                    "resource_lease",
                    "资源租约",
                    Some(mac),
                    "当前没有未过期的活动租约".into(),
                )),
            }
            let target = RemoteTarget {
                host: node.ip.clone(),
                port: profile.values.ssh_port,
                connect_timeout: Duration::from_secs(u64::from(profile.values.ssh_timeout_seconds)),
            };
            if let Some(host_key) = append_remote_runtime_checks(
                &mut checks,
                mac,
                &target,
                &profile,
                &connector,
                &runtime,
                normalized.mode,
                &published_ports,
            )
            .await
            {
                target_snapshots.push(DeploymentTargetSnapshot {
                    node: node.clone(),
                    ssh_host: target.host.clone(),
                    ssh_port: target.port,
                    host_key_algorithm: host_key.identity.algorithm,
                    host_key_fingerprint: host_key.identity.fingerprint,
                    host_key_accepted_at: host_key.accepted_at,
                });
            }
            if let Some(tracker) = tracker {
                tracker
                    .target_work_finished(
                        mac,
                        Some(&node.name),
                        Some(&node.ip),
                        TargetWork::Runtime,
                        &checks[target_check_start..],
                        None,
                    )
                    .await?;
            }
            if normalized.mode != DeploymentMode::ServiceUpgrade {
                if let Some(tracker) = tracker {
                    tracker
                        .target_work_started(
                            mac,
                            Some(&node.name),
                            Some(&node.ip),
                            TargetWork::Render,
                        )
                        .await?;
                }
                let render_check_start = checks.len();
                let mut context = render_context(node, &profile, &normalized);
                context.node_mac = deployment_mac;
                match render_release_templates(
                    &profile.values.env_template,
                    &profile.values.host_info_template,
                    &profile.values.compose_template,
                    &context,
                ) {
                    Ok(_) => checks.push(passed(
                        "release_render",
                        "发布渲染",
                        Some(mac),
                        "三个项目模板已使用当前一体机参数完整渲染并通过格式检查".into(),
                    )),
                    Err(error) => checks.push(failed(
                        "release_render",
                        "发布渲染",
                        Some(mac),
                        error.to_string(),
                        remediation(
                            "open_release_profile",
                            "修改发布参数",
                            Some("/aio/release"),
                            Some(mac),
                        ),
                    )),
                }
                if let Some(tracker) = tracker {
                    tracker
                        .target_work_finished(
                            mac,
                            Some(&node.name),
                            Some(&node.ip),
                            TargetWork::Render,
                            &checks[render_check_start..],
                            None,
                        )
                        .await?;
                }
            }
            if let Some(tracker) = tracker {
                tracker
                    .target_finished(
                        mac,
                        Some(&node.name),
                        Some(&node.ip),
                        &checks[target_check_start..],
                    )
                    .await?;
            }
        }
        let execution_snapshot = DeploymentExecutionSnapshot {
            schema_version: DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION,
            local_project_id: project_id.into(),
            checked_at: checked_at.clone(),
            profile_version: profile.version,
            artifact_fingerprint,
            plan: normalized.clone(),
            targets: target_snapshots,
        };
        Ok(DeploymentPreflightReport::from_checks(
            checks,
            normalized,
            Some(profile.version),
            Some(execution_snapshot),
            checked_at,
        ))
    }
}

impl DeploymentPreflightPort for Stage75BPreflightAdapter<'_> {
    async fn preflight(
        &self,
        project_id: &str,
        input: &DeploymentPlanInput,
    ) -> AppResult<DeploymentPreflightReport> {
        let tracker = match self.task_id.as_deref() {
            Some(task_id) => {
                Some(PreflightTaskTracker::start(self.state, project_id, task_id, input).await?)
            }
            None => None,
        };
        let result = self
            .preflight_inner(project_id, input, tracker.as_ref())
            .await;
        if let Some(tracker) = tracker {
            match &result {
                Ok(report) => {
                    if let Err(error) = tracker.finish(report).await {
                        let _ = tracker.interrupt(&error).await;
                        return Err(error);
                    }
                }
                Err(error) => {
                    if let Err(finalize_error) = tracker.interrupt(error).await {
                        tracing::error!(
                            error = ?crate::core::log_safety::safe_error(&finalize_error),
                            "finalize deployment preflight task failed"
                        );
                    }
                }
            }
        }
        result
    }
}

const PREFLIGHT_COMMON_RESOURCE_TYPE: &str = "preflight_internal";
const PREFLIGHT_COMMON_RESOURCE_KEY: &str = "common";
const PREFLIGHT_COMMON_WORK_UNITS: u64 = 3;

#[derive(Clone, Copy)]
enum CommonWork {
    Parameters,
    Images,
    Batch,
}

impl CommonWork {
    fn stage(self) -> &'static str {
        match self {
            Self::Parameters => "检查参数",
            Self::Images => "检查镜像",
            Self::Batch => "检查批次",
        }
    }

    fn started_code(self) -> &'static str {
        match self {
            Self::Parameters => "PREFLIGHT_PARAMETERS_STARTED",
            Self::Images => "PREFLIGHT_IMAGES_STARTED",
            Self::Batch => "PREFLIGHT_BATCH_STARTED",
        }
    }

    fn finished_code(self, failed: bool) -> &'static str {
        match (self, failed) {
            (Self::Parameters, false) => "PREFLIGHT_PARAMETERS_FINISHED",
            (Self::Parameters, true) => "PREFLIGHT_PARAMETERS_FAILED",
            (Self::Images, false) => "PREFLIGHT_IMAGES_FINISHED",
            (Self::Images, true) => "PREFLIGHT_IMAGES_FAILED",
            (Self::Batch, false) => "PREFLIGHT_BATCH_FINISHED",
            (Self::Batch, true) => "PREFLIGHT_BATCH_FAILED",
        }
    }
}

#[derive(Clone, Copy)]
enum TargetWork {
    Runtime,
    Render,
}

impl TargetWork {
    fn stage(self) -> &'static str {
        match self {
            Self::Runtime => "检查一体机",
            Self::Render => "渲染模板",
        }
    }

    fn started_code(self) -> &'static str {
        match self {
            Self::Runtime => "PREFLIGHT_TARGET_RUNTIME_STARTED",
            Self::Render => "PREFLIGHT_TARGET_RENDER_STARTED",
        }
    }

    fn finished_code(self, failed: bool) -> &'static str {
        match (self, failed) {
            (Self::Runtime, false) => "PREFLIGHT_TARGET_RUNTIME_FINISHED",
            (Self::Runtime, true) => "PREFLIGHT_TARGET_RUNTIME_FAILED",
            (Self::Render, false) => "PREFLIGHT_TARGET_RENDER_FINISHED",
            (Self::Render, true) => "PREFLIGHT_TARGET_RENDER_FAILED",
        }
    }

    fn target_progress(self) -> u64 {
        match self {
            Self::Runtime => 1,
            Self::Render => 2,
        }
    }
}

struct PreflightTaskTracker<'a> {
    state: &'a FormalAppState,
    task_id: String,
    project_id: String,
    total_targets: u64,
    total_work_units: u64,
    completed_work_units: AtomicU64,
    target_work_units: u64,
    target_scope: &'static str,
}

impl<'a> PreflightTaskTracker<'a> {
    async fn start(
        state: &'a FormalAppState,
        project_id: &str,
        task_id: &str,
        input: &DeploymentPlanInput,
    ) -> AppResult<Self> {
        uuid::Uuid::parse_str(task_id)
            .map_err(|_| AppError::InvalidConfig("预检任务ID无效".into()))?;
        let targets = input
            .target_macs
            .iter()
            .map(|mac| {
                MacAddress::parse(mac).map(|value| ("aio".into(), value.normalized().to_string()))
            })
            .collect::<AppResult<BTreeSet<_>>>()?;
        if targets.is_empty() {
            return Err(AppError::InvalidConfig(
                "部署预检至少需要一台有效目标".into(),
            ));
        }
        let total_targets = u64::try_from(targets.len())
            .map_err(|_| AppError::InvalidConfig("部署预检目标数量超出范围".into()))?;
        let target_work_units = target_work_units(input.mode);
        let total_work_units = preflight_work_units(input.mode, total_targets)?;
        let batch_size = u32::try_from(targets.len())
            .map_err(|_| AppError::InvalidConfig("部署预检目标数量超出范围".into()))?;
        let target_keys = targets
            .iter()
            .map(|(_, mac)| mac.clone())
            .collect::<Vec<_>>();
        let mut persisted_targets = targets;
        persisted_targets.insert((
            PREFLIGHT_COMMON_RESOURCE_TYPE.into(),
            PREFLIGHT_COMMON_RESOURCE_KEY.into(),
        ));
        state
            .task_repository
            .create(CreateTask {
                id: task_id.into(),
                local_project_id: project_id.into(),
                remote_operation_record_id: None,
                domain_type: "aio".into(),
                operation_type: "deployment_preflight".into(),
                name: format!("部署检查 · {}", deployment_mode_label(input.mode)),
                priority: 0,
                batch_size,
                concurrency: input.concurrency.clamp(1, batch_size),
                payload_ref: None,
                log_path: state
                    .paths
                    .project_task_log_path(project_id, task_id)
                    .map_err(crate::infrastructure::project_context::map_formal_error)?
                    .to_string_lossy()
                    .into_owned(),
                targets: persisted_targets.into_iter().collect(),
            })
            .await?;
        let tracker = Self {
            state,
            task_id: task_id.into(),
            project_id: project_id.into(),
            total_targets,
            total_work_units,
            completed_work_units: AtomicU64::new(0),
            target_work_units,
            target_scope: match input.mode {
                DeploymentMode::ServiceUpgrade => {
                    "资产、任务占用、SSH、平台连通性、系统架构、Docker、目录空间和端口"
                }
                DeploymentMode::FirstDeploy | DeploymentMode::FullUpgrade => {
                    "资产、模板渲染、任务占用、SSH、平台连通性、系统架构、Docker、目录空间和端口"
                }
            },
        };
        let setup = async {
            state
                .task_repository
                .transition(
                    task_id,
                    TaskState::Draft,
                    TaskState::Checking,
                    None,
                    Some("正在检查部署执行条件"),
                )
                .await?;
            state
                .task_repository
                .update_target(
                    task_id,
                    TargetUpdate {
                        resource_type: PREFLIGHT_COMMON_RESOURCE_TYPE.into(),
                        resource_key: PREFLIGHT_COMMON_RESOURCE_KEY.into(),
                        state: TargetState::Running,
                        stage: "检查参数".into(),
                        progress_current: 0,
                        progress_total: PREFLIGHT_COMMON_WORK_UNITS,
                        fencing_token: None,
                        message_code: Some("PREFLIGHT_PARAMETERS_STARTED".into()),
                        message_params_json: None,
                    },
                )
                .await?;
            for mac in &target_keys {
                state
                    .task_repository
                    .update_target(
                        task_id,
                        TargetUpdate {
                            resource_type: "aio".into(),
                            resource_key: mac.clone(),
                            state: TargetState::Pending,
                            stage: "等待检查".into(),
                            progress_current: 0,
                            progress_total: target_work_units,
                            fencing_token: None,
                            message_code: None,
                            message_params_json: None,
                        },
                    )
                    .await?;
            }
            tracker
                .emit_with_progress(
                    None,
                    "检查参数",
                    "checking",
                    TaskEventLevel::Info,
                    "PREFLIGHT_STARTED",
                    format!(
                        "开始检查{}执行条件，共{}项：参数、镜像、批次、{}台一体机运行环境{}",
                        deployment_mode_label(input.mode),
                        total_work_units,
                        total_targets,
                        if input.mode == DeploymentMode::ServiceUpgrade {
                            ""
                        } else {
                            "和逐台模板渲染"
                        }
                    ),
                    0,
                )
                .await?;
            tracker.common_started(CommonWork::Parameters).await
        }
        .await;
        if let Err(error) = setup {
            let _ = tracker.interrupt(&error).await;
            return Err(error);
        }
        Ok(tracker)
    }

    async fn common_started(&self, work: CommonWork) -> AppResult<()> {
        self.emit_transient_with_progress(
            None,
            work.stage(),
            "checking",
            TaskEventLevel::Info,
            work.started_code(),
            format!("正在{}", work.stage()),
            self.completed_work_units.load(Ordering::SeqCst),
        )
        .await
    }

    async fn images_started(&self, image_count: usize) -> AppResult<()> {
        self.emit_transient_with_progress(
            None,
            CommonWork::Images.stage(),
            "checking",
            TaskEventLevel::Info,
            CommonWork::Images.started_code(),
            format!("正在检查{image_count}个镜像文件（归档内容和完整性哈希）"),
            self.completed_work_units.load(Ordering::SeqCst),
        )
        .await
    }

    async fn common_finished(
        &self,
        work: CommonWork,
        checks: &[DeploymentPreflightCheck],
    ) -> AppResult<()> {
        let blockers = checks
            .iter()
            .filter(|check| {
                check.target_mac.is_none()
                    && check.blocking
                    && check.status == PreflightStatus::Failed
            })
            .collect::<Vec<_>>();
        let checked = checks
            .iter()
            .filter(|check| check.target_mac.is_none())
            .map(|check| check.label.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("、");
        let failed = !blockers.is_empty();
        let completed = self.completed_work_units.fetch_add(1, Ordering::SeqCst) + 1;
        self.state
            .task_repository
            .update_target(
                &self.task_id,
                TargetUpdate {
                    resource_type: PREFLIGHT_COMMON_RESOURCE_TYPE.into(),
                    resource_key: PREFLIGHT_COMMON_RESOURCE_KEY.into(),
                    state: TargetState::Running,
                    stage: work.stage().into(),
                    progress_current: completed.min(PREFLIGHT_COMMON_WORK_UNITS),
                    progress_total: PREFLIGHT_COMMON_WORK_UNITS,
                    fencing_token: None,
                    message_code: Some(work.finished_code(failed).into()),
                    message_params_json: None,
                },
            )
            .await?;
        let (level, status, message) = if blockers.is_empty() {
            (
                TaskEventLevel::Info,
                "succeeded",
                if checked.is_empty() {
                    format!("{}完成", work.stage())
                } else {
                    format!("{}完成：{checked}均通过", work.stage())
                },
            )
        } else {
            (
                TaskEventLevel::Error,
                "failed",
                format!(
                    "{}失败：{}",
                    work.stage(),
                    blockers
                        .iter()
                        .map(|check| format!("{}：{}", check.label, check.message))
                        .collect::<Vec<_>>()
                        .join("；")
                ),
            )
        };
        self.emit_with_progress(
            None,
            work.stage(),
            status,
            level,
            work.finished_code(failed),
            message,
            completed,
        )
        .await
    }

    async fn target_started(
        &self,
        mac: &str,
        name: Option<&str>,
        ip: Option<&str>,
    ) -> AppResult<()> {
        self.state
            .task_repository
            .update_target(
                &self.task_id,
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: mac.into(),
                    state: TargetState::Running,
                    stage: "检查一体机".into(),
                    progress_current: 0,
                    progress_total: self.target_work_units,
                    fencing_token: None,
                    message_code: Some("PREFLIGHT_TARGET_STARTED".into()),
                    message_params_json: None,
                },
            )
            .await?;
        let identity = match (name, ip) {
            (Some(name), Some(ip)) => format!("{name}（{ip}，MAC {mac}）"),
            _ => format!("MAC {mac}"),
        };
        self.emit(
            Some(mac),
            "检查一体机",
            "running",
            TaskEventLevel::Info,
            "PREFLIGHT_TARGET_STARTED",
            format!("开始检查{identity}；范围：{}", self.target_scope),
        )
        .await
    }

    async fn target_work_started(
        &self,
        mac: &str,
        name: Option<&str>,
        ip: Option<&str>,
        work: TargetWork,
    ) -> AppResult<()> {
        let identity = target_identity(mac, name, ip);
        self.emit_transient_with_progress(
            Some(mac),
            work.stage(),
            "checking",
            TaskEventLevel::Info,
            work.started_code(),
            match work {
                TargetWork::Runtime => {
                    format!("正在检查{identity}的连通性、服务、端口和运行环境")
                }
                TargetWork::Render => format!("正在为{identity}渲染并校验发布模板"),
            },
            self.completed_work_units.load(Ordering::SeqCst),
        )
        .await
    }

    async fn target_work_finished(
        &self,
        mac: &str,
        name: Option<&str>,
        ip: Option<&str>,
        work: TargetWork,
        checks: &[DeploymentPreflightCheck],
        skipped_reason: Option<&str>,
    ) -> AppResult<()> {
        let blockers = checks
            .iter()
            .filter(|check| check.blocking && check.status == PreflightStatus::Failed)
            .collect::<Vec<_>>();
        let warnings = checks
            .iter()
            .filter(|check| check.status == PreflightStatus::Warning)
            .count();
        let failed = !blockers.is_empty();
        let completed = self.completed_work_units.fetch_add(1, Ordering::SeqCst) + 1;
        self.state
            .task_repository
            .update_target(
                &self.task_id,
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: mac.into(),
                    state: TargetState::Running,
                    stage: work.stage().into(),
                    progress_current: work.target_progress().min(self.target_work_units),
                    progress_total: self.target_work_units,
                    fencing_token: None,
                    message_code: Some(if skipped_reason.is_some() {
                        "PREFLIGHT_TARGET_RENDER_SKIPPED".into()
                    } else {
                        work.finished_code(failed).into()
                    }),
                    message_params_json: None,
                },
            )
            .await?;
        let identity = target_identity(mac, name, ip);
        let checked = checks
            .iter()
            .map(|check| check.label.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("、");
        let (status, level, code, message) = if let Some(reason) = skipped_reason {
            (
                "skipped",
                TaskEventLevel::Warn,
                "PREFLIGHT_TARGET_RENDER_SKIPPED",
                format!("{identity}模板渲染已跳过：{reason}"),
            )
        } else if failed {
            (
                "failed",
                TaskEventLevel::Error,
                work.finished_code(true),
                format!(
                    "{identity}{}失败：{}",
                    work.stage(),
                    blockers
                        .iter()
                        .map(|check| format!("{}：{}", check.label, check.message))
                        .collect::<Vec<_>>()
                        .join("；")
                ),
            )
        } else {
            (
                "succeeded",
                if warnings > 0 {
                    TaskEventLevel::Warn
                } else {
                    TaskEventLevel::Info
                },
                work.finished_code(false),
                if warnings > 0 {
                    format!(
                        "{identity}{}完成：{checked}通过，另有{warnings}项提示",
                        work.stage()
                    )
                } else {
                    format!("{identity}{}完成：{checked}均通过", work.stage())
                },
            )
        };
        self.emit_with_progress(
            Some(mac),
            work.stage(),
            status,
            level,
            code,
            message,
            completed,
        )
        .await
    }

    async fn target_finished(
        &self,
        mac: &str,
        name: Option<&str>,
        ip: Option<&str>,
        checks: &[DeploymentPreflightCheck],
    ) -> AppResult<()> {
        let blockers = checks
            .iter()
            .filter(|check| check.blocking && check.status == PreflightStatus::Failed)
            .collect::<Vec<_>>();
        let warnings = checks
            .iter()
            .filter(|check| check.status == PreflightStatus::Warning)
            .count();
        let failed = !blockers.is_empty();
        self.state
            .task_repository
            .update_target(
                &self.task_id,
                TargetUpdate {
                    resource_type: "aio".into(),
                    resource_key: mac.into(),
                    state: if failed {
                        TargetState::Failed
                    } else {
                        TargetState::Succeeded
                    },
                    stage: if failed {
                        "检查失败".into()
                    } else {
                        "检查完成".into()
                    },
                    progress_current: self.target_work_units,
                    progress_total: self.target_work_units,
                    fencing_token: None,
                    message_code: Some(if failed {
                        "PREFLIGHT_TARGET_FAILED".into()
                    } else {
                        "PREFLIGHT_TARGET_PASSED".into()
                    }),
                    message_params_json: None,
                },
            )
            .await?;
        let identity = target_identity(mac, name, ip);
        let checked = checks
            .iter()
            .map(|check| check.label.as_str())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join("、");
        let message = if failed {
            format!(
                "{identity}检查失败；已检查{checked}；原因：{}",
                blockers
                    .iter()
                    .map(|check| format!("{}：{}", check.label, check.message))
                    .collect::<Vec<_>>()
                    .join("；")
            )
        } else if warnings > 0 {
            format!("{identity}检查完成；{checked}的必需条件均通过，另有{warnings}项提示")
        } else {
            format!("{identity}检查完成；{checked}均通过")
        };
        self.emit(
            Some(mac),
            if failed {
                "检查失败"
            } else {
                "检查完成"
            },
            if failed { "failed" } else { "succeeded" },
            if failed {
                TaskEventLevel::Error
            } else {
                TaskEventLevel::Info
            },
            if failed {
                "PREFLIGHT_TARGET_FAILED"
            } else {
                "PREFLIGHT_TARGET_PASSED"
            },
            message,
        )
        .await
    }

    async fn finish(&self, report: &DeploymentPreflightReport) -> AppResult<()> {
        self.finish_unvisited_targets(report).await?;
        if report.ready {
            let snapshot = report
                .execution_snapshot
                .as_ref()
                .ok_or_else(|| AppError::Conflict("部署检查通过但未形成执行快照".into()))?;
            snapshot.validate(&self.project_id)?;
            self.state
                .task_repository
                .bind_preflight_snapshot(
                    &self.task_id,
                    &self.project_id,
                    "deployment_preflight",
                    &snapshot.integrity_sha256()?,
                )
                .await?;
        }
        let blockers = report
            .checks
            .iter()
            .filter(|check| check.blocking && check.status == PreflightStatus::Failed)
            .collect::<Vec<_>>();
        let warnings = report
            .checks
            .iter()
            .filter(|check| check.status == PreflightStatus::Warning)
            .count();
        let next = if report.ready {
            TaskState::Succeeded
        } else {
            TaskState::Failed
        };
        let message = if report.ready {
            if warnings > 0 {
                format!(
                    "部署检查完成：{}台目标均可执行，另有{warnings}项提示",
                    self.total_targets
                )
            } else {
                format!(
                    "部署检查完成：{}台目标全部通过，可以提交部署任务",
                    self.total_targets
                )
            }
        } else {
            format!(
                "部署检查未通过，共{}项阻断：{}",
                blockers.len(),
                blockers
                    .iter()
                    .take(8)
                    .map(|check| check
                        .target_mac
                        .as_deref()
                        .map(|mac| format!("{}（{mac}）：{}", check.label, check.message))
                        .unwrap_or_else(|| format!("{}：{}", check.label, check.message)))
                    .collect::<Vec<_>>()
                    .join("；")
            )
        };
        self.state
            .task_repository
            .transition(
                &self.task_id,
                TaskState::Checking,
                next,
                (!report.ready).then_some("PREFLIGHT_BLOCKED"),
                (!report.ready).then_some("部署执行条件检查未通过"),
            )
            .await?;
        if let Err(error) = self
            .emit_with_progress(
                None,
                if report.ready {
                    "检查完成"
                } else {
                    "检查失败"
                },
                if report.ready { "succeeded" } else { "failed" },
                if report.ready {
                    TaskEventLevel::Info
                } else {
                    TaskEventLevel::Error
                },
                if report.ready {
                    "PREFLIGHT_SUCCEEDED"
                } else {
                    "PREFLIGHT_FAILED"
                },
                message,
                self.total_work_units,
            )
            .await
        {
            tracing::error!(
                error = ?crate::core::log_safety::safe_error(&error),
                task_id = self.task_id,
                "persist final deployment preflight event failed"
            );
        }
        if let Err(error) = TaskDataLifecycle::new(&self.state.paths).finalize_task(
            &self.project_id,
            &self.task_id,
            next,
        ) {
            tracing::warn!(
                error = ?crate::core::log_safety::safe_error(&error),
                task_id = self.task_id,
                "finalize deployment preflight data lifecycle failed"
            );
        }
        Ok(())
    }

    async fn interrupt(&self, error: &AppError) -> AppResult<()> {
        let safe_message = self
            .state
            .task_event_pipeline
            .redact_text(&error.to_string());
        let targets = self.state.task_repository.targets(&self.task_id).await?;
        for target in targets {
            if matches!(target.state, TargetState::Pending | TargetState::Running) {
                self.state
                    .task_repository
                    .update_target(
                        &self.task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: TargetState::Interrupted,
                            stage: "检查中断".into(),
                            progress_current: target.progress_total,
                            progress_total: target.progress_total,
                            fencing_token: target.fencing_token,
                            message_code: Some("PREFLIGHT_INTERRUPTED".into()),
                            message_params_json: None,
                        },
                    )
                    .await?;
            }
        }
        let task = self.state.task_repository.get(&self.task_id).await?;
        match task.state {
            TaskState::Draft => {
                self.state
                    .task_repository
                    .transition(
                        &self.task_id,
                        TaskState::Draft,
                        TaskState::Checking,
                        None,
                        Some("正在收敛未完成的部署检查"),
                    )
                    .await?;
                self.state
                    .task_repository
                    .transition(
                        &self.task_id,
                        TaskState::Checking,
                        TaskState::Interrupted,
                        Some("PREFLIGHT_INTERRUPTED"),
                        Some(&safe_message),
                    )
                    .await?;
            }
            TaskState::Checking => {
                self.state
                    .task_repository
                    .transition(
                        &self.task_id,
                        TaskState::Checking,
                        TaskState::Interrupted,
                        Some("PREFLIGHT_INTERRUPTED"),
                        Some(&safe_message),
                    )
                    .await?;
            }
            state if state.is_terminal() => return Ok(()),
            _ => {
                return Err(AppError::Conflict(format!(
                    "部署检查任务状态无法收敛：{}",
                    task.state.as_str()
                )));
            }
        }
        if let Err(event_error) = self
            .emit_with_progress(
                None,
                "检查中断",
                "interrupted",
                TaskEventLevel::Error,
                "PREFLIGHT_INTERRUPTED",
                format!("部署检查异常中断：{safe_message}"),
                self.total_work_units,
            )
            .await
        {
            tracing::error!(
                error = ?crate::core::log_safety::safe_error(&event_error),
                task_id = self.task_id,
                "persist interrupted deployment preflight event failed"
            );
        }
        if let Err(lifecycle_error) = TaskDataLifecycle::new(&self.state.paths).finalize_task(
            &self.project_id,
            &self.task_id,
            TaskState::Interrupted,
        ) {
            tracing::warn!(
                error = ?crate::core::log_safety::safe_error(&lifecycle_error),
                task_id = self.task_id,
                "finalize interrupted deployment preflight data lifecycle failed"
            );
        }
        Ok(())
    }

    async fn finish_unvisited_targets(&self, report: &DeploymentPreflightReport) -> AppResult<()> {
        let common_failed = report.checks.iter().any(|check| {
            check.target_mac.is_none() && check.blocking && check.status == PreflightStatus::Failed
        });
        let targets = self.state.task_repository.targets(&self.task_id).await?;
        for target in targets {
            if matches!(target.state, TargetState::Pending | TargetState::Running) {
                let internal = target.resource_type == PREFLIGHT_COMMON_RESOURCE_TYPE
                    && target.resource_key == PREFLIGHT_COMMON_RESOURCE_KEY;
                let target_failed = common_failed
                    || (!internal
                        && report.checks.iter().any(|check| {
                            check.target_mac.as_deref() == Some(target.resource_key.as_str())
                                && check.blocking
                                && check.status == PreflightStatus::Failed
                        }));
                self.state
                    .task_repository
                    .update_target(
                        &self.task_id,
                        TargetUpdate {
                            resource_type: target.resource_type,
                            resource_key: target.resource_key,
                            state: if target_failed {
                                TargetState::Failed
                            } else {
                                TargetState::Succeeded
                            },
                            stage: if target_failed {
                                "检查失败".into()
                            } else {
                                "检查完成".into()
                            },
                            progress_current: target.progress_total,
                            progress_total: target.progress_total,
                            fencing_token: target.fencing_token,
                            message_code: Some(if target_failed {
                                "PREFLIGHT_TARGET_FAILED".into()
                            } else {
                                "PREFLIGHT_TARGET_PASSED".into()
                            }),
                            message_params_json: None,
                        },
                    )
                    .await?;
            }
        }
        self.completed_work_units
            .store(self.total_work_units, Ordering::SeqCst);
        Ok(())
    }

    async fn emit(
        &self,
        resource_key: Option<&str>,
        stage: &str,
        status: &str,
        level: TaskEventLevel,
        message_code: &str,
        message: String,
    ) -> AppResult<()> {
        self.emit_with_progress(
            resource_key,
            stage,
            status,
            level,
            message_code,
            message,
            self.completed_work_units.load(Ordering::SeqCst),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn emit_with_progress(
        &self,
        resource_key: Option<&str>,
        stage: &str,
        status: &str,
        level: TaskEventLevel,
        message_code: &str,
        message: String,
        progress_current: u64,
    ) -> AppResult<()> {
        self.state
            .task_event_pipeline
            .emit(
                &self.task_id,
                TaskEventInput {
                    resource_type: resource_key.map(|_| "aio".into()),
                    resource_key: resource_key.map(str::to_string),
                    stage: stage.into(),
                    status: status.into(),
                    progress_current: Some(progress_current.min(self.total_work_units)),
                    progress_total: Some(self.total_work_units),
                    level,
                    message_code: message_code.into(),
                    message_params: BTreeMap::new(),
                    message: Some(message),
                },
            )
            .await?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn emit_transient_with_progress(
        &self,
        resource_key: Option<&str>,
        stage: &str,
        status: &str,
        level: TaskEventLevel,
        message_code: &str,
        message: String,
        progress_current: u64,
    ) -> AppResult<()> {
        self.state
            .task_event_pipeline
            .emit_transient(
                &self.task_id,
                TaskEventInput {
                    resource_type: resource_key.map(|_| "aio".into()),
                    resource_key: resource_key.map(str::to_string),
                    stage: stage.into(),
                    status: status.into(),
                    progress_current: Some(progress_current.min(self.total_work_units)),
                    progress_total: Some(self.total_work_units),
                    level,
                    message_code: message_code.into(),
                    message_params: BTreeMap::new(),
                    message: Some(message),
                },
            )
            .await?;
        Ok(())
    }
}

fn target_identity(mac: &str, name: Option<&str>, ip: Option<&str>) -> String {
    match (name, ip) {
        (Some(name), Some(ip)) => format!("{name}（{ip}，MAC {mac}）"),
        _ => format!("MAC {mac}"),
    }
}

fn target_work_units(mode: DeploymentMode) -> u64 {
    if mode == DeploymentMode::ServiceUpgrade {
        1
    } else {
        2
    }
}

fn preflight_work_units(mode: DeploymentMode, target_count: u64) -> AppResult<u64> {
    PREFLIGHT_COMMON_WORK_UNITS
        .checked_add(
            target_count
                .checked_mul(target_work_units(mode))
                .ok_or_else(|| AppError::InvalidConfig("部署预检事项数量超出范围".into()))?,
        )
        .ok_or_else(|| AppError::InvalidConfig("部署预检事项数量超出范围".into()))
}

fn deployment_mode_label(mode: DeploymentMode) -> &'static str {
    match mode {
        DeploymentMode::FirstDeploy => "首次部署",
        DeploymentMode::FullUpgrade => "整包升级",
        DeploymentMode::ServiceUpgrade => "单服升级",
    }
}

#[allow(clippy::too_many_arguments)]
async fn append_remote_runtime_checks(
    checks: &mut Vec<DeploymentPreflightCheck>,
    mac: &str,
    target: &RemoteTarget,
    profile: &ReleaseProfileRecord,
    connector: &ObservedConnector<RusshConnector>,
    runtime: &ReleaseRuntime,
    mode: DeploymentMode,
    published_ports: &[u16],
) -> Option<HostKeyRecord> {
    let auth = match release_remote_auth(&profile.credentials) {
        Ok(auth) => auth,
        Err(error) => {
            checks.push(failed(
                "ssh_auth",
                "SSH认证",
                Some(mac),
                error.to_string(),
                remediation(
                    "open_release_profile",
                    "维护SSH凭据",
                    Some("/aio/release"),
                    Some(mac),
                ),
            ));
            return None;
        }
    };
    let (connection, observation) = match connector.connect_observed(target, &auth).await {
        Ok(connected) => {
            checks.push(passed(
                "ssh_auth",
                "SSH认证",
                Some(mac),
                format!("{}:{}连接和登录验证通过", target.host, target.port),
            ));
            connected
        }
        Err(error) => {
            checks.push(failed(
                "ssh_auth",
                "SSH认证",
                Some(mac),
                error.to_string(),
                remediation(
                    "open_release_profile",
                    "检查SSH连接和凭据",
                    Some("/aio/release"),
                    Some(mac),
                ),
            ));
            return None;
        }
    };
    if observation.changed() {
        checks.push(warning(
            "host_key_changed",
            "主机指纹变化",
            Some(mac),
            observation.message(),
        ));
    }
    checks.push(remote_endpoint_check(&connection, &profile.values, mac).await);
    let cancellation = CancellationToken::new();
    enum Expectation<'a> {
        Exact(&'a str),
        Minimum(&'a str),
    }
    for (code, label, program, args, expectation) in [
        (
            "runtime_os",
            "操作系统",
            "uname",
            vec!["-s".into()],
            Expectation::Exact(runtime.os.as_str()),
        ),
        (
            "runtime_arch",
            "运行环境兼容性",
            "uname",
            vec!["-m".into()],
            Expectation::Exact(runtime.arch.as_str()),
        ),
        (
            "docker",
            "Docker",
            "docker",
            vec![
                "version".into(),
                "--format".into(),
                "{{.Server.Version}}".into(),
            ],
            Expectation::Minimum(runtime.docker.as_str()),
        ),
        (
            "docker_compose",
            "Docker Compose",
            "docker",
            vec!["compose".into(), "version".into()],
            Expectation::Minimum(runtime.compose.as_str()),
        ),
    ] {
        let request = ExecRequest {
            program: program.into(),
            args,
            env: BTreeMap::new(),
            stdin: None,
            total_timeout: Duration::from_secs(30),
            inactivity_timeout: Duration::from_secs(15),
        };
        let mut result = connection
            .run(&request, &cancellation, &NoopRemoteOutputSink)
            .await;
        if code == "docker_compose" && !matches!(&result, Ok(value) if value.exit_status == 0) {
            let fallback = ExecRequest {
                program: "docker-compose".into(),
                args: vec!["version".into()],
                ..request
            };
            result = connection
                .run(&fallback, &cancellation, &NoopRemoteOutputSink)
                .await;
        }
        match result {
            Ok(result) if result.exit_status == 0 => {
                let output = result.stdout.lines().next().unwrap_or("").trim();
                let (satisfied, message) = match expectation {
                    Expectation::Exact(expected) => (
                        output.eq_ignore_ascii_case(expected),
                        if output.eq_ignore_ascii_case(expected) {
                            format!("运行环境符合发布物要求（{expected}）")
                        } else {
                            format!(
                                "一体机运行环境与发布物不匹配，要求{expected}；请检查发布物或一体机环境"
                            )
                        },
                    ),
                    Expectation::Minimum(requirement) => {
                        match runtime_version_satisfies(output, requirement) {
                            Ok(satisfied) => (
                                satisfied,
                                if satisfied {
                                    format!("远端版本满足{requirement}约束")
                                } else {
                                    format!("远端版本不满足{requirement}要求，请更新运行环境")
                                },
                            ),
                            Err(_) => (false, format!("远端版本无法按{requirement}约束解析")),
                        }
                    }
                };
                if satisfied {
                    checks.push(passed(code, label, Some(mac), message));
                } else {
                    checks.push(failed(
                        code,
                        label,
                        Some(mac),
                        message,
                        remediation("repair_remote_runtime", "修复远端运行环境", None, Some(mac)),
                    ));
                }
            }
            Ok(result) => checks.push(failed(
                code,
                label,
                Some(mac),
                format!("退出码{}，未取得可验证运行时版本", result.exit_status),
                remediation("repair_remote_runtime", "修复远端运行环境", None, Some(mac)),
            )),
            Err(error) => checks.push(failed(
                code,
                label,
                Some(mac),
                error.to_string(),
                remediation("repair_remote_runtime", "修复远端运行环境", None, Some(mac)),
            )),
        }
    }
    checks.extend(
        remote_environment_checks(&connection, &profile.values, mac, mode, published_ports).await,
    );
    if let Err(error) = connection.disconnect().await {
        checks.push(warning(
            "ssh_disconnect",
            "SSH会话关闭",
            Some(mac),
            error.to_string(),
        ));
    }
    Some(observation.record)
}

struct InspectedArtifact {
    normalized: DeploymentPlanInput,
    fingerprint: String,
    runtime: ReleaseRuntime,
}

fn normalize_and_inspect_artifact(
    input: &DeploymentPlanInput,
    profile: &ReleaseProfileRecord,
    checked_at: &str,
) -> (AppResult<InspectedArtifact>, DeploymentPreflightCheck) {
    let result = (|| {
        let mut normalized = input.clone();
        normalized.target_macs = input
            .target_macs
            .iter()
            .map(|mac| MacAddress::parse(mac).map(|value| value.normalized().to_string()))
            .collect::<AppResult<Vec<_>>>()?;
        normalized.target_macs.sort();
        normalized.target_macs.dedup();
        if normalized.target_macs.is_empty() {
            return Err(AppError::InvalidConfig("部署目标不能为空".into()));
        }
        let compose_services = inspect_compose_services(&profile.values.compose_template)?;
        let configured = compose_services
            .iter()
            .map(|service| service.name.as_str())
            .collect::<BTreeSet<_>>();
        let inspected = inspect_deployment_images(&normalized.image_files)?;
        normalized.image_files = inspected.image_files;
        let selected = normalized
            .image_files
            .iter()
            .map(|image| image.service_name.as_str())
            .collect::<BTreeSet<_>>();
        match normalized.mode {
            DeploymentMode::FirstDeploy | DeploymentMode::FullUpgrade if selected != configured => {
                let missing = configured
                    .difference(&selected)
                    .copied()
                    .collect::<Vec<_>>();
                let extra = selected
                    .difference(&configured)
                    .copied()
                    .collect::<Vec<_>>();
                return Err(AppError::InvalidConfig(format!(
                    "镜像选择与Compose服务不一致；缺少：{}；多余：{}",
                    if missing.is_empty() {
                        "无".into()
                    } else {
                        missing.join("、")
                    },
                    if extra.is_empty() {
                        "无".into()
                    } else {
                        extra.join("、")
                    }
                )));
            }
            DeploymentMode::ServiceUpgrade => {
                if normalized.image_files.len() != 1 {
                    return Err(AppError::InvalidConfig(
                        "单服升级只能选择一个服务镜像".into(),
                    ));
                }
                let service = normalized
                    .image_files
                    .first()
                    .map(|image| image.service_name.as_str())
                    .ok_or_else(|| AppError::InvalidConfig("请选择单服升级镜像".into()))?;
                let descriptor = compose_services
                    .iter()
                    .find(|candidate| candidate.name == service)
                    .ok_or_else(|| {
                        AppError::InvalidConfig(format!("服务 {service} 不在当前Compose配置中"))
                    })?;
                normalized.service_image_environment_variable =
                    Some(descriptor.image_environment_variable.clone());
            }
            _ => {}
        }
        normalized.artifact_name = if normalized.mode == DeploymentMode::ServiceUpgrade {
            normalized.image_files[0].service_name.clone()
        } else {
            format!("{}个服务镜像", normalized.image_files.len())
        };
        normalized.artifact_version = if normalized.mode == DeploymentMode::ServiceUpgrade {
            normalized.image_files[0]
                .image_tag
                .rsplit_once(':')
                .map(|(_, version)| version.to_string())
                .unwrap_or_else(|| "latest".into())
        } else {
            let time_part = checked_at
                .chars()
                .filter(|value| value.is_ascii_digit())
                .take(20)
                .collect::<String>();
            format!("bundle-{time_part}-{}", &inspected.fingerprint[..8])
        };
        normalized.artifact_path = normalized.image_files[0].file_path.clone();
        validate_release_version(&normalized.artifact_version)?;
        normalized.images = normalized
            .image_files
            .iter()
            .map(|image| (image.service_name.clone(), image.image_tag.clone()))
            .collect();
        if normalized.mode == DeploymentMode::ServiceUpgrade {
            normalized.service_name = Some(normalized.image_files[0].service_name.clone());
            normalized.image_name = Some(normalized.image_files[0].image_tag.clone());
        } else {
            normalized.service_name = None;
            normalized.image_name = None;
        }
        Ok(InspectedArtifact {
            normalized,
            fingerprint: inspected.fingerprint,
            runtime: standard_runtime(),
        })
    })();
    let check = match &result {
        Ok(inspected) => passed(
            "artifact",
            "发布物",
            None,
            format!(
                "{} {} 的镜像文件、RepoTag与Compose服务均已校验",
                inspected.normalized.artifact_name, inspected.normalized.artifact_version
            ),
        ),
        Err(error) => failed(
            "artifact",
            "发布物",
            None,
            error.to_string(),
            remediation("select_artifact", "重新选择发布物", None, None),
        ),
    };
    (result, check)
}

fn validate_batch_and_finalize_artifact(
    inspected: InspectedArtifact,
) -> (AppResult<InspectedArtifact>, DeploymentPreflightCheck) {
    let result: AppResult<InspectedArtifact> = (|| {
        validate_deployment_batch(
            inspected.normalized.batch_size,
            inspected.normalized.concurrency,
            inspected.normalized.target_macs.len(),
        )?;
        Ok(inspected)
    })();
    let check = match &result {
        Ok(inspected) => passed(
            "deployment_batch",
            "批次策略",
            None,
            format!(
                "一批{}台、并发{}台，适用于当前{}台目标",
                inspected.normalized.batch_size,
                inspected.normalized.concurrency,
                inspected.normalized.target_macs.len()
            ),
        ),
        Err(error) => failed(
            "deployment_batch",
            "批次策略",
            None,
            error.to_string(),
            remediation("adjust_batch", "调整批次与并发", None, None),
        ),
    };
    (result, check)
}

fn render_context(
    node: &crate::domain::aio::inventory::WorkbenchNodeSnapshot,
    profile: &ReleaseProfileRecord,
    plan: &DeploymentPlanInput,
) -> ReleaseRenderContext {
    ReleaseRenderContext {
        release_version: plan.artifact_version.clone(),
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
        images: plan.images.clone(),
    }
}

fn report(
    checks: Vec<DeploymentPreflightCheck>,
    normalized: DeploymentPlanInput,
    profile_version: Option<u64>,
    checked_at: String,
) -> DeploymentPreflightReport {
    DeploymentPreflightReport::from_checks(checks, normalized, profile_version, None, checked_at)
}

pub(super) fn passed(
    code: &str,
    label: &str,
    target_mac: Option<&str>,
    message: String,
) -> DeploymentPreflightCheck {
    DeploymentPreflightCheck {
        code: code.into(),
        label: label.into(),
        status: PreflightStatus::Passed,
        blocking: false,
        target_mac: target_mac.map(str::to_string),
        message,
        remediation: None,
    }
}

pub(super) fn failed(
    code: &str,
    label: &str,
    target_mac: Option<&str>,
    message: String,
    remediation: PreflightRemediation,
) -> DeploymentPreflightCheck {
    DeploymentPreflightCheck {
        code: code.into(),
        label: label.into(),
        status: PreflightStatus::Failed,
        blocking: true,
        target_mac: target_mac.map(str::to_string),
        message,
        remediation: Some(remediation),
    }
}

fn warning(
    code: &str,
    label: &str,
    target_mac: Option<&str>,
    message: String,
) -> DeploymentPreflightCheck {
    DeploymentPreflightCheck {
        code: code.into(),
        label: label.into(),
        status: PreflightStatus::Warning,
        blocking: false,
        target_mac: target_mac.map(str::to_string),
        message,
        remediation: None,
    }
}

pub(super) fn remediation(
    action: &str,
    label: &str,
    route: Option<&str>,
    target: Option<&str>,
) -> PreflightRemediation {
    PreflightRemediation {
        action: action.into(),
        label: label.into(),
        route: route.map(str::to_string),
        target: target.map(str::to_string),
    }
}

fn timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 timestamp")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::{
        CommonWork, PREFLIGHT_COMMON_RESOURCE_TYPE, PreflightTaskTracker, TargetWork, failed,
        passed, preflight_work_units, warning,
    };
    use crate::core::error::AppError;
    use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlanInput};
    use crate::domain::aio::deployment_workflow::{
        DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION, DeploymentExecutionSnapshot, DeploymentPreflightReport,
        DeploymentTargetSnapshot, PreflightRemediation, PreflightStatus,
    };
    use crate::domain::aio::inventory::WorkbenchNodeSnapshot;
    use crate::domain::common::task::{TargetState, TaskState};
    use crate::formal::app_state::FormalAppState;
    use crate::formal::config::AppPaths;
    use crate::formal::job_supervisor::JobSupervisor;
    use crate::formal::local_store::LocalStore;
    use crate::formal::runtime_registry::ProjectRuntimeRegistry;
    use crate::formal::secret_store::MemorySecretStore;
    use crate::infrastructure::local_sqlite::task_repository::TaskRepository;
    use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
    use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
    use crate::interface::commands::task_activity::query_activity_tasks;
    use crate::runtime::event_bus::TaskEventBus;
    use crate::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};

    #[test]
    fn a_changed_host_key_is_only_a_non_blocking_warning() {
        let check = warning(
            "host_key_changed",
            "主机指纹变化",
            Some("001122334455"),
            "已自动记录并继续".into(),
        );
        assert_eq!(check.status, PreflightStatus::Warning);
        assert!(!check.blocking);
        assert!(check.remediation.is_none());
    }

    #[test]
    fn unexecuted_remote_gate_is_blocking() {
        let check = failed(
            "remote_runtime",
            "远端运行时",
            Some("001122334455"),
            "未执行".into(),
            PreflightRemediation {
                action: "run".into(),
                label: "执行".into(),
                route: None,
                target: Some("001122334455".into()),
            },
        );
        assert_eq!(check.status, PreflightStatus::Failed);
        assert!(check.blocking);
    }

    #[test]
    fn work_unit_total_matches_mode_and_target_count() {
        assert_eq!(
            preflight_work_units(DeploymentMode::FirstDeploy, 2).expect("first deploy units"),
            7
        );
        assert_eq!(
            preflight_work_units(DeploymentMode::FullUpgrade, 2).expect("full upgrade units"),
            7
        );
        assert_eq!(
            preflight_work_units(DeploymentMode::ServiceUpgrade, 2).expect("service upgrade units"),
            5
        );
    }

    #[tokio::test]
    async fn tracked_preflight_normalizes_formatted_mac_for_creation_updates_and_completion() {
        let temp = tempfile::tempdir().expect("temporary app data");
        let paths = AppPaths::from_data_dir(temp.path()).expect("app paths");
        paths.ensure().expect("ensure paths");
        let local_store = LocalStore::open(&paths.local_db)
            .await
            .expect("local store");
        sqlx::query(
            "INSERT INTO local_project \
             (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
              db_password_secret_ref, created_at, updated_at) \
             VALUES ('project', 'Project', 'http://platform.test', 'db.test', 3306, 'user', \
                     'business', 'workbench', 'secret-ref', '1', '1')",
        )
        .execute(local_store.pool())
        .await
        .expect("project fixture");
        let task_repository = TaskRepository::new(local_store.pool().clone());
        let task_event_bus = TaskEventBus::new(32).expect("event bus");
        let mut task_events = task_event_bus.subscribe();
        let task_event_pipeline = TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus.clone(),
            SensitiveValueRedactor::default(),
        );
        let job_supervisor = JobSupervisor::default();
        let task_handler_registry = TaskHandlerRegistry::default();
        let task_queue =
            TaskQueue::start(8, 2, task_handler_registry.clone(), job_supervisor.clone())
                .await
                .expect("task queue");
        let state = FormalAppState {
            local_store,
            secret_store: Arc::new(MemorySecretStore::default()),
            runtime_registry: ProjectRuntimeRegistry::default(),
            job_supervisor,
            task_recovery_registry:
                crate::infrastructure::task_handlers::built_in_recovery_registry(),
            task_handler_registry,
            task_queue,
            task_event_bus,
            task_repository: task_repository.clone(),
            task_event_pipeline,
            paths,
        };
        let input = DeploymentPlanInput {
            mode: DeploymentMode::FullUpgrade,
            target_macs: vec!["00:11:22:aa:bb:cc".into()],
            image_files: vec![],
            artifact_path: String::new(),
            artifact_name: String::new(),
            artifact_version: String::new(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: BTreeMap::new(),
            batch_size: 1,
            concurrency: 1,
        };
        let task_id = uuid::Uuid::now_v7().to_string();
        let tracker = PreflightTaskTracker::start(&state, "project", &task_id, &input)
            .await
            .expect("start tracked preflight");
        let targets = task_repository.targets(&task_id).await.expect("targets");
        let target = targets
            .iter()
            .find(|target| target.resource_type == "aio")
            .expect("aio target");
        assert_eq!(target.resource_key, "001122AABBCC");
        assert_eq!(target.progress_current, 0);
        assert_eq!(target.progress_total, 2);
        let common = targets
            .iter()
            .find(|target| target.resource_type == PREFLIGHT_COMMON_RESOURCE_TYPE)
            .expect("common progress target");
        assert_eq!(common.progress_current, 0);
        assert_eq!(common.progress_total, 3);
        tracker
            .common_finished(
                CommonWork::Parameters,
                &[passed(
                    "release_profile",
                    "发布参数",
                    None,
                    "参数有效".into(),
                )],
            )
            .await
            .expect("finish parameter work");
        tracker.images_started(1).await.expect("start image work");
        tracker
            .common_finished(
                CommonWork::Images,
                &[passed("artifact", "发布物", None, "镜像有效".into())],
            )
            .await
            .expect("finish image work");
        tracker
            .common_started(CommonWork::Batch)
            .await
            .expect("start batch work");
        tracker
            .common_finished(
                CommonWork::Batch,
                &[passed(
                    "deployment_batch",
                    "批次策略",
                    None,
                    "批次有效".into(),
                )],
            )
            .await
            .expect("finish batch work");
        tracker
            .target_started("001122AABBCC", Some("测试一体机"), Some("192.0.2.10"))
            .await
            .expect("start normalized target");
        tracker
            .target_work_started(
                "001122AABBCC",
                Some("测试一体机"),
                Some("192.0.2.10"),
                TargetWork::Runtime,
            )
            .await
            .expect("start target runtime work");
        let target_checks = vec![
            passed(
                "ssh_auth",
                "SSH认证",
                Some("001122AABBCC"),
                "连接通过".into(),
            ),
            passed(
                "release_render",
                "发布渲染",
                Some("001122AABBCC"),
                "渲染通过".into(),
            ),
        ];
        tracker
            .target_work_finished(
                "001122AABBCC",
                Some("测试一体机"),
                Some("192.0.2.10"),
                TargetWork::Runtime,
                &target_checks[..1],
                None,
            )
            .await
            .expect("finish target runtime work");
        tracker
            .target_work_started(
                "001122AABBCC",
                Some("测试一体机"),
                Some("192.0.2.10"),
                TargetWork::Render,
            )
            .await
            .expect("start target render work");
        tracker
            .target_work_finished(
                "001122AABBCC",
                Some("测试一体机"),
                Some("192.0.2.10"),
                TargetWork::Render,
                &target_checks[1..],
                None,
            )
            .await
            .expect("finish target render work");
        tracker
            .target_finished(
                "001122AABBCC",
                Some("测试一体机"),
                Some("192.0.2.10"),
                &target_checks,
            )
            .await
            .expect("finish normalized target");
        tracker
            .finish(&DeploymentPreflightReport::from_checks(
                target_checks,
                input.clone(),
                Some(1),
                Some(DeploymentExecutionSnapshot {
                    schema_version: DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION,
                    local_project_id: "project".into(),
                    checked_at: "2026-09-05T00:00:00Z".into(),
                    profile_version: 1,
                    artifact_fingerprint: "a".repeat(64),
                    plan: DeploymentPlanInput {
                        target_macs: vec!["001122AABBCC".into()],
                        ..input.clone()
                    },
                    targets: vec![DeploymentTargetSnapshot {
                        node: WorkbenchNodeSnapshot {
                            mac_normalized: "001122AABBCC".into(),
                            name: "测试一体机".into(),
                            ip: "192.0.2.10".into(),
                            building_id: None,
                            region_id: None,
                            addr_alias: None,
                            floor: None,
                            location: None,
                            remark: None,
                            platform_aio_id: None,
                            management_state: "pending".into(),
                            source: "import".into(),
                            last_operation_id: None,
                            version: 1,
                        },
                        ssh_host: "192.0.2.10".into(),
                        ssh_port: 22,
                        host_key_algorithm: "ssh-ed25519".into(),
                        host_key_fingerprint: "SHA256:test".into(),
                        host_key_accepted_at: "2026-09-05T00:00:00Z".into(),
                    }],
                }),
                "2026-09-05T00:00:00Z".into(),
            ))
            .await
            .expect("finish tracked preflight");
        let targets = task_repository.targets(&task_id).await.expect("targets");
        let target = targets
            .iter()
            .find(|target| target.resource_type == "aio")
            .expect("aio target");
        assert_eq!(target.resource_key, "001122AABBCC");
        assert_eq!(target.state, TargetState::Succeeded);
        assert_eq!(target.progress_current, 2);
        assert_eq!(target.progress_total, 2);
        let common = targets
            .iter()
            .find(|target| target.resource_type == PREFLIGHT_COMMON_RESOURCE_TYPE)
            .expect("common progress target");
        assert_eq!(common.state, TargetState::Succeeded);
        assert_eq!(common.progress_current, 3);
        assert_eq!(common.progress_total, 3);
        assert_eq!(
            task_repository.get(&task_id).await.expect("task").state,
            TaskState::Succeeded
        );
        let activity = query_activity_tasks(&state, "project", 10)
            .await
            .expect("activity tasks");
        assert_eq!(activity[0].target_count, 1);
        assert_eq!(activity[0].completed_count, 1);
        assert_eq!(activity[0].progress, Some(100));
        let mut progress = Vec::new();
        while let Ok(event) = task_events.try_recv() {
            if matches!(
                event.message_code.as_str(),
                "PREFLIGHT_STARTED"
                    | "PREFLIGHT_PARAMETERS_FINISHED"
                    | "PREFLIGHT_IMAGES_FINISHED"
                    | "PREFLIGHT_BATCH_FINISHED"
                    | "PREFLIGHT_TARGET_RUNTIME_FINISHED"
                    | "PREFLIGHT_TARGET_RENDER_FINISHED"
                    | "PREFLIGHT_SUCCEEDED"
            ) {
                progress.push((
                    event.message_code,
                    event.progress_current,
                    event.progress_total,
                ));
            }
        }
        assert_eq!(
            progress,
            vec![
                ("PREFLIGHT_STARTED".into(), Some(0), Some(5)),
                ("PREFLIGHT_PARAMETERS_FINISHED".into(), Some(1), Some(5)),
                ("PREFLIGHT_IMAGES_FINISHED".into(), Some(2), Some(5)),
                ("PREFLIGHT_BATCH_FINISHED".into(), Some(3), Some(5)),
                ("PREFLIGHT_TARGET_RUNTIME_FINISHED".into(), Some(4), Some(5)),
                ("PREFLIGHT_TARGET_RENDER_FINISHED".into(), Some(5), Some(5)),
                ("PREFLIGHT_SUCCEEDED".into(), Some(5), Some(5)),
            ]
        );

        let early_task_id = uuid::Uuid::now_v7().to_string();
        let mut early_events = state.task_event_bus.subscribe();
        let early_tracker = PreflightTaskTracker::start(&state, "project", &early_task_id, &input)
            .await
            .expect("start early-failure preflight");
        let parameter_failure = failed(
            "release_profile",
            "发布参数",
            None,
            "参数无效".into(),
            PreflightRemediation {
                action: "edit".into(),
                label: "修改".into(),
                route: None,
                target: None,
            },
        );
        early_tracker
            .common_finished(
                CommonWork::Parameters,
                std::slice::from_ref(&parameter_failure),
            )
            .await
            .expect("finish failed parameter work");
        early_tracker
            .finish(&DeploymentPreflightReport::from_checks(
                vec![parameter_failure],
                input.clone(),
                None,
                None,
                "2026-09-05T00:00:01Z".into(),
            ))
            .await
            .expect("converge early-failure preflight");
        let early_progress = std::iter::from_fn(|| early_events.try_recv().ok())
            .filter_map(|event| {
                Some((
                    event.progress_current?,
                    event.progress_total?,
                    event.message_code,
                ))
            })
            .collect::<Vec<_>>();
        assert!(
            early_progress
                .windows(2)
                .all(|window| window[0].0 <= window[1].0)
        );
        assert_eq!(
            early_progress.last(),
            Some(&(5, 5, "PREFLIGHT_FAILED".into()))
        );
        assert_eq!(
            task_repository
                .get(&early_task_id)
                .await
                .expect("early task")
                .state,
            TaskState::Failed
        );

        let interrupted_task_id = uuid::Uuid::now_v7().to_string();
        let mut interrupted_events = state.task_event_bus.subscribe();
        let interrupted_tracker =
            PreflightTaskTracker::start(&state, "project", &interrupted_task_id, &input)
                .await
                .expect("start interrupted preflight");
        interrupted_tracker
            .interrupt(&AppError::Conflict("模拟异常".into()))
            .await
            .expect("converge interrupted preflight");
        let interrupted_progress = std::iter::from_fn(|| interrupted_events.try_recv().ok())
            .filter_map(|event| {
                Some((
                    event.progress_current?,
                    event.progress_total?,
                    event.message_code,
                ))
            })
            .collect::<Vec<_>>();
        assert!(
            interrupted_progress
                .windows(2)
                .all(|window| window[0].0 <= window[1].0)
        );
        assert_eq!(
            interrupted_progress.last(),
            Some(&(5, 5, "PREFLIGHT_INTERRUPTED".into()))
        );
        assert_eq!(
            task_repository
                .get(&interrupted_task_id)
                .await
                .expect("interrupted task")
                .state,
            TaskState::Interrupted
        );
        state
            .task_queue
            .shutdown(std::time::Duration::from_secs(1))
            .await;
        state.local_store.close().await;
    }
}
