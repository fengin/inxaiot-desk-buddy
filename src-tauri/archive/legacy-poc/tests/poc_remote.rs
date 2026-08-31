use std::path::{Path, PathBuf};
use std::time::Duration;

use inxaiot_desk_buddy_lib::infrastructure::remote::{
    HostKeyPolicy, RemoteSession, SshConnectionConfig, shell_quote,
};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

struct RemoteTestConfig {
    hosts: Vec<String>,
    user: String,
    key: PathBuf,
    platform_host: String,
    platform_api_port: String,
    platform_mqtt_port: String,
    agent: PathBuf,
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .to_path_buf()
}

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn split_host_port(value: &str) -> (&str, &str) {
    value.rsplit_once(':').expect("host:port")
}

fn config() -> RemoteTestConfig {
    let root = project_root();
    let description =
        std::fs::read_to_string(root.join("test/测试数据说明.txt")).expect("test data");
    let (platform_host, api_port) = split_host_port(line(&description, "平台API："));
    let (_, mqtt_port) = split_host_port(line(&description, "平台mqtt："));
    let workspace = root.parent().and_then(Path::parent).expect("workspace");
    RemoteTestConfig {
        hosts: line(&description, "一体机IP:")
            .split('/')
            .map(str::trim)
            .map(str::to_string)
            .collect(),
        user: line(&description, "一体机ssh用户：").to_string(),
        key: root.join("test/id_rsa"),
        platform_host: platform_host.to_string(),
        platform_api_port: api_port.to_string(),
        platform_mqtt_port: mqtt_port.to_string(),
        agent: workspace
            .join("inxvision-assistance/inxaiot-edge-workbench/agent/edge-node-agent.sh"),
    }
}

fn ssh(config: &RemoteTestConfig, host: &str) -> SshConnectionConfig {
    SshConnectionConfig {
        host: host.to_string(),
        port: 22,
        username: config.user.clone(),
        private_key_path: config.key.clone(),
        private_key_password: None,
        connect_timeout: Duration::from_secs(10),
        inactivity_timeout: Duration::from_secs(30),
    }
}

async fn connect_pinned(config: &SshConnectionConfig) -> RemoteSession {
    let discovery = RemoteSession::connect_private_key(config, HostKeyPolicy::Capture)
        .await
        .expect("capture host key");
    let fingerprint = discovery.server_fingerprint().to_string();
    assert!(fingerprint.starts_with("SHA256:"));
    discovery.disconnect().await.expect("disconnect discovery");
    RemoteSession::connect_private_key(config, HostKeyPolicy::RequireFingerprint(fingerprint))
        .await
        .expect("connect with pinned host key")
}

#[test]
fn shell_arguments_are_quoted_for_agent_environment() {
    assert_eq!(shell_quote("two words"), "'two words'");
    assert_eq!(shell_quote("a'b"), "'a'\\''b'");
}

#[tokio::test]
#[ignore = "requires both authorized Linux edge nodes"]
async fn private_key_host_pinning_and_atomic_sftp_work_on_both_nodes() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let local = std::env::temp_dir().join("inxaiot-desk-buddy-sftp-poc.bin");
    tokio::fs::write(&local, vec![0x5a; 2 * 1024 * 1024])
        .await
        .expect("local fixture");
    for host in &config.hosts {
        let session = connect_pinned(&ssh(&config, host)).await;
        let architecture = session.execute("uname -m").await.expect("uname");
        assert_eq!(architecture.exit_status, 0, "{}", architecture.stderr);
        assert_eq!(architecture.stdout.trim(), "x86_64");

        let remote_path = "/opt/data/.inxaiot-desk-buddy-sftp-poc.bin";
        let mut last = 0_u64;
        session
            .upload_atomic(
                &local,
                remote_path,
                &CancellationToken::new(),
                |transferred, total| {
                    assert!(transferred >= last);
                    assert!(transferred <= total);
                    last = transferred;
                },
            )
            .await
            .expect("atomic sftp upload");
        assert_eq!(last, 2 * 1024 * 1024);
        let verify = session
            .execute(&format!("wc -c < {remote_path}; rm -f {remote_path}"))
            .await
            .expect("verify upload");
        assert_eq!(verify.exit_status, 0, "{}", verify.stderr);
        assert_eq!(verify.stdout.trim(), (2 * 1024 * 1024).to_string());
        session.disconnect().await.expect("disconnect");
    }
    tokio::fs::remove_file(local).await.expect("remove fixture");
}

#[tokio::test]
#[ignore = "requires both authorized Linux edge nodes"]
async fn existing_agent_version_precheck_and_service_check_are_compatible() {
    let config = config();
    for host in &config.hosts {
        let session = connect_pinned(&ssh(&config, host)).await;
        let remote_agent = "/opt/data/.inxaiot-desk-buddy-poc-agent.sh";
        session
            .upload_atomic(
                &config.agent,
                remote_agent,
                &CancellationToken::new(),
                |_, _| {},
            )
            .await
            .expect("upload agent");
        let version = session
            .execute(&format!(
                "chmod 700 {remote_agent}; sh {remote_agent} version"
            ))
            .await
            .expect("agent version");
        assert_eq!(version.exit_status, 0, "{}", version.stderr);
        let version_json: Value =
            serde_json::from_str(version.stdout.trim()).expect("agent version json");
        assert_eq!(version_json["protocolVersion"].as_u64(), Some(1));

        let precheck = format!(
            "ALLOW_EXISTING_PORTS='true' MIN_FREE_MB='1' PLATFORM_API_HOST={} PLATFORM_API_PORT={} PLATFORM_MQTT_HOST={} PLATFORM_MQTT_PORT={} sh {remote_agent} precheck",
            shell_quote(&config.platform_host),
            shell_quote(&config.platform_api_port),
            shell_quote(&config.platform_host),
            shell_quote(&config.platform_mqtt_port),
        );
        let precheck = session.execute(&precheck).await.expect("agent precheck");
        assert_eq!(
            precheck.exit_status, 0,
            "{}\n{}",
            precheck.stdout, precheck.stderr
        );
        assert!(
            precheck
                .stdout
                .lines()
                .any(|line| line.contains("\"status\":\"success\""))
        );

        let service_check = session
            .execute(&format!(
                "SERVICE_NAME='device-edge' sh {remote_agent} service-check; rm -f {remote_agent}"
            ))
            .await
            .expect("agent service check");
        assert_eq!(
            service_check.exit_status, 0,
            "{}\n{}",
            service_check.stdout, service_check.stderr
        );
        assert!(service_check.stdout.contains("\"step\":\"service_check\""));
        session.disconnect().await.expect("disconnect");
    }
}
