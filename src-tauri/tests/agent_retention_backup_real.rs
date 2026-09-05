mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned, run, run_with_timeout};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::AGENT_SHA256;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn ensure(condition: bool, message: impl Into<String>) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        Ok(())
    } else {
        Err(std::io::Error::other(message.into()).into())
    }
}

#[tokio::test]
#[ignore = "stops and restarts real Compose; prunes only uniquely marked 30/14/3-day fixtures"]
async fn agent_consistent_backup_and_retention_run_on_both_nodes_with_exact_cleanup() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let operation_id = format!("p108-{}", &Uuid::now_v7().simple().to_string()[..12]);

    for (index, host) in config.hosts.iter().take(2).enumerate() {
        let session = connect_pinned(&config, host).await;
        let remote_agent = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}.sh");
        let release_version = format!("{operation_id}-{index}");
        let backup_old = format!("/opt/data/backup/{operation_id}-{index}-expired");
        let backup_fresh = format!("/opt/data/backup/{operation_id}-{index}-fresh");
        let upgrade_old =
            format!("/opt/data/deploy/service-upgrades/{operation_id}-{index}-expired");
        let upgrade_fresh =
            format!("/opt/data/deploy/service-upgrades/{operation_id}-{index}-fresh");
        let staging_old = format!("/opt/data/.inxaiot-desk-buddy/{operation_id}-{index}-expired");
        let staging_fresh = format!("/opt/data/.inxaiot-desk-buddy/{operation_id}-{index}-fresh");
        let mut generated_backup = None;

        let result: Result<(), Box<dyn std::error::Error>> = async {
            session
                .upload(
                    &UploadRequest {
                        operation_id: operation_id.clone(),
                        local_path: config.agent.clone(),
                        remote_path: remote_agent.clone(),
                        expected_sha256: Some(AGENT_SHA256.into()),
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
            ensure(
                run(
                    &session,
                    "sh",
                    vec!["-n".into(), remote_agent.clone()],
                    BTreeMap::new(),
                )
                .await
                .exit_status
                    == 0,
                "agent POSIX shell syntax check failed",
            )?;

            let fixture_script = format!(
                "set -eu\n\
                 mkdir -p '{backup_old}/nested' '{backup_fresh}' '{upgrade_old}/nested' '{upgrade_fresh}' '{staging_old}/nested' '{staging_fresh}'\n\
                 printf p108 > '{backup_old}/nested/evidence'\n\
                 printf p108 > '{upgrade_old}/nested/evidence'\n\
                 printf p108 > '{staging_old}/nested/evidence'\n\
                 touch -d '31 days ago' '{backup_old}'\n\
                 touch -d '29 days ago' '{backup_fresh}'\n\
                 touch -d '15 days ago' '{upgrade_old}'\n\
                 touch -d '13 days ago' '{upgrade_fresh}'\n\
                 touch -d '4 days ago' '{staging_old}'\n\
                 touch -d '2 days ago' '{staging_fresh}'"
            );
            let fixture = run(
                &session,
                "sh",
                vec!["-c".into(), fixture_script],
                BTreeMap::new(),
            )
            .await;
            ensure(
                fixture.exit_status == 0,
                format!("retention fixture failed: {}", fixture.stderr),
            )?;

            let precheck = run(
                &session,
                "sh",
                vec![remote_agent.clone(), "precheck".into()],
                BTreeMap::from([
                    ("ALLOW_EXISTING_PORTS".into(), "true".into()),
                    ("MIN_FREE_MB".into(), "1".into()),
                    ("BACKUP_RETENTION_DAYS".into(), "30".into()),
                    ("SERVICE_UPGRADE_RETENTION_DAYS".into(), "14".into()),
                    ("STAGING_RETENTION_DAYS".into(), "3".into()),
                    ("PLATFORM_API_HOST".into(), config.platform_host.clone()),
                    ("PLATFORM_API_PORT".into(), config.platform_api_port.clone()),
                    ("PLATFORM_MQTT_HOST".into(), config.platform_host.clone()),
                    ("PLATFORM_MQTT_PORT".into(), config.platform_mqtt_port.clone()),
                ]),
            )
            .await;
            ensure(
                precheck.exit_status == 0,
                format!("precheck failed: {}\n{}", precheck.stdout, precheck.stderr),
            )?;
            let retention_assertion = format!(
                "test ! -e '{backup_old}'\n\
                 test -d '{backup_fresh}'\n\
                 test ! -e '{upgrade_old}'\n\
                 test -d '{upgrade_fresh}'\n\
                 test ! -e '{staging_old}'\n\
                 test -d '{staging_fresh}'"
            );
            ensure(
                run(
                    &session,
                    "sh",
                    vec!["-c".into(), retention_assertion],
                    BTreeMap::new(),
                )
                .await
                .exit_status
                    == 0,
                "30/14/3-day retention boundary mismatch",
            )?;

            let database_sources = run(
                &session,
                "find",
                vec![
                    "/opt/data/device-edge".into(),
                    "/opt/data/rule-engine".into(),
                    "-maxdepth".into(),
                    "3".into(),
                    "-type".into(),
                    "f".into(),
                    "(".into(),
                    "-name".into(),
                    "*.db".into(),
                    "-o".into(),
                    "-name".into(),
                    "*.sqlite".into(),
                    "-o".into(),
                    "-name".into(),
                    "*.sqlite3".into(),
                    ")".into(),
                    "-print".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(database_sources.exit_status == 0, database_sources.stderr)?;
            ensure(
                !database_sources.stdout.trim().is_empty(),
                "no real database files found, consistent stop/copy/start gate cannot be proven",
            )?;

            let backup = run_with_timeout(
                &session,
                "sh",
                vec![remote_agent.clone(), "backup".into()],
                BTreeMap::from([
                    ("RELEASE_VERSION".into(), release_version.clone()),
                    ("BACKUP_RETENTION_DAYS".into(), "30".into()),
                    ("SERVICE_UPGRADE_RETENTION_DAYS".into(), "14".into()),
                    ("STAGING_RETENTION_DAYS".into(), "3".into()),
                ]),
                Duration::from_secs(300),
            )
            .await;
            ensure(
                backup.exit_status == 0,
                format!("consistent backup failed: {}\n{}", backup.stdout, backup.stderr),
            )?;
            let backup_path = backup
                .stdout
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .find(|event| event["step"] == "backup" && event["status"] == "success")
                .and_then(|event| event["message"].as_str().map(str::to_owned))
                .ok_or_else(|| std::io::Error::other("backup success event missing"))?;
            ensure(
                backup_path.starts_with(&format!(
                    "/opt/data/backup/{release_version}-before-upgrade-"
                )),
                "backup event returned an unexpected path",
            )?;
            ensure(
                backup_path
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"/_-.".contains(&byte)),
                "backup path contains unsafe characters",
            )?;
            generated_backup = Some(backup_path.clone());

            for required in ["docker-compose.yml", ".env", "manifest.json", "host-info.json"] {
                ensure(
                    run(
                        &session,
                        "test",
                        vec!["-f".into(), format!("{backup_path}/{required}")],
                        BTreeMap::new(),
                    )
                    .await
                    .exit_status
                        == 0,
                    format!("backup is missing {required}"),
                )?;
            }
            let database_backups = run(
                &session,
                "find",
                vec![
                    format!("{backup_path}/data"),
                    "-type".into(),
                    "f".into(),
                    "-print".into(),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(database_backups.exit_status == 0, database_backups.stderr)?;
            ensure(
                database_backups.stdout.lines().any(|path| {
                    path.starts_with(&format!("{backup_path}/data/device-edge/"))
                        || path.starts_with(&format!("{backup_path}/data/rule-engine/"))
                }),
                "database backups did not preserve a service-relative path",
            )?;
            for container in [
                "inx-edge-emqx",
                "inx-device-edge",
                "inx-rule-engine",
                "inx-device-edge-web",
            ] {
                let inspect = run(
                    &session,
                    "docker",
                    vec![
                        "inspect".into(),
                        "-f".into(),
                        "{{.State.Status}}".into(),
                        container.into(),
                    ],
                    BTreeMap::new(),
                )
                .await;
                ensure(
                    inspect.exit_status == 0 && inspect.stdout.trim() == "running",
                    format!("container {container} was not restored to running"),
                )?;
            }
            Ok(())
        }
        .await;

        let mut cleanup_paths = vec![
            backup_old,
            backup_fresh,
            upgrade_old,
            upgrade_fresh,
            staging_old,
            staging_fresh,
        ];
        if let Some(path) = generated_backup {
            cleanup_paths.push(path);
        }
        for path in cleanup_paths {
            let cleanup = run(
                &session,
                "rm",
                vec!["-rf".into(), "--".into(), path.clone()],
                BTreeMap::new(),
            )
            .await;
            assert_eq!(cleanup.exit_status, 0, "cleanup failed for {path}");
        }
        session
            .remove_file(&remote_agent)
            .await
            .expect("remove exact remote agent");
        session.disconnect().await.expect("disconnect");
        result.expect("P1-08 real gate");
    }
}
