#[path = "../src/core/mod.rs"]
mod core;
#[path = "../src/infrastructure/mod.rs"]
mod infrastructure;

use std::path::PathBuf;
use std::time::Duration;

use core::secret::SecretValue;
use infrastructure::database::{DualMySqlPools, MySqlProjectConfig};
use infrastructure::platform_auth::{PlatformAuthClient, PlatformLoginConfig};
use infrastructure::remote::{HostKeyPolicy, RemoteSession, SshConnectionConfig};
use tokio_util::sync::CancellationToken;

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing required environment variable {name}"))
}

fn database_config() -> MySqlProjectConfig {
    MySqlProjectConfig {
        host: required_env("INX_POC_MYSQL_HOST"),
        port: std::env::var("INX_POC_MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(3306),
        username: required_env("INX_POC_MYSQL_USER"),
        password: SecretValue::new(required_env("INX_POC_MYSQL_PASSWORD")),
        platform_schema: required_env("INX_POC_PLATFORM_DB"),
        workbench_schema: required_env("INX_POC_WORKBENCH_DB"),
        connect_timeout: Duration::from_secs(10),
    }
}

fn ssh_config(host: String) -> SshConnectionConfig {
    SshConnectionConfig {
        host,
        port: 22,
        username: required_env("INX_POC_SSH_USER"),
        private_key_path: PathBuf::from(required_env("INX_POC_SSH_KEY")),
        private_key_password: std::env::var("INX_POC_SSH_KEY_PASSWORD").ok(),
        connect_timeout: Duration::from_secs(10),
        inactivity_timeout: Duration::from_secs(20),
    }
}

#[tokio::test]
#[ignore = "requires the authorized project MySQL test environment"]
async fn dual_mysql_pool_and_platform_schema_probe() {
    let config = database_config();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect dual mysql pools");
    let report = pools.probe(&config).await.expect("probe mysql schemas");
    assert!(report.platform_aio_table_exists);
    assert!(
        report.missing_required_columns.is_empty(),
        "missing platform columns: {:?}",
        report.missing_required_columns
    );
    assert!(report.workbench_charset.is_some());
    pools.close().await;
}

#[tokio::test]
#[ignore = "requires the authorized project platform login environment"]
async fn platform_rsa_login_returns_a_redacted_session() {
    let config = PlatformLoginConfig {
        base_url: required_env("INX_POC_PLATFORM_URL"),
        principal: required_env("INX_POC_PLATFORM_PRINCIPAL"),
        credentials: SecretValue::new(required_env("INX_POC_PLATFORM_CREDENTIALS")),
        session_uuid: SecretValue::new(required_env("INX_POC_PLATFORM_SESSION_UUID")),
        image_code: SecretValue::new(required_env("INX_POC_PLATFORM_IMAGE_CODE")),
        timeout: Duration::from_secs(10),
    };
    let client = PlatformAuthClient::new(config.timeout).expect("create platform auth client");
    let session = client.login(&config).await.expect("platform rsa login");
    assert!(!session.access_token.is_empty());
    assert!(!format!("{session:?}").contains(session.access_token.expose()));
}

#[tokio::test]
#[ignore = "requires the authorized Linux edge nodes"]
async fn ssh_host_key_pinning_and_sftp_atomic_upload_work_on_both_nodes() {
    let hosts = required_env("INX_POC_SSH_HOSTS")
        .split(',')
        .map(str::trim)
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert!(hosts.len() >= 2, "two SSH nodes are required");

    let local_file = std::env::temp_dir().join("inxaiot-desk-buddy-sftp-poc.bin");
    tokio::fs::write(&local_file, vec![0x5a; 2 * 1024 * 1024])
        .await
        .expect("create local sftp fixture");

    for host in hosts {
        let config = ssh_config(host);
        let discovery = RemoteSession::connect_private_key(&config, HostKeyPolicy::Capture)
            .await
            .expect("capture ssh host key");
        let fingerprint = discovery.server_fingerprint().to_string();
        discovery.disconnect().await.expect("disconnect discovery session");

        let session = RemoteSession::connect_private_key(
            &config,
            HostKeyPolicy::RequireFingerprint(fingerprint),
        )
        .await
        .expect("connect with pinned ssh host key");
        let remote_path = "/opt/data/.inxaiot-desk-buddy-sftp-poc.bin";
        let mut last_progress = 0;
        session
            .upload_atomic(
                &local_file,
                remote_path,
                &CancellationToken::new(),
                |transferred, total| {
                    assert!(transferred >= last_progress);
                    assert!(transferred <= total);
                    last_progress = transferred;
                },
            )
            .await
            .expect("sftp atomic upload");
        assert_eq!(last_progress, 2 * 1024 * 1024);
        let result = session
            .execute(&format!("wc -c < {remote_path}; rm -f {remote_path}"))
            .await
            .expect("verify and remove remote fixture");
        assert_eq!(result.exit_status, 0, "{}", result.stderr);
        assert_eq!(result.stdout.trim(), (2 * 1024 * 1024).to_string());
        session.disconnect().await.expect("disconnect ssh session");
    }
    tokio::fs::remove_file(local_file)
        .await
        .expect("remove local sftp fixture");
}

