//! 部署前只读探测；不上传Agent、不创建目录、不清理数据。
use std::collections::BTreeMap;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use super::stage75b_preflight_adapter::{failed, passed, remediation};
use crate::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use crate::domain::aio::deployment::DeploymentMode;
use crate::domain::aio::deployment_workflow::DeploymentPreflightCheck;
use crate::formal::release_profile_repository::ReleaseProfileValues;

pub(super) async fn local_endpoint_checks(
    values: &ReleaseProfileValues,
) -> DeploymentPreflightCheck {
    let (api, mqtt) = tokio::join!(
        tcp_reachable(&values.platform_host, values.platform_api_port),
        tcp_reachable(&values.platform_mqtt_host, values.platform_mqtt_port)
    );
    let endpoints = [
        (
            "平台 API",
            &values.platform_host,
            values.platform_api_port,
            api,
        ),
        (
            "平台 MQTT",
            &values.platform_mqtt_host,
            values.platform_mqtt_port,
            mqtt,
        ),
    ];
    let unavailable = endpoints
        .iter()
        .filter(|(_, _, _, reachable)| !reachable)
        .map(|(label, host, port, _)| format!("{label} {host}:{port} 无法连接"))
        .collect::<Vec<_>>();
    if unavailable.is_empty() {
        passed(
            "release_endpoints",
            "发布参数",
            None,
            format!(
                "平台 API {}:{}、MQTT {}:{} 端口连通（从本机检查）",
                values.platform_host,
                values.platform_api_port,
                values.platform_mqtt_host,
                values.platform_mqtt_port
            ),
        )
    } else {
        failed(
            "release_endpoints",
            "发布参数",
            None,
            unavailable.join("；"),
            remediation(
                "open_release_profile",
                "检查发布参数和网络",
                Some("/aio/release"),
                None,
            ),
        )
    }
}

async fn tcp_reachable(host: &str, port: u16) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(5), TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

// 使用固定脚本+独立位置参数，不把配置插入shell源码；与既有Agent的端口探测口径一致。
const ENDPOINT_SCRIPT: &str = r#"set -eu
probe() {
  if command -v nc >/dev/null 2>&1; then
    nc -z -w 5 -- "$1" "$2" >/dev/null 2>&1
  elif command -v bash >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    timeout 5 bash -c 'exec 3<>"/dev/tcp/$1/$2"' _ "$1" "$2" >/dev/null 2>&1
  else
    exit 23
  fi
}
probe "$1" "$2" || exit 21
probe "$3" "$4" || exit 22
"#;

