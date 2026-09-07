mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned, run, run_with_timeout};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::domain::aio::release::sha256_file;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::AGENT_SHA256;
use inxaiot_desk_buddy_lib::infrastructure::release_archive::create_generated_release_tar;
use sha2::{Digest, Sha256};
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
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

fn setting(config: &str, name: &str) -> String {
    let key = format!("${{{name}:");
    config
        .split_once(&key)
        .expect("配置字段")
        .1
        .split_once('}')
        .expect("配置默认值")
        .0
        .trim()
        .into()
}

async fn workbench_read_only() -> sqlx::MySqlPool {
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let config = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .unwrap();
    MySqlPoolOptions::new()
        .max_connections(1)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET SESSION TRANSACTION READ ONLY")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(
            MySqlConnectOptions::new()
                .host(&setting(&config, "MYSQL_HOST"))
                .port(3306)
                .username(&setting(&config, "MYSQL_USER"))
                .password(&setting(&config, "MYSQL_PASSWORD"))
                .database("inxaiot_desk_buddy")
                .ssl_mode(MySqlSslMode::Disabled),
        )
        .await
        .expect("connect workbench read-only precondition")
}

async fn assert_host_without_active_lease(pool: &sqlx::MySqlPool, host: &str) {
    let active_lease_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM resource_lease lease_row JOIN aio_node node \
         ON node.mac_normalized = lease_row.resource_key \
         WHERE lease_row.resource_type = 'aio' AND node.ip = ? \
         AND lease_row.lease_state = 'active' AND lease_row.expires_at > UTC_TIMESTAMP(6)",
    )
    .bind(host)
    .fetch_one(pool)
    .await
    .expect("read active deployment lease");
    assert_eq!(
        active_lease_count, 0,
        "target host has an active workbench deployment lease"
    );
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

fn invalid_image_release(
    temp: &tempfile::TempDir,
    version: &str,
    image_tag: &str,
) -> (std::path::PathBuf, String) {
    let release = temp.path().join("invalid-release");
    let images = release.join("images");
    let checksums = release.join("checksums");
    std::fs::create_dir_all(&images).unwrap();
    std::fs::create_dir_all(&checksums).unwrap();
    let image = images.join("device-edge.tar");
    let tag = images.join("device-edge.tag");
    std::fs::write(&image, b"not-a-docker-image").unwrap();
    std::fs::write(&tag, format!("{image_tag}\n")).unwrap();
    let image_sha256 = sha256_file(&image).unwrap();
    let tag_sha256 = sha256_file(&tag).unwrap();
    std::fs::write(
        release.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schemaVersion": 1,
            "version": version,
            "images": [{
                "service": "device-edge",
                "image": image_tag,
                "archive": "images/device-edge.tar",
                "sha256": image_sha256
            }]
        }))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        checksums.join("sha256.txt"),
        format!("{image_sha256}  images/device-edge.tar\n{tag_sha256}  images/device-edge.tag\n"),
    )
    .unwrap();
    let archive = temp.path().join("invalid-release.tar");
    create_generated_release_tar(&release, &archive).unwrap();
    let archive_sha256 = sha256_file(&archive).unwrap();
    (archive, archive_sha256)
}

async fn remote_release_state(
    session: &inxaiot_desk_buddy_lib::infrastructure::remote::RemoteSession,
) -> String {
    let state = run_with_timeout(
        session,
        "sh",
        vec![
            "-c".into(),
            concat!(
                "set -eu\n",
                "release_dir=$(readlink -f /opt/data/deploy/current)\n",
                "cd \"$release_dir\"\n",
                "if command -v docker-compose >/dev/null 2>&1; then compose_cmd=docker-compose; else compose_cmd='docker compose'; fi\n",
                "services=$($compose_cmd -f docker-compose.yml config --services)\n",
                "printf 'release=%s\\n' \"$release_dir\"\n",
                "for service in $services; do\n",
                "  ids=$($compose_cmd -f docker-compose.yml ps -q \"$service\")\n",
                "  [ \"$(printf '%s\\n' \"$ids\" | awk 'NF {count++} END {print count+0}')\" = 1 ]\n",
                "  docker inspect -f \"$service|{{.Id}}|{{.State.Status}}|{{.State.StartedAt}}|{{.Config.Image}}|{{.Image}}\" \"$ids\"\n",
                "done\n",
                "sha256sum /opt/data/config/host-info.json\n"
            )
            .into(),
        ],
        BTreeMap::new(),
        Duration::from_secs(60),
    )
    .await;
    assert_eq!(state.exit_status, 0, "read remote state: {}", state.stderr);
    state.stdout
}

