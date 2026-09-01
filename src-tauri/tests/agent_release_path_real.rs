mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned, run, run_with_timeout};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::AGENT_SHA256;
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
#[ignore = "verifies release path traversal rejection and immutable same-version install on both nodes"]
async fn unsafe_release_version_and_existing_release_are_rejected_before_compose_changes() {
    let config = config();
    assert!(config.hosts.len() >= 2);
    let operation_id = format!("p009-{}", &Uuid::now_v7().simple().to_string()[..12]);
    for (index, host) in config.hosts.iter().take(2).enumerate() {
        let session = connect_pinned(&config, host).await;
        let remote_agent = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}.sh");
        let fixture_dir = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}");
        let package = format!("{fixture_dir}/same-version.tar");
        let current = "/opt/data/deploy/inxvision-edge/current";
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
                    "chmod",
                    vec!["700".into(), remote_agent.clone()],
                    BTreeMap::new(),
                )
                .await
                .exit_status
                    == 0,
                "chmod agent failed",
            )?;
            let current_target = run(
                &session,
                "readlink",
                vec!["-f".into(), current.into()],
                BTreeMap::new(),
            )
            .await;
            ensure(current_target.exit_status == 0, current_target.stderr)?;
            let current_target = current_target.stdout.trim().to_string();
            ensure(
                current_target.starts_with("/opt/data/deploy/inxvision-edge/releases/"),
                "current release is outside releases root",
            )?;
            let version = run(
                &session,
                "sh",
                vec![
                    "-c".into(),
                    format!(
                        r#"awk -F'"' '/"version"[[:space:]]*:/ {{print $4; exit}}' '{current}/manifest.json'"#
                    ),
                ],
                BTreeMap::new(),
            )
            .await;
            ensure(version.exit_status == 0, version.stderr)?;
            let version = version.stdout.trim().to_string();
            ensure(!version.is_empty(), "current release version missing")?;

            let unsafe_version = format!("../escape-{operation_id}-{index}");
            let traversal = run(
                &session,
                "sh",
                vec![remote_agent.clone(), "backup".into()],
                BTreeMap::from([("RELEASE_VERSION".into(), unsafe_version)]),
            )
            .await;
            ensure(
                traversal.exit_status == 40,
                format!("unsafe version returned {}", traversal.exit_status),
            )?;
            ensure(
                !traversal.stdout.contains(r#""step":"compose""#),
                "unsafe version reached Compose",
            )?;
            let escaped = format!("/opt/data/deploy/inxvision-edge/escape-{operation_id}-{index}");
            ensure(
                run(
                    &session,
                    "test",
                    vec!["!".into(), "-e".into(), escaped],
                    BTreeMap::new(),
                )
                .await
                .exit_status
                    == 0,
                "unsafe release version created an escaped path",
            )?;

            let fixture_script = format!(
                "set -eu\nmkdir -p '{fixture_dir}'\n\
                 tar -cf '{package}' -C '{current}' docker-compose.yml manifest.json"
            );
            let fixture = run(
                &session,
                "sh",
                vec!["-c".into(), fixture_script],
                BTreeMap::new(),
            )
            .await;
            ensure(fixture.exit_status == 0, fixture.stderr)?;
            let immutable = run_with_timeout(
                &session,
                "sh",
                vec![remote_agent.clone(), "install".into()],
                BTreeMap::from([
                    ("RELEASE_VERSION".into(), version),
                    ("REMOTE_PACKAGE".into(), package),
                    ("REMOTE_ENV".into(), format!("{current}/.env")),
                    (
                        "REMOTE_HOST_INFO".into(),
                        "/opt/data/config/host-info.json".into(),
                    ),
                    (
                        "REMOTE_COMPOSE".into(),
                        format!("{current}/docker-compose.yml"),
                    ),
                    ("ALLOW_EXISTING_PORTS".into(), "true".into()),
                    ("MIN_FREE_MB".into(), "1".into()),
                    ("PLATFORM_API_HOST".into(), config.platform_host.clone()),
                    ("PLATFORM_API_PORT".into(), config.platform_api_port.clone()),
                    ("PLATFORM_MQTT_HOST".into(), config.platform_host.clone()),
                    ("PLATFORM_MQTT_PORT".into(), config.platform_mqtt_port.clone()),
                ]),
                Duration::from_secs(180),
            )
            .await;
            ensure(
                immutable.exit_status == 37,
                format!(
                    "same version returned {}, expected 37: {}\n{}",
                    immutable.exit_status, immutable.stdout, immutable.stderr
                ),
            )?;
            ensure(
                !immutable.stdout.contains(r#""step":"compose""#),
                "same-version attempt reached Compose",
            )?;
            let after_target = run(
                &session,
                "readlink",
                vec!["-f".into(), current.into()],
                BTreeMap::new(),
            )
            .await;
            ensure(after_target.exit_status == 0, after_target.stderr)?;
            ensure(
                after_target.stdout.trim() == current_target,
                "same-version attempt changed current release",
            )?;
            Ok(())
        }
        .await;

        let cleanup = run(
            &session,
            "rm",
            vec!["-rf".into(), "--".into(), fixture_dir.clone()],
            BTreeMap::new(),
        )
        .await;
        assert_eq!(cleanup.exit_status, 0, "fixture cleanup failed");
        let _ = session.remove_file(&remote_agent).await;
        session.disconnect().await.expect("disconnect");
        result.expect("P0-09 Agent release path gate");
    }
}