// 不存在的目录只检查最近存在的父目录，实际创建仍由执行阶段的Agent负责。
const STORAGE_SCRIPT: &str = r#"set -eu
for root in "$1" "$2"; do
  case "$root" in /*) ;; *) exit 14 ;; esac
  [ "$root" != / ] || exit 14
  current="$root"
  while [ ! -e "$current" ]; do
    [ ! -L "$current" ] || exit 14
    current=$(dirname -- "$current")
  done
  [ -d "$current" ] && [ -w "$current" ] && [ -x "$current" ] || exit 14
  free=$(df -Pm -- "$current" | awk 'NR==2 {print $4}')
  case "$free" in ''|*[!0-9]*) exit 17 ;; esac
  [ "$free" -ge 1024 ] || exit 16
done
"#;

// 端口来自已经校验并渲染的项目Compose；升级允许现有服务占用，不能把自身容器误报为冲突。
const PORTS_SCRIPT: &str = r#"set -eu
[ "$1" = first ] || exit 0
shift
command -v ss >/dev/null 2>&1 || exit 23
listeners=$(ss -lntH) || exit 23
for port in "$@"; do
  if printf '%s\n' "$listeners" | awk '{print $4}' | grep -Eq "[:.]${port}$"; then
    printf '%s' "$port"
    exit 20
  fi
done
"#;

// 升级必须从已经落盘的current版本开始；路径和部署模式始终通过独立位置参数传入。
// 校验口径分别对齐Agent的backup（整包升级）和service-check（单服升级）前置要求。
const CURRENT_RELEASE_SCRIPT: &str = r#"set -eu
case "$3" in
  first_deploy) exit 0 ;;
  full_upgrade|service_upgrade) ;;
  *) exit 29 ;;
esac
current_path="$1/current"
if [ ! -L "$current_path" ] && [ ! -d "$current_path" ]; then
  exit 24
fi
current_dir=$(readlink -f "$current_path" 2>/dev/null || true)
[ -n "$current_dir" ] && [ -d "$current_dir" ] || exit 24
[ -f "$current_dir/docker-compose.yml" ] || exit 25
[ -f "$current_dir/.env" ] || exit 26
if [ "$3" = full_upgrade ]; then
  [ -f "$current_dir/manifest.json" ] || exit 27
  [ -f "$2/config/host-info.json" ] || exit 28
fi
"#;

fn script_request(script: &str, args: Vec<String>) -> ExecRequest {
    ExecRequest {
        program: "sh".into(),
        args: [vec!["-s".into(), "--".into()], args].concat(),
        env: BTreeMap::new(),
        stdin: Some(script.as_bytes().to_vec()),
        total_timeout: Duration::from_secs(20),
        inactivity_timeout: Duration::from_secs(15),
    }
}

pub(super) async fn remote_endpoint_check(
    connection: &impl RemoteCommandExecutor,
    values: &ReleaseProfileValues,
    mac: &str,
) -> DeploymentPreflightCheck {
    let request = script_request(
        ENDPOINT_SCRIPT,
        vec![
            values.platform_host.clone(),
            values.platform_api_port.to_string(),
            values.platform_mqtt_host.clone(),
            values.platform_mqtt_port.to_string(),
        ],
    );
    let result = connection
        .run(&request, &CancellationToken::new(), &NoopRemoteOutputSink)
        .await;
    let message = match result {
        Ok(result) if result.exit_status == 0 => {
            return passed(
                "platform_endpoints",
                "连通性",
                Some(mac),
                "一体机到平台 API、MQTT 连通".into(),
            );
        }
        Ok(result) => match result.exit_status {
            21 => format!(
                "一体机无法连接平台 API {}:{}，请检查地址、端口和网络",
                values.platform_host, values.platform_api_port
            ),
            22 => format!(
                "一体机无法连接平台 MQTT {}:{}，请检查地址、端口和网络",
                values.platform_mqtt_host, values.platform_mqtt_port
            ),
            23 => "一体机缺少连通性检查工具（nc 或 bash/timeout）".into(),
            _ => "一体机到平台的连通性检查未完成，请检查远端环境".into(),
        },
        Err(error) => error.to_string(),
    };
    failed(
        "platform_endpoints",
        "连通性",
        Some(mac),
        message,
        remediation(
            "open_release_profile",
            "检查发布参数和网络",
            Some("/aio/release"),
            Some(mac),
        ),
    )
}

pub(super) async fn remote_environment_checks(
    connection: &impl RemoteCommandExecutor,
    values: &ReleaseProfileValues,
    mac: &str,
    mode: DeploymentMode,
    published_ports: &[u16],
) -> Vec<DeploymentPreflightCheck> {
    let mut checks = Vec::new();
    if mode != DeploymentMode::FirstDeploy {
        let mode_argument = match mode {
            DeploymentMode::FullUpgrade => "full_upgrade",
            DeploymentMode::ServiceUpgrade => "service_upgrade",
            DeploymentMode::FirstDeploy => unreachable!("首次部署不执行当前版本检查"),
        };
        let result = connection
            .run(
                &script_request(
                    CURRENT_RELEASE_SCRIPT,
                    vec![
                        values.aio_deploy_root.clone(),
                        values.aio_data_root.clone(),
                        mode_argument.into(),
                    ],
                ),
                &CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await;
        let message = match result {
            Ok(result) if result.exit_status == 0 => {
                checks.push(passed(
                    "remote_current_release",
                    "当前版本",
                    Some(mac),
                    "当前版本满足升级前置条件".into(),
                ));
                None
            }
            Ok(result) => Some(match result.exit_status {
                24 => "当前一体机尚未完成首次部署，请选择首次部署".into(),
                25 => "当前版本缺少docker-compose.yml，无法执行升级，请先恢复当前版本".into(),
                26 => "当前版本缺少.env，无法执行升级，请先恢复当前版本".into(),
                27 => "当前版本缺少manifest.json，无法执行整包升级，请先恢复当前版本".into(),
                28 => {
                    "当前一体机缺少config/host-info.json，无法执行整包升级，请先恢复当前配置".into()
                }
                29 => "升级模式无效，未执行当前版本检查".into(),
                _ => "当前版本检查未完成，请检查一体机部署目录".into(),
            }),
            Err(error) => Some(error.to_string()),
        };
        if let Some(message) = message {
            checks.push(failed(
                "remote_current_release",
                "当前版本",
                Some(mac),
                message,
                remediation(
                    "repair_remote_runtime",
                    "检查一体机当前版本",
                    None,
                    Some(mac),
                ),
            ));
        }
    }
    for (code, script, args, summary) in [
        (
            "remote_storage",
            STORAGE_SCRIPT,
            vec![values.aio_data_root.clone(), values.aio_deploy_root.clone()],
            "数据/部署目录可写，剩余空间不少于1GiB",
        ),
        (
            "remote_ports",
            PORTS_SCRIPT,
            [
                vec![
                    if mode == DeploymentMode::FirstDeploy {
                        "first"
                    } else {
                        "upgrade"
                    }
                    .into(),
                ],
                published_ports.iter().map(u16::to_string).collect(),
            ]
            .concat(),
            if mode == DeploymentMode::FirstDeploy {
                "服务端口未被占用"
            } else {
                "升级模式保留现有服务端口"
            },
        ),
    ] {
        let result = connection
            .run(
                &script_request(script, args),
                &CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await;
        let message = match result {
            Ok(result) if result.exit_status == 0 => {
                checks.push(passed(code, "环境准备", Some(mac), summary.into()));
                continue;
            }
            Ok(result) => match result.exit_status {
                14 => "数据或部署目录不可写/路径不可用，请检查目录权限".into(),
                16 => "数据或部署目录所在磁盘剩余空间不足1GiB，请先释放空间".into(),
                20 => {
                    let port = result.stdout.trim();
                    if port
                        .parse::<u16>()
                        .is_ok_and(|port| published_ports.contains(&port))
                    {
                        format!("首次部署所需端口{port}已被占用，请确认旧服务或其他程序")
                    } else {
                        "首次部署所需服务端口已被占用".into()
                    }
                }
                23 => "无法检查服务端口，请确认一体机已提供ss命令".into(),
                _ => "目录、空间或端口检查未完成，请检查一体机环境".into(),
            },
            Err(error) => error.to_string(),
        };
        checks.push(failed(
            code,
            "环境准备",
            Some(mac),
            message,
            remediation("repair_remote_runtime", "检查一体机环境", None, Some(mac)),
        ));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::remote_command::{RemoteCommandResult, RemoteOutputSink};
    use crate::core::error::AppResult;
    use crate::domain::aio::deployment_workflow::PreflightStatus;
    use std::sync::Mutex;

    struct ProbeConnection {
        exit_status: u32,
        stdout: String,
        requests: Mutex<Vec<ExecRequest>>,
    }

    struct CurrentReleaseProbeConnection {
        current_exit_status: u32,
        requests: Mutex<Vec<ExecRequest>>,
    }

    impl RemoteCommandExecutor for CurrentReleaseProbeConnection {
        async fn run(
            &self,
            request: &ExecRequest,
            _: &CancellationToken,
            _: &dyn RemoteOutputSink,
        ) -> AppResult<RemoteCommandResult> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(RemoteCommandResult {
                exit_status: if request.stdin.as_deref() == Some(CURRENT_RELEASE_SCRIPT.as_bytes())
                {
                    self.current_exit_status
                } else {
                    0
                },
                stdout: String::new(),
                stderr: "不应回显的远端输出".into(),
                duration_ms: 1,
            })
        }
    }

    impl RemoteCommandExecutor for ProbeConnection {
        async fn run(
            &self,
            request: &ExecRequest,
            _: &CancellationToken,
            _: &dyn RemoteOutputSink,
        ) -> AppResult<RemoteCommandResult> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(RemoteCommandResult {
                exit_status: self.exit_status,
                stdout: self.stdout.clone(),
                stderr: "不应回显的远端输出".into(),
                duration_ms: 1,
            })
        }
    }

    fn values() -> ReleaseProfileValues {
        ReleaseProfileValues {
            env_template: String::new(),
            compose_template: String::new(),
            host_info_template: String::new(),
            platform_host: "192.0.2.1".into(),
            platform_api_port: 8055,
            platform_mqtt_host: "192.0.2.2".into(),
            platform_mqtt_port: 1883,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy".into(),
        }
    }

    #[tokio::test]
    async fn node_connectivity_failure_identifies_endpoint_without_echoing_remote_output() {
        let connection = ProbeConnection {
            exit_status: 22,
            stdout: "不应回显".into(),
            requests: Mutex::new(vec![]),
        };
        let result = remote_endpoint_check(&connection, &values(), "001122334455").await;
        assert_eq!(result.status, PreflightStatus::Failed);
        assert!(result.blocking);
        assert!(result.message.contains("MQTT 192.0.2.2:1883"));
        assert!(!result.message.contains("不应回显"));
        assert_eq!(
            connection.requests.lock().unwrap()[0].args,
            ["-s", "--", "192.0.2.1", "8055", "192.0.2.2", "1883"]
        );
    }

    #[tokio::test]
    async fn environment_failure_is_blocking_and_upgrade_does_not_treat_existing_ports_as_conflict()
    {
        let connection = ProbeConnection {
            exit_status: 16,
            stdout: String::new(),
            requests: Mutex::new(vec![]),
        };
        let checks = remote_environment_checks(
            &connection,
            &values(),
            "001122334455",
            DeploymentMode::FirstDeploy,
            &[1883, 6002],
        )
        .await;
        assert!(checks.iter().all(|check| check.blocking));
        assert!(checks[0].message.contains("空间不足"));
        assert_eq!(
            connection
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|request| request.stdin.as_deref() == Some(PORTS_SCRIPT.as_bytes()))
                .expect("必须执行端口检查")
                .args[2],
            "first"
        );
        let connection = ProbeConnection {
            exit_status: 0,
            stdout: String::new(),
            requests: Mutex::new(vec![]),
        };
        let checks = remote_environment_checks(
            &connection,
            &values(),
            "001122334455",
            DeploymentMode::FullUpgrade,
            &[1883, 6002],
        )
        .await;
        assert!(
            checks
                .iter()
                .all(|check| check.status == PreflightStatus::Passed)
        );
        assert_eq!(
            connection
                .requests
                .lock()
                .unwrap()
                .iter()
                .find(|request| request.stdin.as_deref() == Some(PORTS_SCRIPT.as_bytes()))
                .expect("必须执行端口检查")
                .args[2],
            "upgrade"
        );
    }

    #[tokio::test]
    async fn upgrade_requires_a_valid_current_release_but_first_deploy_does_not_probe_it() {
        let first = CurrentReleaseProbeConnection {
            current_exit_status: 24,
            requests: Mutex::new(vec![]),
        };
        let first_checks = remote_environment_checks(
            &first,
            &values(),
            "001122334455",
            DeploymentMode::FirstDeploy,
            &[1883, 6002],
        )
        .await;
        assert!(
            first_checks
                .iter()
                .all(|check| check.code != "remote_current_release")
        );
        assert!(first.requests.lock().unwrap().iter().all(|request| {
            request.stdin.as_deref() != Some(CURRENT_RELEASE_SCRIPT.as_bytes())
        }));

        for (mode, mode_argument) in [
            (DeploymentMode::FullUpgrade, "full_upgrade"),
            (DeploymentMode::ServiceUpgrade, "service_upgrade"),
        ] {
            let upgrade = CurrentReleaseProbeConnection {
                current_exit_status: 24,
                requests: Mutex::new(vec![]),
            };
            let checks =
                remote_environment_checks(&upgrade, &values(), "001122334455", mode, &[1883, 6002])
                    .await;
            let current = checks
                .iter()
                .find(|check| check.code == "remote_current_release")
                .expect("升级必须产生当前版本检查");
            assert_eq!(current.status, PreflightStatus::Failed);
            assert!(current.blocking);
            assert_eq!(
                current.message,
                "当前一体机尚未完成首次部署，请选择首次部署"
            );
            let requests = upgrade.requests.lock().unwrap();
            let request = requests
                .iter()
                .find(|request| request.stdin.as_deref() == Some(CURRENT_RELEASE_SCRIPT.as_bytes()))
                .expect("升级必须执行固定当前版本脚本");
            assert_eq!(
                request.args,
                ["-s", "--", "/opt/data/deploy", "/opt/data", mode_argument]
            );
        }
    }

    #[tokio::test]
    async fn upgrade_current_release_file_failures_are_specific_and_do_not_echo_remote_output() {
        for (exit_status, expected) in [
            (25, "docker-compose.yml"),
            (26, ".env"),
            (27, "manifest.json"),
            (28, "config/host-info.json"),
        ] {
            let connection = CurrentReleaseProbeConnection {
                current_exit_status: exit_status,
                requests: Mutex::new(vec![]),
            };
            let checks = remote_environment_checks(
                &connection,
                &values(),
                "001122334455",
                DeploymentMode::FullUpgrade,
                &[1883, 6002],
            )
            .await;
            let current = checks
                .iter()
                .find(|check| check.code == "remote_current_release")
                .expect("整包升级必须产生当前版本检查");
            assert_eq!(current.status, PreflightStatus::Failed);
            assert!(current.message.contains(expected));
            assert!(!current.message.contains("不应回显"));
        }
    }

    #[tokio::test]
    async fn tcp_probe_reports_open_and_closed_ports_without_credentials() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(tcp_reachable("127.0.0.1", port).await);
        drop(listener);
        assert!(!tcp_reachable("127.0.0.1", port).await);
    }

    #[test]
    fn configured_values_are_arguments_not_shell_source() {
        let value = "/opt/a path/'quoted';echo unwanted";
        let request = script_request(STORAGE_SCRIPT, vec![value.into(), "/opt/data".into()]);
        assert_eq!(request.args[2], value);
        assert_eq!(request.stdin.unwrap(), STORAGE_SCRIPT.as_bytes());
        assert!(!STORAGE_SCRIPT.contains("mkdir"));
        assert!(!STORAGE_SCRIPT.contains("rm "));
        assert!(PORTS_SCRIPT.contains("[ \"$1\" = first ] || exit 0"));
        assert!(CURRENT_RELEASE_SCRIPT.contains("current_path=\"$1/current\""));
        assert!(CURRENT_RELEASE_SCRIPT.contains("[ -f \"$2/config/host-info.json\" ]"));
        assert!(!CURRENT_RELEASE_SCRIPT.contains(&values().aio_deploy_root));
    }
}
