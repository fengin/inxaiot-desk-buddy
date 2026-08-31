mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::core::error::AppError;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires an authorized Linux edge node"]
async fn nonzero_exit_command_cancel_timeout_and_upload_cancel_are_distinct() {
    let config = config();
    let session = connect_pinned(&config, &config.hosts[0]).await;
    let nonzero = session
        .run(
            &ExecRequest {
                program: "sh".into(),
                args: vec!["-lc".into(), "printf phase4-error >&2; exit 7".into()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(10),
                inactivity_timeout: Duration::from_secs(5),
            },
            &CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await
        .expect("nonzero command result");
    assert_eq!(nonzero.exit_status, 7);
    assert!(nonzero.stderr.contains("phase4-error"));

    let timeout = session
        .run(
            &ExecRequest {
                program: "sleep".into(),
                args: vec!["2".into()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(5),
                inactivity_timeout: Duration::from_millis(200),
            },
            &CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await;
    assert!(matches!(timeout, Err(AppError::Timeout { .. })));

    let total_timeout = session
        .run(
            &ExecRequest {
                program: "sh".into(),
                args: vec![
                    "-lc".into(),
                    "while true; do printf x; sleep 0.05; done".into(),
                ],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_millis(350),
                inactivity_timeout: Duration::from_millis(200),
            },
            &CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await;
    assert!(matches!(total_timeout, Err(AppError::Timeout { .. })));

    let cancellation = CancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let cancelled = session
        .run(
            &ExecRequest {
                program: "sleep".into(),
                args: vec!["30".into()],
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(60),
                inactivity_timeout: Duration::from_secs(30),
            },
            &cancellation,
            &NoopRemoteOutputSink,
        )
        .await;
    assert!(matches!(cancelled, Err(AppError::Cancelled)));

    let operation_id = format!("phase4-cancel-{}", Uuid::now_v7().simple());
    println!("phase4 failure operation_id={operation_id}");
    let local = std::env::temp_dir().join(format!("{operation_id}.bin"));
    tokio::fs::write(&local, vec![0x33; 1024 * 1024])
        .await
        .expect("local fixture");
    let remote_path = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}.bin");
    let upload_cancellation = CancellationToken::new();
    upload_cancellation.cancel();
    let upload = session
        .upload(
            &UploadRequest {
                operation_id: operation_id.clone(),
                local_path: local.clone(),
                remote_path: remote_path.clone(),
                expected_sha256: None,
                overwrite: false,
                chunk_size: 1024 * 1024,
                inactivity_timeout: Duration::from_secs(30),
                minimum_bytes_per_second: 64 * 1024,
                minimum_total_timeout: Duration::from_secs(60),
            },
            &upload_cancellation,
            &NoopTransferProgressSink,
        )
        .await;
    assert!(matches!(upload, Err(AppError::Cancelled)));
    assert!(session.stat(&remote_path).await.is_err());
    assert!(
        session
            .stat(&format!("{remote_path}.part-{operation_id}"))
            .await
            .is_err()
    );
    tokio::fs::remove_file(local).await.expect("remove fixture");
    session.disconnect().await.expect("disconnect");
}
