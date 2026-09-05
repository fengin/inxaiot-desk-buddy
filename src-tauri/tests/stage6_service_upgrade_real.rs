mod common;

use std::collections::BTreeMap;

use common::{config, connect_pinned, project_root, run};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::domain::aio::release::inspect_image_archive;
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles, execute_connected_deployment,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "recreates device-edge on both authorized Linux edge nodes"]
async fn service_upgrade_runs_on_both_nodes_and_cleans_exact_staging() {
    let config = config();
    let image = project_root().join("test/images/device-edge-1.0.0.Alpha.20260819.tar");
    let archive = inspect_image_archive(&image).expect("inspect image");
    let repo_tag = archive
        .repo_tags
        .iter()
        .find(|tag| tag.contains("device-edge") && !tag.contains("web"))
        .cloned()
        .expect("device-edge repo tag");
    let operation_id = format!("phase6-service-{}", Uuid::now_v7().simple());
    println!("stage6 service operation_id={operation_id} image={repo_tag}");
    let plan = DeploymentPlan::build(DeploymentPlanInput {
        mode: DeploymentMode::ServiceUpgrade,
        target_macs: config
            .hosts
            .iter()
            .enumerate()
            .map(|(index, _)| format!("0200000000{index:02}"))
            .collect(),
        image_files: vec![
            inxaiot_desk_buddy_lib::domain::aio::deployment::DeploymentImageInput {
                service_name: "device-edge".into(),
                file_path: image.to_string_lossy().into_owned(),
                image_tag: repo_tag.clone(),
            },
        ],
        artifact_path: image.to_string_lossy().into_owned(),
        artifact_name: "device-edge".into(),
        artifact_version: repo_tag
            .rsplit_once(':')
            .map(|(_, version)| version)
            .unwrap_or(&repo_tag)
            .into(),
        service_name: Some("device-edge".into()),
        image_name: Some(repo_tag),
        service_image_environment_variable: Some("DEVICE_EDGE_IMAGE".into()),
        images: BTreeMap::new(),
        batch_size: 2,
        concurrency: 2,
    })
    .expect("plan");
    for (index, host) in config.hosts.iter().enumerate() {
        let session = connect_pinned(&config, host).await;
        let mac = &plan.target_macs[index];
        let staging = format!("/opt/data/.inxaiot-desk-buddy/{operation_id}/{mac}");
        let result = execute_connected_deployment(
            &session,
            &plan,
            &RemoteDeploymentFiles {
                local_agent: config.agent.clone(),
                local_artifact: image.clone(),
                local_env: None,
                local_host_info: None,
                local_compose: None,
            },
            &RemoteDeploymentConfig {
                operation_id: operation_id.clone(),
                release_fingerprint: String::new(),
                mac_normalized: mac.clone(),
                data_root: "/opt/data".into(),
                deploy_root: "/opt/data/deploy".into(),
                platform_api_host: config.platform_host.clone(),
                platform_api_port: config.platform_api_port.parse().expect("api port"),
                platform_mqtt_host: config.platform_host.clone(),
                platform_mqtt_port: config.platform_mqtt_port.parse().expect("mqtt port"),
                allow_existing_ports: false,
                published_ports: vec![1883, 18083, 6001, 6002, 7000],
            },
            &CancellationToken::new(),
        )
        .await;
        let _ = run(
            &session,
            "rm",
            vec!["-rf".into(), staging.clone()],
            BTreeMap::new(),
        )
        .await;
        let exists = run(
            &session,
            "test",
            vec!["!".into(), "-e".into(), staging],
            BTreeMap::new(),
        )
        .await;
        assert_eq!(exists.exit_status, 0, "staging cleanup failed");
        session.disconnect().await.expect("disconnect");
        result.expect("service upgrade");
    }
}
