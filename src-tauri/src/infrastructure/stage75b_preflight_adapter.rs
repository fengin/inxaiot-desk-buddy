use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use time::OffsetDateTime;
use tokio_util::sync::CancellationToken;

use crate::application::ports::deployment_workflow::DeploymentPreflightPort;
use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use crate::application::ports::remote_session::{RemoteAuth, RemoteConnection, RemoteTarget};
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
use crate::formal::release_master_key::ReleaseMasterKeyManager;
use crate::formal::release_profile_repository::{ReleaseProfileRecord, ReleaseProfileRepository};
use crate::formal::resource_lease_repository::ResourceLeaseRepository;
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::deployment_preflight_probes::{
    local_endpoint_checks, remote_endpoint_check, remote_environment_checks,
};
use crate::infrastructure::local_sqlite::host_key_repository::{HostKeyRecord, HostKeyRepository};
use crate::infrastructure::project_context::{map_formal_error, project_database};
use crate::infrastructure::remote::RusshConnector;
use crate::infrastructure::remote::observed::ObservedConnector;
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
        let profile = match ReleaseMasterKeyManager::with_local_registry(
            self.state.secret_store.clone(),
            self.state.local_store.pool().clone(),
        )
        .load_profile(
            &ReleaseProfileRepository::new(pools.workbench.clone()),
            project_id,
            "default",
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
                return Ok(report(
                    checks,
                    normalized,
                    Some(profile.version),
                    checked_at,
                ));
            }
        };
        let leases = ResourceLeaseRepository::new(pools.workbench.clone());
        let connector = ObservedConnector::new(
            RusshConnector::default(),
            HostKeyRepository::new(self.state.local_store.pool().clone()),
            project_id,
        );
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
    connector: &ObservedConnector<RusshConnector>,
    runtime: &ReleaseRuntime,
    mode: DeploymentMode,
) -> Option<HostKeyRecord> {
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
    checks.extend(remote_environment_checks(&connection, &profile.values, mac, mode).await);
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
    use super::{failed, warning};
    use crate::domain::aio::deployment_workflow::{PreflightRemediation, PreflightStatus};

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
}
