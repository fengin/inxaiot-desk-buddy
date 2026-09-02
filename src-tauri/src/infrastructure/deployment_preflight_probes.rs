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

// 沿用旧版Agent的默认服务端口；升级允许现有服务占用，不能把自身容器误报为冲突。
const PORTS_SCRIPT: &str = r#"set -eu
[ "$1" = first ] || exit 0
command -v ss >/dev/null 2>&1 || exit 23
listeners=$(ss -lntH) || exit 23
for port in 1883 6001 6002 7000; do
  if printf '%s\n' "$listeners" | awk '{print $4}' | grep -Eq "[:.]${port}$"; then
    printf '%s' "$port"
    exit 20
  fi
done
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
) -> Vec<DeploymentPreflightCheck> {
    let mut checks = Vec::new();
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
            vec![
                if mode == DeploymentMode::FirstDeploy {
                    "first"
                } else {
                    "upgrade"
                }
                .into(),
            ],
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
                    if ["1883", "6001", "6002", "7000"].contains(&port) {
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
            platform_host: "192.0.2.1".into(),
            platform_api_port: 8055,
            platform_mqtt_host: "192.0.2.2".into(),
            platform_mqtt_port: 1883,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/inxaiot".into(),
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
        )
        .await;
        assert!(checks.iter().all(|check| check.blocking));
        assert!(checks[0].message.contains("空间不足"));
        assert_eq!(connection.requests.lock().unwrap()[1].args[2], "first");
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
        )
        .await;
        assert!(
            checks
                .iter()
                .all(|check| check.status == PreflightStatus::Passed)
        );
        assert_eq!(connection.requests.lock().unwrap()[1].args[2], "upgrade");
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
    }
}
