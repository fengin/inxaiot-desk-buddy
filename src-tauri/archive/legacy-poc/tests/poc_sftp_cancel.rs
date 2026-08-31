use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::infrastructure::remote::{
    HostKeyPolicy, RemoteSession, SshConnectionConfig,
};
use tokio_util::sync::CancellationToken;

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

#[tokio::test]
#[ignore = "requires an authorized Linux edge node"]
async fn cancelled_sftp_upload_removes_remote_temporary_file() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let host = line(&description, "一体机IP:")
        .split('/')
        .next()
        .expect("first host")
        .trim();
    let config = SshConnectionConfig {
        host: host.to_string(),
        port: 22,
        username: line(&description, "一体机ssh用户：").to_string(),
        private_key_path: project.join("test/id_rsa"),
        private_key_password: None,
        connect_timeout: Duration::from_secs(10),
        inactivity_timeout: Duration::from_secs(30),
    };
    let discovery = RemoteSession::connect_private_key(&config, HostKeyPolicy::Capture)
        .await
        .expect("capture host key");
    let fingerprint = discovery.server_fingerprint().to_string();
    discovery.disconnect().await.expect("disconnect discovery");
    let session =
        RemoteSession::connect_private_key(&config, HostKeyPolicy::RequireFingerprint(fingerprint))
            .await
            .expect("connect pinned");

    let local = std::env::temp_dir().join("inxaiot-desk-buddy-sftp-cancel.bin");
    tokio::fs::write(&local, vec![0x33; 1024 * 1024])
        .await
        .expect("local fixture");
    let token = CancellationToken::new();
    token.cancel();
    let remote_path = "/opt/data/.inxaiot-desk-buddy-sftp-cancel.bin";
    let error = session
        .upload_atomic(&local, remote_path, &token, |_, _| {})
        .await
        .expect_err("upload must be cancelled");
    assert!(matches!(error, AppError::Cancelled));
    let verify = session
        .execute(&format!(
            "test ! -e {remote_path} && test ! -e {remote_path}.part"
        ))
        .await
        .expect("verify remote cleanup");
    assert_eq!(verify.exit_status, 0, "{}", verify.stderr);
    session.disconnect().await.expect("disconnect");
    tokio::fs::remove_file(local).await.expect("remove fixture");
}
