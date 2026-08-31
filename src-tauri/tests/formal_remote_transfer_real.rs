mod common;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use common::{config, connect_pinned, run};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    DownloadRequest, FileTransferService, TransferProgress, TransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::core::error::AppResult;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Default)]
struct RecordingProgress {
    last: AtomicU64,
}

impl TransferProgressSink for RecordingProgress {
    fn emit(&self, progress: TransferProgress) -> AppResult<()> {
        let previous = self.last.fetch_max(progress.transferred, Ordering::SeqCst);
        assert!(progress.transferred >= previous);
        assert!(progress.transferred <= progress.total);
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn formal_remote_ports_redact_private_key_debug() {
    let config = config();
    let auth = common::auth(&config);
    let debug = format!("{auth:?}");
    assert!(!debug.contains(&config.private_key));
    assert!(debug.contains("REDACTED"));
}

#[tokio::test]
#[ignore = "requires both authorized Linux edge nodes"]
async fn private_key_pinning_command_upload_download_hash_and_exact_cleanup() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let bytes = vec![0x5a; 2 * 1024 * 1024];
    let expected_hash = sha256(&bytes);
    let operation_id = format!("phase4-{}", Uuid::now_v7().simple());
    println!("phase4 transfer operation_id={operation_id}");
    let local = std::env::temp_dir().join(format!("{operation_id}-upload.bin"));
    tokio::fs::write(&local, &bytes)
        .await
        .expect("local upload fixture");
    for (index, host) in config.hosts.iter().enumerate() {
        let session = connect_pinned(&config, host).await;
        let architecture = run(&session, "uname", vec!["-m".into()], BTreeMap::new()).await;
        assert_eq!(architecture.exit_status, 0, "{}", architecture.stderr);
        assert_eq!(architecture.stdout.trim(), "x86_64");

        let remote_path = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}.bin");
        let upload_progress = RecordingProgress::default();
        session
            .upload(
                &UploadRequest {
                    operation_id: operation_id.clone(),
                    local_path: local.clone(),
                    remote_path: remote_path.clone(),
                    expected_sha256: Some(expected_hash.clone()),
                    overwrite: false,
                    chunk_size: 1024 * 1024,
                    inactivity_timeout: Duration::from_secs(30),
                    minimum_bytes_per_second: 64 * 1024,
                    minimum_total_timeout: Duration::from_secs(60),
                },
                &CancellationToken::new(),
                &upload_progress,
            )
            .await
            .expect("verified atomic upload");
        assert_eq!(
            upload_progress.last.load(Ordering::SeqCst),
            bytes.len() as u64
        );
        assert_eq!(
            session.stat(&remote_path).await.expect("remote stat").size,
            Some(bytes.len() as u64)
        );

        let download = std::env::temp_dir().join(format!("{operation_id}-{index}-download.bin"));
        let download_progress = RecordingProgress::default();
        session
            .download(
                &DownloadRequest {
                    operation_id: operation_id.clone(),
                    remote_path: remote_path.clone(),
                    local_path: download.clone(),
                    expected_sha256: Some(expected_hash.clone()),
                    overwrite: false,
                    chunk_size: 1024 * 1024,
                    inactivity_timeout: Duration::from_secs(30),
                    minimum_bytes_per_second: 64 * 1024,
                    minimum_total_timeout: Duration::from_secs(60),
                },
                &CancellationToken::new(),
                &download_progress,
            )
            .await
            .expect("verified atomic download");
        assert_eq!(
            download_progress.last.load(Ordering::SeqCst),
            bytes.len() as u64
        );
        assert_eq!(
            sha256(&tokio::fs::read(&download).await.expect("download bytes")),
            expected_hash
        );
        tokio::fs::remove_file(&download)
            .await
            .expect("remove local download");
        session
            .remove_file(&remote_path)
            .await
            .expect("remove exact remote fixture");
        assert!(session.stat(&remote_path).await.is_err());
        assert!(
            session
                .stat(&format!("{remote_path}.part-{operation_id}"))
                .await
                .is_err()
        );
        session.disconnect().await.expect("disconnect");
    }
    tokio::fs::remove_file(&local)
        .await
        .expect("remove local upload fixture");
}
