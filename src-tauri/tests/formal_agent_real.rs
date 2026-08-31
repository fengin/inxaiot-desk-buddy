mod common;

use std::collections::BTreeMap;
use std::time::Duration;

use common::{config, connect_pinned, run};
use inxaiot_desk_buddy_lib::application::ports::file_transfer::{
    FileTransferService, NoopTransferProgressSink, UploadRequest,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires both authorized Linux edge nodes"]
async fn existing_agent_v1_is_compatible_with_typed_commands() {
    let config = config();
    let operation_id = format!("phase4-agent-{}", Uuid::now_v7().simple());
    println!("phase4 agent operation_id={operation_id}");
    for (index, host) in config.hosts.iter().enumerate() {
        let session = connect_pinned(&config, host).await;
        let remote_agent = format!("/opt/data/.inxaiot-desk-buddy-{operation_id}-{index}.sh");
        session
            .upload(
                &UploadRequest {
                    operation_id: operation_id.clone(),
                    local_path: config.agent.clone(),
                    remote_path: remote_agent.clone(),
                    expected_sha256: None,
                    overwrite: false,
                    chunk_size: 1024 * 1024,
                    inactivity_timeout: Duration::from_secs(30),
                    minimum_bytes_per_second: 64 * 1024,
                    minimum_total_timeout: Duration::from_secs(60),
                },
                &CancellationToken::new(),
                &NoopTransferProgressSink,
            )
            .await
            .expect("upload agent");
        assert_eq!(
            run(
                &session,
                "chmod",
                vec!["700".into(), remote_agent.clone()],
                BTreeMap::new(),
            )
            .await
            .exit_status,
            0
        );
        let version = run(
            &session,
            "sh",
            vec![remote_agent.clone(), "version".into()],
            BTreeMap::new(),
        )
        .await;
        assert_eq!(version.exit_status, 0, "{}", version.stderr);
        let version_json: Value =
            serde_json::from_str(version.stdout.trim()).expect("agent version json");
        assert_eq!(version_json["protocolVersion"].as_u64(), Some(1));

        let precheck = run(
            &session,
            "sh",
            vec![remote_agent.clone(), "precheck".into()],
            BTreeMap::from([
                ("ALLOW_EXISTING_PORTS".into(), "true".into()),
                ("MIN_FREE_MB".into(), "1".into()),
                ("PLATFORM_API_HOST".into(), config.platform_host.clone()),
                ("PLATFORM_API_PORT".into(), config.platform_api_port.clone()),
                ("PLATFORM_MQTT_HOST".into(), config.platform_host.clone()),
                (
                    "PLATFORM_MQTT_PORT".into(),
                    config.platform_mqtt_port.clone(),
                ),
            ]),
        )
        .await;
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

        let service_check = run(
            &session,
            "sh",
            vec![remote_agent.clone(), "service-check".into()],
            BTreeMap::from([("SERVICE_NAME".into(), "device-edge".into())]),
        )
        .await;
        assert_eq!(
            service_check.exit_status, 0,
            "{}\n{}",
            service_check.stdout, service_check.stderr
        );
        assert!(service_check.stdout.contains("\"step\":\"service_check\""));
        session
            .remove_file(&remote_agent)
            .await
            .expect("remove exact remote agent");
        session.disconnect().await.expect("disconnect");
    }
}
