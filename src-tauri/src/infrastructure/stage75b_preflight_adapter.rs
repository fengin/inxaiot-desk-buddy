use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::application::ports::deployment_workflow::DeploymentPreflightPort;
use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use crate::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};
use crate::domain::aio::deployment_workflow::{
    DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION, DeploymentExecutionSnapshot, DeploymentPreflightCheck,
    DeploymentPreflightReport, DeploymentTargetSnapshot, PreflightRemediation, PreflightStatus,
};
use crate::domain::aio::mac::MacAddress;
use crate::domain::aio::release::{
    ReleaseRuntime, inspect_image_archive, inspect_release_directory, runtime_version_satisfies,
    sha256_file,
};
use crate::formal::app_state::FormalAppState;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::release_master_key::ReleaseMasterKeyManager;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::infrastructure::aio_assets_service::{application_instance_id, project_operator};
use crate::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use crate::infrastructure::project_context::{map_formal_error, project_database};
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::workbench_aio::WorkbenchAioRepository;

pub struct Stage75BPreflightAdapter<'a> {
    state: &'a FormalAppState,
}

impl<'a> Stage75BPreflightAdapter<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl DeploymentPreflightPort for Stage75BPreflightAdapter<'_> {
    async fn preflight(
        &self,
        project_id: &str,
        input: &DeploymentPlanInput,
    ) -> AppResult<DeploymentPreflightReport> {
        let checked_at = timestamp();
        let (inspected, artifact_check) = normalize_and_inspect_artifact(input);
        let mut checks = vec![artifact_check];
        let inspected = match inspected {
            Ok(value) => value,
            Err(_) => {
                return Ok(DeploymentPreflightReport::from_checks(
                    checks,
                    input.clone(),
                    None,
                    None,
                    checked_at,
                ));
            }
        };
        let normalized = inspected.normalized;
        let artifact_fingerprint = inspected.fingerprint;
        let runtime = inspected.runtime;

        let operator = match project_operator(self.state, project_id).await {
            Ok(operator) => {
                checks.push(passed(
                    "project_session",
                    "平台会话",
                    None,
                    format!("已登录：{operator}"),
                ));
                Some(operator)
            }
            Err(error) => {
                checks.push(failed(
                    "project_session",
                    "平台会话",
                    None,
                    error.to_string(),
                    remediation("login_project", "重新登录", None, None),
                ));
                None
            }
        };

        let pools = match project_database(self.state, project_id).await {
            Ok(pools) => {
                checks.push(passed(
                    "project_database",
                    "项目数据库与Schema",
                    None,
                    "平台库只读连接与工作台Schema已就绪".into(),
                ));
                pools
            }
            Err(error) => {
                checks.push(failed(
                    "project_database",
                    "项目数据库与Schema",
                    None,
                    error.to_string(),
                    remediation("open_project_settings", "检查项目连接", None, None),
                ));
                return Ok(report(checks, normalized, None, checked_at));
            }
        };
        let projects = LocalProjectRepository::new(
            self.state.local_store.pool().clone(),
            self.state.secret_store.clone(),
        );
        let connection = match projects.connection_secrets(project_id).await {
            Ok(value) => value,
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
                return Ok(report(checks, normalized, None, checked_at));
            }
        };
        let Some(operator) = operator else {
            checks.push(failed(
                "release_profile",
                "发布参数",
                None,
                "平台会话未就绪，发布凭据读取与旧密文迁移已阻止".into(),
                remediation("login_project", "重新登录", None, None),
            ));
            return Ok(report(checks, normalized, None, checked_at));
        };
        let profile = match ReleaseMasterKeyManager::with_local_registry(
            self.state.secret_store.clone(),
            self.state.local_store.pool().clone(),
        )
        .load_profile(
            &ReleaseProfileRepository::new(pools.workbench.clone()),
            project_id,
            "default",
            &connection.db_password,
            &operator,
            application_instance_id(),
        )
        .await
        {
            Ok(value) => {
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
                return Ok(report(checks, normalized, None, checked_at));
            }
        };
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
                return Ok(report(
                    checks,
                    normalized,
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        let leases = ResourceLeaseRepository::new(pools.workbench.clone());
        let host_keys = HostKeyRepository::new(self.state.local_store.pool().clone());
        let mut target_snapshots = Vec::with_capacity(normalized.target_macs.len());
        for mac in &normalized.target_macs {
            let Some(node) = nodes.get(mac) else {
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
                continue;
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
                        "目标正由实例{}执行操作{}",
                        lease.owner_instance_id, lease.operation_id
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
            let host_key = match host_keys.get(project_id, &target).await? {
                Some(host_key) => {
                    checks.push(passed(
                        "host_key",
                        "HostKey",
                        Some(mac),
                        format!("已确认{}", host_key.identity.fingerprint),
                    ));
                    host_key
                }
                None => {
                    checks.push(failed(
                        "host_key",
                        "HostKey",
                        Some(mac),
                        format!("{}:{}尚未确认主机指纹", target.host, target.port),
                        remediation("confirm_host_key", "确认HostKey", None, Some(mac)),
                    ));
                    continue;
                }
            };
            target_snapshots.push(DeploymentTargetSnapshot {
                node: node.clone(),
                ssh_host: target.host.clone(),
                ssh_port: target.port,
                host_key_algorithm: host_key.identity.algorithm.clone(),
                host_key_fingerprint: host_key.identity.fingerprint.clone(),
                host_key_accepted_at: host_key.accepted_at,
            });
            append_remote_runtime_checks(
                &mut checks,
                mac,
                &target,
                &profile,
                host_key.identity,
                &runtime,
                &normalized,
            )
            .await;
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

async fn append_remote_runtime_checks(
    checks: &mut Vec<DeploymentPreflightCheck>,
    mac: &str,
    target: &RemoteTarget,
    profile: &ReleaseProfileRecord,
    host_key: HostKeyIdentity,
    runtime: &ReleaseRuntime,
    plan: &DeploymentPlanInput,
) {
    let auth = match remote_auth(profile) {
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
            return;
        }
    };
    let connection = match RusshConnector::default()
        .connect(target, &auth, HostKeyPolicy::Require(host_key))
        .await
    {
        Ok(connection) => {
            checks.push(passed(
                "ssh_auth",
                "SSH认证",
                Some(mac),
                format!(
                    "{}@{}:{}认证与固定指纹校验通过",
                    auth.username(),
                    target.host,
                    target.port
                ),
            ));
            connection
        }
        Err(error) => {
            checks.push(failed(
                "ssh_auth",
                "SSH认证",
                Some(mac),
                error.to_string(),
                remediation(
                    "confirm_host_key",
                    "检查凭据或重新确认HostKey",
                    None,
                    Some(mac),
                ),
            ));
            return;
        }
    };
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
            "CPU架构",
            "uname",
            vec!["-m".into()],
            Expectation::Exact(runtime.arch.as_str()),
        ),
        (
            "docker",
            "Docker",
            "docker",
            vec!["--version".into()],
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
        match connection
            .run(&request, &cancellation, &NoopRemoteOutputSink)
            .await
        {
            Ok(result) if result.exit_status == 0 => {
                let output = result.stdout.lines().next().unwrap_or("").trim();
                let (satisfied, message) = match expectation {
                    Expectation::Exact(expected) => (
                        output.eq_ignore_ascii_case(expected),
                        format!("要求{expected}，远端已返回受支持值"),
                    ),
                    Expectation::Minimum(requirement) => {
                        match runtime_version_satisfies(output, requirement) {
                            Ok(satisfied) => (satisfied, format!("远端版本满足{requirement}约束")),
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
    if requires_rule_engine_schema_check(plan.mode, plan.service_name.as_deref()) {
        append_rule_engine_schema_check(checks, mac, &connection, &profile.values.aio_data_root)
            .await;
    }
    if let Err(error) = connection.disconnect().await {
        checks.push(warning(
            "ssh_disconnect",
            "SSH会话关闭",
            Some(mac),
            error.to_string(),
        ));
    }
}

fn requires_rule_engine_schema_check(mode: DeploymentMode, service_name: Option<&str>) -> bool {
    mode == DeploymentMode::FullUpgrade
        || (mode == DeploymentMode::ServiceUpgrade && service_name == Some("rule-engine"))
}

async fn append_rule_engine_schema_check(
    checks: &mut Vec<DeploymentPreflightCheck>,
    mac: &str,
    connection: &impl RemoteCommandExecutor,
    data_root: &str,
) {
    let database = format!(
        "{}/rule-engine/db/rules_engine.db",
        data_root.trim_end_matches('/')
    );
    let cancellation = CancellationToken::new();
    let exists = connection
        .run(
            &ExecRequest {
                program: "test".into(),
                args: vec!["-e".into(), database.clone()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(15),
                inactivity_timeout: Duration::from_secs(10),
            },
            &cancellation,
            &NoopRemoteOutputSink,
        )
        .await;
    match exists {
        Ok(result) if result.exit_status == 1 => {
            checks.push(warning(
                "rule_engine_schema",
                "规则引擎数据Schema",
                Some(mac),
                "规则库尚不存在，将由Release首次初始化；本次没有存量迁移对象".into(),
            ));
            return;
        }
        Ok(result) if result.exit_status == 0 => {}
        Ok(result) => {
            checks.push(failed(
                "rule_engine_schema",
                "规则引擎数据Schema",
                Some(mac),
                format!("无法确认规则库是否存在，test退出码{}", result.exit_status),
                remediation(
                    "inspect_rule_engine_schema",
                    "检查规则库文件",
                    None,
                    Some(mac),
                ),
            ));
            return;
        }
        Err(error) => {
            checks.push(failed(
                "rule_engine_schema",
                "规则引擎数据Schema",
                Some(mac),
                error.to_string(),
                remediation(
                    "inspect_rule_engine_schema",
                    "检查规则库文件",
                    None,
                    Some(mac),
                ),
            ));
            return;
        }
    }

    let inspection = connection
        .run(
            &ExecRequest {
                program: "sqlite3".into(),
                args: vec![
                    "-readonly".into(),
                    database,
                    "SELECT COUNT(*) FROM pragma_table_info('rule_definition') WHERE name='record_type';"
                        .into(),
                ],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(15),
                inactivity_timeout: Duration::from_secs(10),
            },
            &cancellation,
            &NoopRemoteOutputSink,
        )
        .await;
    match inspection {
        Ok(result) if result.exit_status == 0 && result.stdout.trim() == "1" => {
            checks.push(passed(
                "rule_engine_schema",
                "规则引擎数据Schema",
                Some(mac),
                "rule_definition.record_type已就绪，可安全重启rule-engine".into(),
            ));
        }
        Ok(result) if result.exit_status == 0 => checks.push(failed(
            "rule_engine_schema",
            "规则引擎数据Schema",
            Some(mac),
            "存量规则库缺少rule_definition.record_type，重启将导致rule-engine失败".into(),
            remediation(
                "migrate_rule_engine_schema",
                "先备份并迁移规则库",
                None,
                Some(mac),
            ),
        )),
        Ok(result) => checks.push(failed(
            "rule_engine_schema",
            "规则引擎数据Schema",
            Some(mac),
            format!("sqlite3只读检查失败，退出码{}", result.exit_status),
            remediation(
                "inspect_rule_engine_schema",
                "检查sqlite3和规则库",
                None,
                Some(mac),
            ),
        )),
        Err(error) => checks.push(failed(
            "rule_engine_schema",
            "规则引擎数据Schema",
            Some(mac),
            error.to_string(),
            remediation(
                "inspect_rule_engine_schema",
                "检查sqlite3和规则库",
                None,
                Some(mac),
            ),
        )),
    }
}

fn remote_auth(profile: &ReleaseProfileRecord) -> AppResult<RemoteAuth> {
    if let Some(private_key) = profile
        .credentials
        .ssh_private_key
        .clone()
        .filter(|value| !value.is_empty())
    {
        Ok(RemoteAuth::PrivateKey {
            username: profile.credentials.ssh_user.clone(),
            private_key: SecretValue::new(private_key),
            passphrase: None,
        })
    } else {
        Ok(RemoteAuth::Password {
            username: profile.credentials.ssh_user.clone(),
            password: SecretValue::new(
                profile
                    .credentials
                    .ssh_password
                    .clone()
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| AppError::InvalidConfig("SSH凭据为空".into()))?,
            ),
        })
    }
}

struct InspectedArtifact {
    normalized: DeploymentPlanInput,
    fingerprint: String,
    runtime: ReleaseRuntime,
}

fn normalize_and_inspect_artifact(
    input: &DeploymentPlanInput,
) -> (AppResult<InspectedArtifact>, DeploymentPreflightCheck) {
    let result = (|| {
        let mut normalized = input.clone();
        normalized.target_macs = input
            .target_macs
            .iter()
            .map(|mac| MacAddress::parse(mac).map(|value| value.normalized().to_string()))
            .collect::<AppResult<Vec<_>>>()?;
        match normalized.mode {
            DeploymentMode::ServiceUpgrade => {
                let archive = inspect_image_archive(Path::new(&normalized.artifact_path))?;
                let fingerprint = sha256_file(Path::new(&normalized.artifact_path))?;
                let tag = normalized
                    .image_name
                    .clone()
                    .filter(|expected| archive.repo_tags.contains(expected))
                    .or_else(|| archive.repo_tags.first().cloned())
                    .ok_or_else(|| AppError::InvalidConfig("单镜像没有可用RepoTag".into()))?;
                let service = normalized
                    .service_name
                    .clone()
                    .unwrap_or_else(|| "service".into());
                normalized.artifact_name = service.clone();
                normalized.artifact_version = tag
                    .rsplit_once(':')
                    .map(|(_, version)| version.to_string())
                    .unwrap_or_else(|| tag.clone());
                normalized.image_name = Some(tag.clone());
                normalized.images = BTreeMap::from([(service, tag)]);
                DeploymentPlan::build(normalized.clone())?;
                Ok(InspectedArtifact {
                    normalized,
                    fingerprint,
                    runtime: ReleaseRuntime {
                        os: "linux".into(),
                        arch: "x86_64".into(),
                        docker: ">=20.10".into(),
                        compose: ">=2.0".into(),
                    },
                })
            }
            DeploymentMode::FirstDeploy | DeploymentMode::FullUpgrade => {
                let validation = inspect_release_directory(Path::new(&normalized.artifact_path))?;
                if !validation.valid {
                    return Err(AppError::InvalidConfig(validation.errors.join("；")));
                }
                let fingerprint = validation.fingerprint.ok_or(AppError::Integrity {
                    operation: "读取Release指纹",
                })?;
                let manifest = validation
                    .manifest
                    .ok_or_else(|| AppError::InvalidConfig("Release缺少manifest".into()))?;
                normalized.artifact_name = "Release".into();
                normalized.artifact_version = manifest.version;
                let runtime = manifest.runtime.clone();
                normalized.images = manifest
                    .images
                    .into_iter()
                    .map(|image| (image.service, image.image))
                    .collect();
                DeploymentPlan::build(normalized.clone())?;
                Ok(InspectedArtifact {
                    normalized,
                    fingerprint,
                    runtime,
                })
            }
        }
    })();
    let check = match &result {
        Ok(inspected) => passed(
            "artifact",
            "发布物",
            None,
            format!(
                "{} {} 校验通过",
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

fn report(
    checks: Vec<DeploymentPreflightCheck>,
    normalized: DeploymentPlanInput,
    profile_version: Option<u64>,
    checked_at: String,
) -> DeploymentPreflightReport {
    DeploymentPreflightReport::from_checks(checks, normalized, profile_version, None, checked_at)
}

fn passed(
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

fn failed(
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

fn remediation(
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
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use super::{append_rule_engine_schema_check, failed, requires_rule_engine_schema_check};
    use crate::application::ports::remote_command::{
        ExecRequest, RemoteCommandExecutor, RemoteCommandResult, RemoteOutputSink,
    };
    use crate::core::error::AppResult;
    use crate::domain::aio::deployment::DeploymentMode;
    use crate::domain::aio::deployment_workflow::{PreflightRemediation, PreflightStatus};
    use tokio_util::sync::CancellationToken;

    struct FakeRemote {
        results: Mutex<VecDeque<RemoteCommandResult>>,
        requests: Mutex<Vec<ExecRequest>>,
    }

    impl FakeRemote {
        fn new(results: impl IntoIterator<Item = RemoteCommandResult>) -> Self {
            Self {
                results: Mutex::new(results.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    impl RemoteCommandExecutor for FakeRemote {
        async fn run(
            &self,
            request: &ExecRequest,
            _cancellation: &CancellationToken,
            _output: &dyn RemoteOutputSink,
        ) -> AppResult<RemoteCommandResult> {
            self.requests
                .lock()
                .expect("requests")
                .push(request.clone());
            Ok(self
                .results
                .lock()
                .expect("results")
                .pop_front()
                .expect("fake remote result"))
        }
    }

    fn result(exit_status: u32, stdout: &str) -> RemoteCommandResult {
        RemoteCommandResult {
            exit_status,
            stdout: stdout.into(),
            stderr: String::new(),
            duration_ms: 1,
        }
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
    fn rule_engine_schema_gate_runs_only_before_rule_engine_restart() {
        assert!(requires_rule_engine_schema_check(
            DeploymentMode::FullUpgrade,
            None
        ));
        assert!(requires_rule_engine_schema_check(
            DeploymentMode::ServiceUpgrade,
            Some("rule-engine")
        ));
        assert!(!requires_rule_engine_schema_check(
            DeploymentMode::ServiceUpgrade,
            Some("device-edge")
        ));
        assert!(!requires_rule_engine_schema_check(
            DeploymentMode::FirstDeploy,
            None
        ));
    }

    #[test]
    fn rule_engine_schema_gate_is_strictly_read_only() {
        let source = include_str!("stage75b_preflight_adapter.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("production preflight source");
        assert!(production.contains("\"-readonly\".into()"));
        assert!(production.contains("pragma_table_info('rule_definition')"));
        assert!(!production.contains("ALTER TABLE"));
    }

    #[tokio::test]
    async fn rule_engine_schema_gate_blocks_missing_column_with_readonly_command() {
        let remote = FakeRemote::new([result(0, ""), result(0, "0\n")]);
        let mut checks = Vec::new();
        append_rule_engine_schema_check(&mut checks, "001122334455", &remote, "/opt/data").await;
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].code, "rule_engine_schema");
        assert_eq!(checks[0].status, PreflightStatus::Failed);
        assert!(checks[0].blocking);
        assert_eq!(
            checks[0]
                .remediation
                .as_ref()
                .map(|item| item.action.as_str()),
            Some("migrate_rule_engine_schema")
        );
        let requests = remote.requests.lock().expect("requests");
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].program, "sqlite3");
        assert_eq!(requests[1].args[0], "-readonly");
        assert_eq!(
            requests[1].args[1],
            "/opt/data/rule-engine/db/rules_engine.db"
        );
    }

    #[tokio::test]
    async fn rule_engine_schema_gate_passes_present_column_and_warns_for_new_database() {
        let present = FakeRemote::new([result(0, ""), result(0, "1\n")]);
        let mut present_checks = Vec::new();
        append_rule_engine_schema_check(
            &mut present_checks,
            "001122334455",
            &present,
            "/opt/data/",
        )
        .await;
        assert_eq!(present_checks[0].status, PreflightStatus::Passed);
        assert!(!present_checks[0].blocking);

        let missing_database = FakeRemote::new([result(1, "")]);
        let mut missing_checks = Vec::new();
        append_rule_engine_schema_check(
            &mut missing_checks,
            "001122334455",
            &missing_database,
            "/opt/data",
        )
        .await;
        assert_eq!(missing_checks[0].status, PreflightStatus::Warning);
        assert!(!missing_checks[0].blocking);
    }
}
