mod common;

use std::collections::{BTreeMap, BTreeSet};

use common::{config, connect_pinned, project_root, run};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::domain::aio::release::{
    ReleaseImage, ReleaseManifest, ReleaseTemplates, inspect_image_archive,
    inspect_release_directory,
};
use inxaiot_desk_buddy_lib::domain::aio::release_render::ReleaseRenderContext;
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles, execute_connected_deployment,
};
use inxaiot_desk_buddy_lib::infrastructure::release_archive::create_release_tar;
use inxaiot_desk_buddy_lib::infrastructure::release_template::render_release_templates;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[tokio::test]
#[ignore = "installs a complete release on both authorized Linux edge nodes"]
async fn full_upgrade_backs_up_installs_checks_and_cleans_exact_staging() {
    let config = config();
    let root = project_root();
    let temp = tempfile::tempdir().expect("temp release");
    let release = temp.path().join("release");
    std::fs::create_dir_all(release.join("images")).expect("images");
    std::fs::create_dir_all(release.join("templates")).expect("templates");
    std::fs::copy(
        root.join("test/docker-compose.yml"),
        release.join("docker-compose.yml"),
    )
    .expect("compose");
    std::fs::copy(
        root.join("test/templates/env.template"),
        release.join("templates/env.template"),
    )
    .expect("env template");
    std::fs::copy(
        root.join("test/templates/host-info.json.template"),
        release.join("templates/host-info.json.template"),
    )
    .expect("host template");
    let image_files = [
        ("emqx", "emqx-5.10.0.tar"),
        ("device-edge", "device-edge-1.0.0.Alpha.20260819.tar"),
        ("rule-engine", "rule-engine-1.0.0.Alpha.20260810.tar"),
        (
            "device-edge-web",
            "device-edge-web-1.0.1.Alpha.20260812.tar",
        ),
    ];
    let mut images = Vec::new();
    for (service, file) in image_files {
        let source = root.join("test/images").join(file);
        let destination = release.join("images").join(file);
        std::fs::hard_link(&source, &destination)
            .or_else(|_| std::fs::copy(&source, &destination).map(|_| ()))
            .expect("release image");
        let archive = inspect_image_archive(&destination).expect("inspect image");
        let tag = archive
            .repo_tags
            .iter()
            .find(|tag| match service {
                "device-edge" => tag.contains("device-edge") && !tag.contains("web"),
                _ => tag.contains(service),
            })
            .cloned()
            .expect("service repo tag");
        images.push(ReleaseImage {
            service: service.into(),
            image: tag,
            archive: format!("images/{file}"),
        });
    }
    let version = "phase6-2026.08.28";
    let manifest = ReleaseManifest {
        schema_version: 1,
        version: version.into(),
        compose_file: "docker-compose.yml".into(),
        images: images.clone(),
        templates: ReleaseTemplates {
            env: "templates/env.template".into(),
            host_info: "templates/host-info.json.template".into(),
        },
        runtime: inxaiot_desk_buddy_lib::domain::aio::release::ReleaseRuntime {
            os: "linux".into(),
            arch: "x86_64".into(),
            docker: ">=20.10".into(),
            compose: ">=2.0".into(),
        },
    };
    std::fs::write(
        release.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest).expect("manifest json"),
    )
    .expect("manifest");
    let validation = inspect_release_directory(&release).expect("validate release");
    assert!(validation.valid, "{:?}", validation.errors);
    let archive = create_release_tar(&release, &temp.path().join("release.tar")).expect("tar");
    let env_template =
        std::fs::read_to_string(release.join("templates/env.template")).expect("env");
    let host_template =
        std::fs::read_to_string(release.join("templates/host-info.json.template")).expect("host");
    let compose = std::fs::read_to_string(release.join("docker-compose.yml")).expect("compose");
    let image_map = images
        .iter()
        .map(|image| (image.service.clone(), image.image.clone()))
        .collect::<BTreeMap<_, _>>();
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
        artifact_path: release.to_string_lossy().into_owned(),
        artifact_name: "Release".into(),
        artifact_version: version.into(),
        service_name: None,
        image_name: None,
        images: image_map.clone(),
        batch_size: 2,
        concurrency: 2,
    })
    .expect("plan");
    for (index, host) in config.hosts.iter().enumerate() {
        let rendered = render_release_templates(
            &env_template,
            &host_template,
            &compose,
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
        std::fs::write(&rendered_compose, rendered.compose_preview).expect("node compose");
        let session = connect_pinned(&config, host).await;
        let mac = &plan.target_macs[index];
        let staging = format!("/opt/data/.inxaiot-desk-buddy/{operation_id}/{mac}");
        let result = execute_connected_deployment(
            &session,
            &plan,
            &RemoteDeploymentFiles {
                local_agent: config.agent.clone(),
                local_artifact: archive.clone(),
                local_env: Some(env),
                local_host_info: Some(host_info),
                local_compose: Some(rendered_compose),
            },
            &RemoteDeploymentConfig {
                operation_id: operation_id.clone(),
                release_fingerprint: validation.fingerprint.clone().unwrap_or_default(),
                mac_normalized: mac.clone(),
                data_root: "/opt/data".into(),
                deploy_root: "/opt/data/deploy/inxvision-edge".into(),
                platform_api_host: config.platform_host.clone(),
                platform_api_port: config.platform_api_port.parse().expect("api port"),
                platform_mqtt_host: config.platform_host.clone(),
                platform_mqtt_port: config.platform_mqtt_port.parse().expect("mqtt port"),
                allow_existing_ports: true,
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
            vec![
                "-f".into(),
                "/opt/data/deploy/inxvision-edge/current/manifest.json".into(),
            ],
            BTreeMap::new(),
        )
        .await;
        session.disconnect().await.expect("disconnect");
        result.expect("full upgrade");
        assert_eq!(current.exit_status, 0);
    }
    let services = images
        .iter()
        .map(|image| image.service.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(services.len(), 4);
}
