mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{build_test_release, config, connect_pinned, run};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::domain::aio::release::sha256_file;
use inxaiot_desk_buddy_lib::domain::aio::release_render::ReleaseRenderContext;
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles, execute_connected_deployment,
};
use inxaiot_desk_buddy_lib::infrastructure::release_template::render_release_templates;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "installs a complete release on both authorized Linux edge nodes"]
async fn full_upgrade_backs_up_installs_checks_and_cleans_exact_staging() {
    let config = config();
    let version = "phase6-2026.08.28";
    let release = build_test_release(version);
    let temp = tempfile::tempdir().expect("temp render");
    let image_map = release.images.clone();
    let operation_id = format!("phase6-full-{}", Uuid::now_v7().simple());
    println!("stage6 full operation_id={operation_id}");
    let plan = DeploymentPlan::build(DeploymentPlanInput {
        mode: DeploymentMode::FullUpgrade,
        target_macs: config
            .hosts
            .iter()
            .enumerate()
            .map(|(index, _)| format!("0200000010{index:02}"))
            .collect(),
        image_files: release.image_files.clone(),
        artifact_path: release.release_dir.to_string_lossy().into_owned(),
        artifact_name: "镜像组合".into(),
        artifact_version: version.into(),
        service_name: None,
        image_name: None,
        service_image_environment_variable: None,
        images: image_map.clone(),
        batch_size: 2,
        concurrency: 2,
    })
    .expect("plan");
    for (index, host) in config.hosts.iter().enumerate() {
        let rendered = render_release_templates(
            &release.env_template,
            &release.host_template,
            &release.compose,
            &ReleaseRenderContext {
                release_version: version.into(),
                platform_host: config.platform_host.clone(),
                platform_api_port: config.platform_api_port.parse().expect("api"),
                platform_mqtt_host: config.platform_host.clone(),
                platform_mqtt_port: config.platform_mqtt_port.parse().expect("mqtt"),
                platform_mqtt_user: "edge_platform".into(),
                platform_mqtt_password: "test".into(),
                local_mqtt_user: "aio_local".into(),
                local_mqtt_password: "test".into(),
                auth_key: "phase6-test".into(),
                node_name: format!("phase6-{index}"),
                node_ip: host.clone(),
                node_mac: plan.target_macs[index].clone(),
                images: image_map.clone(),
                ..ReleaseRenderContext::default()
            },
        )
        .expect("render");
        let node_dir = temp.path().join(format!("node-{index}"));
        std::fs::create_dir_all(&node_dir).expect("node dir");
        let env = node_dir.join(".env");
        let host_info = node_dir.join("host-info.json");
        let rendered_compose = node_dir.join("docker-compose.yml");
        std::fs::write(&env, rendered.env).expect("node env");
        std::fs::write(&host_info, rendered.host_info_json).expect("host info");
        std::fs::write(&rendered_compose, rendered.compose_runtime).expect("node compose");
        let expected_compose_sha256 = sha256_file(&rendered_compose).expect("compose sha256");
        let session = connect_pinned(&config, host).await;
        let mac = &plan.target_macs[index];
        let staging = format!("/opt/data/.inxaiot-desk-buddy/{operation_id}/{mac}");
        let result = execute_connected_deployment(
            &session,
            &plan,
            &RemoteDeploymentFiles {
                local_agent: config.agent.clone(),
                local_artifact: release.archive.clone(),
                local_env: Some(env),
                local_host_info: Some(host_info),
                local_compose: Some(rendered_compose),
            },
            &RemoteDeploymentConfig {
                operation_id: operation_id.clone(),
                release_fingerprint: "a".repeat(64),
                mac_normalized: mac.clone(),
                data_root: "/opt/data".into(),
                deploy_root: "/opt/data/deploy".into(),
                platform_api_host: config.platform_host.clone(),
                platform_api_port: config.platform_api_port.parse().expect("api port"),
                platform_mqtt_host: config.platform_host.clone(),
                platform_mqtt_port: config.platform_mqtt_port.parse().expect("mqtt port"),
                allow_existing_ports: true,
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
        let current = run(
            &session,
            "test",
            vec!["-f".into(), "/opt/data/deploy/current/manifest.json".into()],
            BTreeMap::new(),
        )
        .await;
        let remote_compose_hash = run(
            &session,
            "sha256sum",
            vec![
                "--".into(),
                "/opt/data/deploy/current/docker-compose.yml".into(),
            ],
            BTreeMap::new(),
        )
        .await;
        session.disconnect().await.expect("disconnect");
        result.expect("full upgrade");
        assert_eq!(current.exit_status, 0);
        assert_eq!(remote_compose_hash.exit_status, 0);
        assert_eq!(
            remote_compose_hash.stdout.split_whitespace().next(),
            Some(expected_compose_sha256.as_str()),
            "rendered Compose did not reach remote current release"
        );
    }
    let services = release
        .manifest
        .images
        .iter()
        .map(|image| image.service.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(services.len(), 4);
}
