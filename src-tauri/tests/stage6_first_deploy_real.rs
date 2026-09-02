mod common;

use std::collections::BTreeMap;

use common::{build_test_release, config, connect_pinned, run};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::domain::aio::release_render::ReleaseRenderContext;
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles, execute_connected_deployment,
};
use inxaiot_desk_buddy_lib::infrastructure::device_api::{AioRegistrationPayload, DeviceApiClient};
use inxaiot_desk_buddy_lib::infrastructure::release_template::render_release_templates;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "reinstalls both authorized registered nodes and verifies idempotent registration"]
async fn first_deploy_reinstall_is_healthy_registration_is_idempotent_and_staging_is_clean() {
    let remote = config();
    let root = common::project_root();
    let description =
        std::fs::read_to_string(root.join("test/测试数据说明.txt")).expect("description");
    let auth_key = common::line(&description, "平台API auth Key：").to_string();
    let mqtt_user = common::line(&description, "平台mqtt账号：").to_string();
    let mqtt_password = common::line(&description, "平台mqtt密码：").to_string();
    let release = build_test_release("phase6-first-2026.08.28");
    let display_macs = [
        "00:0C:29:3B:B9:33".to_string(),
        "00:0C:29:0B:71:F4".to_string(),
    ];
    let normalized_macs = display_macs
        .iter()
        .map(|mac| mac.replace(':', ""))
        .collect::<Vec<_>>();
    let operation_id = format!("phase6-first-{}", Uuid::now_v7().simple());
    println!("stage6 first operation_id={operation_id}");
    let plan = DeploymentPlan::build(DeploymentPlanInput {
        mode: DeploymentMode::FirstDeploy,
        target_macs: normalized_macs.clone(),
        artifact_path: release.release_dir.to_string_lossy().into_owned(),
        artifact_name: "Release".into(),
        artifact_version: release.manifest.version.clone(),
        service_name: None,
        image_name: None,
        images: release.images.clone(),
        batch_size: 2,
        concurrency: 2,
    })
    .expect("plan");
    for (index, host) in remote.hosts.iter().enumerate() {
        let context = ReleaseRenderContext {
            release_version: release.manifest.version.clone(),
            platform_host: remote.platform_host.clone(),
            platform_api_port: remote.platform_api_port.parse().expect("api"),
            platform_mqtt_host: remote.platform_host.clone(),
            platform_mqtt_port: remote.platform_mqtt_port.parse().expect("mqtt"),
            platform_mqtt_user: mqtt_user.clone(),
            platform_mqtt_password: mqtt_password.clone(),
            local_mqtt_user: "aio_local".into(),
            local_mqtt_password: "phase6-local".into(),
            auth_key: auth_key.clone(),
            node_name: format!("phase6-reinstall-{index}"),
            node_ip: host.clone(),
            node_mac: display_macs[index].clone(),
            images: release.images.clone(),
            ..ReleaseRenderContext::default()
        };
        let rendered = render_release_templates(
            &release.env_template,
            &release.host_template,
            &release.compose,
            &context,
        )
        .expect("render");
        let node_dir = release._temp.path().join(format!("first-{index}"));
        std::fs::create_dir_all(&node_dir).expect("node dir");
        let env = node_dir.join(".env");
        let host_info = node_dir.join("host-info.json");
        let compose = node_dir.join("docker-compose.yml");
        std::fs::write(&env, rendered.env).expect("env");
        std::fs::write(&host_info, rendered.host_info_json).expect("host info");
        std::fs::write(&compose, rendered.compose_runtime).expect("compose");
        let session = connect_pinned(&remote, host).await;
        let staging = format!(
            "/opt/data/.inxaiot-desk-buddy/{}/{}",
            operation_id, normalized_macs[index]
        );
        execute_connected_deployment(
            &session,
            &plan,
            &RemoteDeploymentFiles {
                local_agent: remote.agent.clone(),
                local_artifact: release.archive.clone(),
                local_env: Some(env),
                local_host_info: Some(host_info),
                local_compose: Some(compose),
            },
            &RemoteDeploymentConfig {
                operation_id: operation_id.clone(),
                release_fingerprint: String::new(),
                mac_normalized: normalized_macs[index].clone(),
                data_root: "/opt/data".into(),
                deploy_root: "/opt/data/deploy/inxvision-edge".into(),
                platform_api_host: remote.platform_host.clone(),
                platform_api_port: remote.platform_api_port.parse().expect("api"),
                platform_mqtt_host: remote.platform_host.clone(),
                platform_mqtt_port: remote.platform_mqtt_port.parse().expect("mqtt"),
                allow_existing_ports: true,
            },
            &CancellationToken::new(),
        )
        .await
        .expect("first deploy reinstall");
        DeviceApiClient::new(&format!("http://{host}:6002"), &auth_key)
            .expect("device api")
            .register_if_missing_with_retry(
                &AioRegistrationPayload {
                    name: format!("phase6-reinstall-{index}"),
                    ip: host.clone(),
                    mac: display_macs[index].clone(),
                    platform_ip: remote.platform_host.clone(),
                    platform_port: remote.platform_api_port.clone(),
                    auth_key: auth_key.clone(),
                    building_id: None,
                    addr_alias: Some("phase6-test".into()),
                },
                30,
                std::time::Duration::from_secs(2),
            )
            .await
            .expect("idempotent register");
        let clean = run(
            &session,
            "test",
            vec!["!".into(), "-e".into(), staging],
            BTreeMap::new(),
        )
        .await;
        assert_eq!(clean.exit_status, 0);
        session.disconnect().await.expect("disconnect");
    }
}