#[tokio::test]
#[ignore = "injects real same-tag Compose and health failures and verifies image-ID rollback"]
async fn service_upgrade_compose_and_health_failures_restore_env_image_and_running_state() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let workbench = workbench_read_only().await;
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
        assert_host_without_active_lease(&workbench, host).await;
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
        let mut expected_image_id = None;
        let mut candidate_image_id = None;
        let mut candidate_image_tag = None;

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
            let snapshot_check = run_with_timeout(
                &session,
                "sh",
                vec![
                    "-c".into(),
                    concat!(
                        "set -eu\n",
                        "agent_path=\"$1\"\n",
                        "set -- version\n",
                        ". \"$agent_path\" >/dev/null\n",
                        "snapshot_dir=\"$(mktemp -d)\"\n",
                        "trap 'rm -rf \"$snapshot_dir\"' EXIT\n",
                        "release_dir=\"$(current_release_dir)\"\n",
                        "[ -n \"$release_dir\" ]\n",
                        "services=\"$(cd \"$release_dir\" && compose -f docker-compose.yml config --services)\"\n",
                        "for service in $services; do\n",
                        "  service_runtime_container_id \"$release_dir\" \"$service\" >/dev/null || { echo \"container:$service\"; exit 71; }\n",
                        "  service_runtime_image_reference \"$release_dir\" \"$service\" >/dev/null || { echo \"reference:$service\"; exit 72; }\n",
                        "  service_runtime_image_id \"$release_dir\" \"$service\" >/dev/null || { echo \"image-id:$service\"; exit 73; }\n",
                        "done\n",
                        "snapshot_release_images \"$release_dir\" \"$snapshot_dir\" || { echo snapshot; exit 74; }\n",
                        "verify_release_image_snapshot \"$release_dir\" \"$snapshot_dir\" || { echo verify; exit 75; }\n"
                    )
                    .into(),
                    "fixture".into(),
                    remote_agent.clone(),
                ],
                BTreeMap::new(),
                Duration::from_secs(60),
            )
            .await;
            ensure(
                snapshot_check.exit_status == 0,
                format!(
                    "real current release image snapshot check failed with {}: {}\n{}",
                    snapshot_check.exit_status, snapshot_check.stdout, snapshot_check.stderr
                ),
            )?;

            let image = run(
                &session,
                "docker",
                vec![
                    "inspect".into(),
                    "-f".into(),
                    "{{.Config.Image}}|{{.Image}}".into(),
                    "inx-device-edge".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(image.exit_status == 0, image.stderr)?;
            let (current_image, current_image_id) = image
                .stdout
                .trim()
                .split_once('|')
                .ok_or_else(|| std::io::Error::other("current image name/ID is incomplete"))?;
            let current_image = current_image.to_string();
            let current_image_id = current_image_id.to_string();
            ensure(
                !current_image.is_empty() && current_image_id.starts_with("sha256:"),
                "current device-edge image name or ID is invalid",
            )?;
            expected_image = Some(current_image.clone());
            expected_image_id = Some(current_image_id.clone());
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
            let candidate_tag = format!(
                "inx-review/device-edge-same-tag:{}-{index}",
                operation_id.to_lowercase()
            );
            candidate_image_tag = Some(candidate_tag.clone());
            let commit = run_with_timeout(
                &session,
                "docker",
                vec![
                    "commit".into(),
                    "--pause=false".into(),
                    "--change".into(),
                    format!("LABEL inx.review.operation={operation_id}"),
                    "inx-device-edge".into(),
                    candidate_tag.clone(),
                ],
                BTreeMap::new(),
                Duration::from_secs(300),
            )
            .await;
            ensure(commit.exit_status == 0, commit.stderr)?;
            let new_image_id = commit.stdout.trim().to_string();
            ensure(
                new_image_id.starts_with("sha256:") && new_image_id != current_image_id,
                "failed to create a distinct same-tag image",
            )?;
            candidate_image_id = Some(new_image_id.clone());
            let overwrite_tag = run(
                &session,
                "docker",
                vec![
                    "image".into(),
                    "tag".into(),
                    new_image_id.clone(),
                    current_image.clone(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(overwrite_tag.exit_status == 0, overwrite_tag.stderr)?;
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
            let restore_tag = run(
                &session,
                "docker",
                vec![
                    "image".into(),
                    "tag".into(),
                    current_image_id.clone(),
                    current_image.clone(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(restore_tag.exit_status == 0, restore_tag.stderr)?;
            let remove_candidate_tag = run(
                &session,
                "docker",
                vec!["image".into(), "rm".into(), candidate_tag.clone()],
                BTreeMap::new(),
            )
            .await;
            ensure(
                remove_candidate_tag.exit_status == 0,
                remove_candidate_tag.stderr,
            )?;

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
                        "{{.State.Status}}|{{.Config.Image}}|{{.Image}}".into(),
                        "inx-device-edge".into(),
                    ],
                    BTreeMap::new(),
                )
                .await;
                ensure(restored.exit_status == 0, restored.stderr)?;
                ensure(
                    restored.stdout.trim() == format!("running|{current_image}|{current_image_id}"),
                    format!("{mode} rollback did not restore the previous image ID"),
                )?;
                let restored_tag = run(
                    &session,
                    "docker",
                    vec![
                        "image".into(),
                        "inspect".into(),
                        "-f".into(),
                        "{{.Id}}".into(),
                        current_image.clone(),
                    ],
                    BTreeMap::new(),
                )
                .await;
                ensure(restored_tag.exit_status == 0, restored_tag.stderr)?;
                ensure(
                    restored_tag.stdout.trim() == current_image_id,
                    format!("{mode} rollback did not restore the previous tag target"),
                )?;
            }
            Ok(())
        }
        .await;

        let restored_state = async {
            let (Some(env_hash), Some(image), Some(image_id)) =
                (&expected_env_hash, &expected_image, &expected_image_id)
            else {
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
                    "{{.State.Status}}|{{.Config.Image}}|{{.Image}}".into(),
                    "inx-device-edge".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            current_hash.exit_status == 0
                && current_hash.stdout.starts_with(env_hash)
                && current_container.exit_status == 0
                && current_container.stdout.trim() == format!("running|{image}|{image_id}")
        }
        .await;
        let mut recovery_ok = restored_state;
        if !recovery_ok && !state_files.is_empty() {
            let tag_restored =
                if let (Some(image), Some(image_id)) = (&expected_image, &expected_image_id) {
                    let restore_tag = run(
                        &session,
                        "docker",
                        vec![
                            "image".into(),
                            "tag".into(),
                            image_id.clone(),
                            image.clone(),
                        ],
                        BTreeMap::new(),
                    )
                    .await;
                    restore_tag.exit_status == 0
                } else {
                    false
                };
            if tag_restored {
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
                assert!(
                    session.stat(path).await.is_err(),
                    "remote test file remains: {path}"
                );
            }
            if let Some(tag) = &candidate_image_tag {
                let _ = run(
                    &session,
                    "docker",
                    vec!["image".into(), "rm".into(), tag.clone()],
                    BTreeMap::new(),
                )
                .await;
            }
            if let Some(image_id) = &candidate_image_id {
                let removed = run(
                    &session,
                    "docker",
                    vec!["image".into(), "rm".into(), image_id.clone()],
                    BTreeMap::new(),
                )
                .await;
                assert_eq!(
                    removed.exit_status, 0,
                    "failed to remove test-created image: {}",
                    removed.stderr
                );
                let absent = run(
                    &session,
                    "docker",
                    vec!["image".into(), "inspect".into(), image_id.clone()],
                    BTreeMap::new(),
                )
                .await;
                assert_ne!(absent.exit_status, 0, "test-created image still exists");
            }
        }
        session.disconnect().await.expect("disconnect");
        assert!(recovery_ok, "failed to restore device-edge during cleanup");
        result.expect("P0-11 Agent rollback gate");
    }
    workbench.close().await;
}

#[tokio::test]
#[ignore = "injects a real full-install image-load failure on node 79 and verifies host-info never becomes active"]
async fn full_install_image_load_failure_keeps_previous_host_info_and_runtime() {
    let config = config();
    let host = config.hosts.first().expect("authorized host 79");
    assert_eq!(host, "192.168.3.79");
    let workbench = workbench_read_only().await;
    assert_host_without_active_lease(&workbench, host).await;
    let operation_id = format!("r03-{}", &Uuid::now_v7().simple().to_string()[..12]);
    let version = operation_id.clone();
    let temp = tempfile::tempdir().expect("local R03 temp");
    let image_tag = format!("inx-review/invalid:{version}");
    let (local_package, package_sha256) = invalid_image_release(&temp, &version, &image_tag);
    let remote_agent = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}.sh");
    let remote_package = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}.tar");
    let remote_host_candidate = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-host.json");
    let remote_host_backup = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-host.before");
    let remote_new_release = format!("/opt/data/deploy/releases/{version}");
    let session = connect_pinned(&config, host).await;
    let mut before_state = None;

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
            local_package,
            remote_package.clone(),
            package_sha256,
        )
        .await?;
        for path in [&remote_agent, &remote_package] {
            let chmod = run(
                &session,
                "chmod",
                vec!["600".into(), path.clone()],
                BTreeMap::new(),
            )
            .await;
            ensure(chmod.exit_status == 0, chmod.stderr)?;
        }
        let chmod_agent = run(
            &session,
            "chmod",
            vec!["700".into(), remote_agent.clone()],
            BTreeMap::new(),
        )
        .await;
        ensure(chmod_agent.exit_status == 0, chmod_agent.stderr)?;
        let current = run(
            &session,
            "readlink",
            vec!["-f".into(), "/opt/data/deploy/current".into()],
            BTreeMap::new(),
        )
        .await;
        ensure(current.exit_status == 0, current.stderr)?;
        let current_dir = current.stdout.trim().to_string();
        ensure(!current_dir.is_empty(), "current release is empty")?;
        let state = remote_release_state(&session).await;
        before_state = Some(state.clone());
        for destination in [&remote_host_backup, &remote_host_candidate] {
            let copy = run(
                &session,
                "cp",
                vec![
                    "/opt/data/config/host-info.json".into(),
                    destination.clone(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(copy.exit_status == 0, copy.stderr)?;
        }
        let change_candidate = run(
            &session,
            "sh",
            vec![
                "-c".into(),
                "printf '\\n ' >> \"$1\"".into(),
                "fixture".into(),
                remote_host_candidate.clone(),
            ],
            BTreeMap::new(),
        )
        .await;
        ensure(change_candidate.exit_status == 0, change_candidate.stderr)?;
        let hidden_before = run(
            &session,
            "sh",
            vec![
                "-c".into(),
                "find /opt/data/config -maxdepth 1 -name '.host-info.json.*' -print | sort".into(),
            ],
            BTreeMap::new(),
        )
        .await;
        ensure(hidden_before.exit_status == 0, hidden_before.stderr)?;
        let failure = run_with_timeout(
            &session,
            "sh",
            vec![remote_agent.clone(), "install".into()],
            BTreeMap::from([
                ("DATA_ROOT".into(), "/opt/data".into()),
                ("DEPLOY_ROOT".into(), "/opt/data/deploy".into()),
                ("RELEASE_VERSION".into(), version.clone()),
                ("RELEASE_FINGERPRINT".into(), String::new()),
                ("REMOTE_PACKAGE".into(), remote_package.clone()),
                ("REMOTE_ENV".into(), format!("{current_dir}/.env")),
                (
                    "REMOTE_COMPOSE".into(),
                    format!("{current_dir}/docker-compose.yml"),
                ),
                ("REMOTE_HOST_INFO".into(), remote_host_candidate.clone()),
                ("TASK_ID".into(), operation_id.clone()),
                ("ALLOW_EXISTING_PORTS".into(), "true".into()),
                ("PORTS".into(), String::new()),
                ("PLATFORM_API_HOST".into(), config.platform_host.clone()),
                ("PLATFORM_API_PORT".into(), config.platform_api_port.clone()),
                ("PLATFORM_MQTT_HOST".into(), config.platform_host.clone()),
                (
                    "PLATFORM_MQTT_PORT".into(),
                    config.platform_mqtt_port.clone(),
                ),
            ]),
            Duration::from_secs(300),
        )
        .await;
        ensure(
            failure.exit_status == 36,
            format!(
                "image-load failure returned {}, expected 36: {}\n{}",
                failure.exit_status, failure.stdout, failure.stderr
            ),
        )?;
        let after_state = remote_release_state(&session).await;
        ensure(
            after_state == state,
            "image-load failure changed current release, host-info or containers",
        )?;
        let hidden_after = run(
            &session,
            "sh",
            vec![
                "-c".into(),
                "find /opt/data/config -maxdepth 1 -name '.host-info.json.*' -print | sort".into(),
            ],
            BTreeMap::new(),
        )
        .await;
        ensure(hidden_after.exit_status == 0, hidden_after.stderr)?;
        ensure(
            hidden_after.stdout == hidden_before.stdout,
            "host-info candidate remained after failed install",
        )?;
        Ok(())
    }
    .await;

    if session.stat(&remote_host_backup).await.is_ok() {
        let restore = run(
            &session,
            "cp",
            vec![
                remote_host_backup.clone(),
                "/opt/data/config/host-info.json".into(),
            ],
            BTreeMap::new(),
        )
        .await;
        assert_eq!(
            restore.exit_status, 0,
            "restore host-info: {}",
            restore.stderr
        );
    }
    let cleanup = run(
        &session,
        "rm",
        vec![
            "-rf".into(),
            "--".into(),
            remote_new_release,
            remote_agent,
            remote_package,
            remote_host_candidate,
            remote_host_backup,
        ],
        BTreeMap::new(),
    )
    .await;
    assert_eq!(cleanup.exit_status, 0, "R03 cleanup: {}", cleanup.stderr);
    if let Some(before) = before_state {
        assert_eq!(
            remote_release_state(&session).await,
            before,
            "R03 cleanup did not restore the original runtime"
        );
    }
    session.disconnect().await.expect("disconnect");
    workbench.close().await;
    result.expect("R03 host-info activation gate");
}
