mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned, run, run_with_timeout};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::AGENT_SHA256;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const COMPOSE_FAULT_SHIM: &str = r#"#!/usr/bin/env sh
set -eu
mode="${INX_FAULT_MODE:?}"
state="${INX_FAULT_STATE:?}"
real_compose() {
  if command -v docker-compose >/dev/null 2>&1; then
    docker-compose "$@"
  else
    docker compose "$@"
  fi
}
case " $* " in
  *" up "*)
    if [ ! -e "$state" ]; then
      : > "$state"
      if [ "$mode" = "compose-fail" ]; then
        exit 88
      fi
      real_compose "$@"
      docker stop inx-device-edge >/dev/null
      exit 0
    fi
    ;;
esac
real_compose "$@"
"#;

fn ensure(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message.into()).into())
    }
}

async fn upload(
    session: &inxaiot_desk_buddy_lib::infrastructure::remote::RemoteSession,
    operation_id: &str,
    local_path: std::path::PathBuf,
    remote_path: String,
    expected_sha256: String,
) -> Result<(), Box<dyn std::error::Error>> {
    session
        .upload(
            &UploadRequest {
                operation_id: operation_id.into(),
                local_path,
                remote_path,
                expected_sha256: Some(expected_sha256),
                overwrite: false,
                chunk_size: 1024 * 1024,
                inactivity_timeout: Duration::from_secs(30),
                minimum_bytes_per_second: 64 * 1024,
                minimum_total_timeout: Duration::from_secs(60),
            },
            &CancellationToken::new(),
            &NoopTransferProgressSink,
        )
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "injects real Compose start and health failures on both nodes and verifies rollback"]
async fn service_upgrade_compose_and_health_failures_restore_env_image_and_running_state() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let host_limit = std::env::var("INX_TEST_REMOTE_HOST_LIMIT")
        .ok()
        .map(|value| value.parse::<usize>().expect("remote host limit"))
        .unwrap_or(2);
    let host_start = std::env::var("INX_TEST_REMOTE_HOST_START")
        .ok()
        .map(|value| value.parse::<usize>().expect("remote host start"))
        .unwrap_or(0);
    assert!(host_limit >= 1 && host_start + host_limit <= config.hosts.len());
    let operation_id = format!("p011-{}", &Uuid::now_v7().simple().to_string()[..12]);
    let temp = tempfile::tempdir().expect("local fault shim temp");
    let local_shim = temp.path().join("compose-fault-shim.sh");
    tokio::fs::write(&local_shim, COMPOSE_FAULT_SHIM)
        .await
        .expect("write fault shim");
    let shim_sha256 = hex::encode(Sha256::digest(COMPOSE_FAULT_SHIM.as_bytes()));

    for (offset, host) in config
        .hosts
        .iter()
        .skip(host_start)
        .take(host_limit)
        .enumerate()
    {
        let index = host_start + offset;
        let session = connect_pinned(&config, host).await;
        let remote_agent = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}.sh");
        let remote_shim =
            format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}-compose.sh");
        let remote_image =
            format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}-device-edge.tar");
        let current_dir = "/opt/data/deploy/current";
        let mut cleanup_directories = Vec::new();
        let mut state_files = Vec::new();
        let mut expected_env_hash = None;
        let mut expected_image = None;

        let result: Result<(), Box<dyn std::error::Error>> = async {
            upload(
                &session,
                &operation_id,
                config.agent.clone(),
                remote_agent.clone(),
                AGENT_SHA256.into(),
            )
            .await?;
            upload(
                &session,
                &operation_id,
                local_shim.clone(),
                remote_shim.clone(),
                shim_sha256.clone(),
            )
            .await?;
            for path in [&remote_agent, &remote_shim] {
                ensure(
                    run(
                        &session,
                        "chmod",
                        vec!["700".into(), path.clone()],
                        BTreeMap::new(),
                    )
                    .await
                    .exit_status
                        == 0,
                    "chmod remote rollback fixture failed",
                )?;
                ensure(
                    run(
                        &session,
                        "sh",
                        vec!["-n".into(), path.clone()],
                        BTreeMap::new(),
                    )
                    .await
                    .exit_status
                        == 0,
                    "remote rollback fixture shell syntax failed",
                )?;
            }

            let image = run(
                &session,
                "docker",
                vec![
                    "inspect".into(),
                    "-f".into(),
                    "{{.Config.Image}}".into(),
                    "inx-device-edge".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(image.exit_status == 0, image.stderr)?;
            let current_image = image.stdout.trim().to_string();
            ensure(
                !current_image.is_empty(),
                "current device-edge image is empty",
            )?;
            expected_image = Some(current_image.clone());
            let env_hash = run(
                &session,
                "sha256sum",
                vec![format!("{current_dir}/.env")],
                BTreeMap::new(),
            )
            .await;
            ensure(env_hash.exit_status == 0, env_hash.stderr)?;
            let env_hash_before = env_hash
                .stdout
                .split_whitespace()
                .next()
                .ok_or_else(|| std::io::Error::other("current env hash missing"))?
                .to_string();
            expected_env_hash = Some(env_hash_before.clone());
            let save = run_with_timeout(
                &session,
                "docker",
                vec![
                    "save".into(),
                    "-o".into(),
                    remote_image.clone(),
                    current_image.clone(),
                ],
                BTreeMap::new(),
                Duration::from_secs(300),
            )
            .await;
            ensure(save.exit_status == 0, save.stderr)?;

            for (mode, expected_exit) in [("compose-fail", 84), ("health-fail", 86)] {
                let task_id = format!("{operation_id}-{index}-{mode}");
                let state_file = format!("/opt/data/.inxaiot-desk-buddy-{task_id}.state");
                let upgrade_dir = format!("/opt/data/deploy/service-upgrades/{task_id}");
                let backup_dir = format!("/opt/data/backup/{task_id}-before-service-upgrade");
                state_files.push(state_file.clone());
                cleanup_directories.push(upgrade_dir);
                cleanup_directories.push(backup_dir);
                let env = BTreeMap::from([
                    ("COMPOSE_CMD".into(), remote_shim.clone()),
                    ("INX_FAULT_MODE".into(), mode.into()),
                    ("INX_FAULT_STATE".into(), state_file.clone()),
                    ("TASK_ID".into(), task_id),
                    ("REMOTE_IMAGE".into(), remote_image.clone()),
                    ("SERVICE_NAME".into(), "device-edge".into()),
                    ("SERVICE_IMAGE_ENV".into(), "DEVICE_EDGE_IMAGE".into()),
                    ("SERVICE_IMAGE".into(), current_image.clone()),
                    ("BACKUP_RETENTION_DAYS".into(), "30".into()),
                    ("SERVICE_UPGRADE_RETENTION_DAYS".into(), "14".into()),
                    ("STAGING_RETENTION_DAYS".into(), "3".into()),
                ]);
                let failure = run_with_timeout(
                    &session,
                    "sh",
                    vec![remote_agent.clone(), "service-upgrade".into()],
                    env,
                    Duration::from_secs(300),
                )
                .await;
                ensure(
                    failure.exit_status == expected_exit,
                    format!(
                        "{mode} returned {}, expected {expected_exit}: {}\n{}",
                        failure.exit_status, failure.stdout, failure.stderr
                    ),
                )?;
                ensure(
                    failure
                        .stdout
                        .contains(r#""step":"rollback","status":"success""#),
                    format!("{mode} did not report rollback success"),
                )?;

                let restored_hash = run(
                    &session,
                    "sha256sum",
                    vec![format!("{current_dir}/.env")],
                    BTreeMap::new(),
                )
                .await;
                ensure(restored_hash.exit_status == 0, restored_hash.stderr)?;
                ensure(
                    restored_hash.stdout.starts_with(&env_hash_before),
                    format!("{mode} did not restore the original .env"),
                )?;
                let restored = run(
                    &session,
                    "docker",
                    vec![
                        "inspect".into(),
                        "-f".into(),
                        "{{.State.Status}}|{{.Config.Image}}".into(),
                        "inx-device-edge".into(),
                    ],
                    BTreeMap::new(),
                )
                .await;
                ensure(restored.exit_status == 0, restored.stderr)?;
                ensure(
                    restored.stdout.trim() == format!("running|{current_image}"),
                    format!("{mode} rollback did not restore running image"),
                )?;
            }
            Ok(())
        }
        .await;

        let restored_state = async {
            let (Some(env_hash), Some(image)) = (&expected_env_hash, &expected_image) else {
                return state_files.is_empty();
            };
            let current_hash = run(
                &session,
                "sha256sum",
                vec![format!("{current_dir}/.env")],
                BTreeMap::new(),
            )
            .await;
            let current_container = run(
                &session,
                "docker",
                vec![
                    "inspect".into(),
                    "-f".into(),
                    "{{.State.Status}}|{{.Config.Image}}".into(),
                    "inx-device-edge".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            current_hash.exit_status == 0
                && current_hash.stdout.starts_with(env_hash)
                && current_container.exit_status == 0
                && current_container.stdout.trim() == format!("running|{image}")
        }
        .await;
        let mut recovery_ok = restored_state;
        if !recovery_ok && !state_files.is_empty() {
            let recovery_script = format!(
                "set -eu\ncd '{current_dir}'\n\
                 if command -v docker-compose >/dev/null 2>&1; then\n\
                   docker-compose -f docker-compose.yml up -d --no-deps --force-recreate device-edge\n\
                 else\n\
                   docker compose -f docker-compose.yml up -d --no-deps --force-recreate device-edge\n\
                 fi"
            );
            let recovery = run_with_timeout(
                &session,
                "sh",
                vec!["-c".into(), recovery_script],
                BTreeMap::new(),
                Duration::from_secs(180),
            )
            .await;
            recovery_ok = recovery.exit_status == 0;
        }
        for path in &state_files {
            let _ = session.remove_file(path).await;
        }
        if recovery_ok && !state_files.is_empty() {
            let running = run(
                &session,
                "docker",
                vec![
                    "inspect".into(),
                    "-f".into(),
                    "{{.State.Status}}".into(),
                    "inx-device-edge".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            recovery_ok &= running.exit_status == 0 && running.stdout.trim() == "running";
        }
        if recovery_ok {
            for directory in &cleanup_directories {
                let cleanup = run(
                    &session,
                    "rm",
                    vec!["-rf".into(), "--".into(), directory.clone()],
                    BTreeMap::new(),
                )
                .await;
                assert_eq!(cleanup.exit_status, 0, "cleanup failed for {directory}");
            }
            for path in [&remote_image, &remote_shim, &remote_agent] {
                let _ = session.remove_file(path).await;
            }
        }
        session.disconnect().await.expect("disconnect");
        assert!(recovery_ok, "failed to restore device-edge during cleanup");
        result.expect("P0-11 Agent rollback gate");
    }
}
