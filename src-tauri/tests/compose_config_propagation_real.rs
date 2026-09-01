mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use common::{RemoteTestConfig, build_test_release, config, connect_pinned};
use inxaiot_desk_buddy_lib::application::deployment_executor::{
    DeploymentTargetState, execute_deployment_targets,
};
use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor, RemoteCommandResult,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::RemoteConnection;
use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{
    DeploymentMode, DeploymentPlan, DeploymentPlanInput,
};
use inxaiot_desk_buddy_lib::domain::aio::release::inspect_release_directory;
use inxaiot_desk_buddy_lib::domain::aio::release_render::ReleaseRenderContext;
use inxaiot_desk_buddy_lib::infrastructure::deployment_remote::{
    RemoteDeploymentConfig, RemoteDeploymentFiles, execute_connected_deployment,
};
use inxaiot_desk_buddy_lib::infrastructure::release_archive::create_release_tar;
use inxaiot_desk_buddy_lib::infrastructure::release_template::render_release_templates;
use inxaiot_desk_buddy_lib::infrastructure::remote::RemoteSession;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const DATA_ROOT: &str = "/opt/data";
const DEPLOY_ROOT: &str = "/opt/data/deploy/inxvision-edge";
const MARKER_LABEL: &str = "com.inxvision.p010.config-marker";
const CONTAINERS: &[&str] = &[
    "inx-edge-emqx",
    "inx-device-edge",
    "inx-rule-engine",
    "inx-device-edge-web",
];

#[derive(Debug)]
struct RemoteSnapshot {
    current_release: String,
    compose_sha256: String,
    host_info_sha256: String,
    marker: String,
    images: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
struct NodeCase {
    host: String,
    marker: String,
    env_path: PathBuf,
    host_info_path: PathBuf,
    compose_path: PathBuf,
    expected_compose_sha256: String,
}

#[derive(Clone)]
struct GateContext {
    remote: RemoteTestConfig,
    plan: DeploymentPlan,
    archive: PathBuf,
    release_fingerprint: String,
    version: String,
    operation_id: String,
    cases: BTreeMap<String, NodeCase>,
}

#[tokio::test]
#[ignore = "temporarily installs a unique full release on both authorized nodes, proves rendered Compose reaches Docker, then restores exact prior state"]
async fn rendered_compose_reaches_remote_file_and_container_then_restores_both_nodes()
-> Result<(), String> {
    let remote = config();
    if remote.hosts.len() < 2 {
        return Err("P0-10门禁要求至少两台授权节点".into());
    }

    let unique = Uuid::now_v7().simple().to_string();
    let version = format!("p010-compose-{unique}");
    let operation_id = format!("p010-compose-{unique}");
    println!("P010_COMPOSE_GATE_START|version={version}|operation={operation_id}");
    let built = build_test_release(&version);
    let env_template = format!(
        "{}\nP010_COMPOSE_MARKER={{{{node.name}}}}\n",
        built.env_template.trim_end()
    );
    let compose_template = built.compose.replacen(
        "    container_name: inx-device-edge\n",
        &format!(
            "    container_name: inx-device-edge\n    labels:\n      {MARKER_LABEL}: \"${{P010_COMPOSE_MARKER}}\"\n"
        ),
        1,
    );
    if compose_template == built.compose {
        return Err("测试Compose未找到device-edge插入位置".into());
    }
    std::fs::write(
        built.release_dir.join("templates/env.template"),
        &env_template,
    )
    .map_err(|error| format!("写入测试env模板失败：{error}"))?;
    std::fs::write(
        built.release_dir.join("docker-compose.yml"),
        &compose_template,
    )
    .map_err(|error| format!("写入测试Compose模板失败：{error}"))?;

    let validation = inspect_release_directory(&built.release_dir)
        .map_err(|error| format!("校验测试Release失败：{error}"))?;
    if !validation.valid {
        return Err(format!("测试Release无效：{:?}", validation.errors));
    }
    let release_fingerprint = validation
        .fingerprint
        .ok_or_else(|| "测试Release缺少指纹".to_string())?;
    let archive = create_release_tar(
        &built.release_dir,
        &built._temp.path().join("p010-release.tar"),
    )
    .map_err(|error| format!("创建测试Release归档失败：{error}"))?;
    let plan = DeploymentPlan::build(DeploymentPlanInput {
        mode: DeploymentMode::FullUpgrade,
        target_macs: remote
            .hosts
            .iter()
            .enumerate()
            .map(|(index, _)| format!("02000000a0{index:02}"))
            .collect(),
        artifact_path: built.release_dir.to_string_lossy().into_owned(),
        artifact_name: "Release".into(),
        artifact_version: version.clone(),
        service_name: None,
        image_name: None,
        images: built.images.clone(),
        batch_size: 2,
        concurrency: 2,
    })
    .map_err(|error| format!("构建P0-10部署计划失败：{error}"))?;

    let mut cases = BTreeMap::new();
    for (index, host) in remote.hosts.iter().take(2).enumerate() {
        let marker = format!("p010-{unique}-node-{index}");
        let rendered = render_release_templates(
            &env_template,
            &built.host_template,
            &compose_template,
            &ReleaseRenderContext {
                release_version: version.clone(),
                platform_host: remote.platform_host.clone(),
                platform_api_port: remote
                    .platform_api_port
                    .parse()
                    .map_err(|error| format!("平台API端口无效：{error}"))?,
                platform_mqtt_host: remote.platform_host.clone(),
                platform_mqtt_port: remote
                    .platform_mqtt_port
                    .parse()
                    .map_err(|error| format!("平台MQTT端口无效：{error}"))?,
                platform_mqtt_user: "edge_platform".into(),
                platform_mqtt_password: "p010-isolated".into(),
                local_mqtt_user: "aio_local".into(),
                local_mqtt_password: "p010-isolated".into(),
                auth_key: "p010-isolated".into(),
                node_name: marker.clone(),
                node_ip: host.clone(),
                node_mac: plan.target_macs[index].clone(),
                images: built.images.clone(),
                ..ReleaseRenderContext::default()
            },
        )
        .map_err(|error| format!("{host}渲染失败：{error}"))?;
        if !rendered.compose_preview.contains(&marker)
            || rendered.compose_preview.contains("${P010_COMPOSE_MARKER}")
        {
            return Err(format!("{host}渲染后的Compose没有唯一标记"));
        }
        let node_dir = built._temp.path().join(format!("p010-node-{index}"));
        std::fs::create_dir_all(&node_dir)
            .map_err(|error| format!("创建节点临时目录失败：{error}"))?;
        let env_path = node_dir.join(".env");
        let host_info_path = node_dir.join("host-info.json");
        let compose_path = node_dir.join("docker-compose.yml");
        std::fs::write(&env_path, rendered.env)
            .map_err(|error| format!("写入节点env失败：{error}"))?;
        std::fs::write(&host_info_path, rendered.host_info_json)
            .map_err(|error| format!("写入节点host-info失败：{error}"))?;
        std::fs::write(&compose_path, rendered.compose_preview)
            .map_err(|error| format!("写入节点Compose失败：{error}"))?;
        let expected_compose_sha256 = sha256_file(&compose_path)?;
        cases.insert(
            plan.target_macs[index].clone(),
            NodeCase {
                host: host.clone(),
                marker,
                env_path,
                host_info_path,
                compose_path,
                expected_compose_sha256,
            },
        );
    }

    let context = Arc::new(GateContext {
        remote,
        plan: plan.clone(),
        archive,
        release_fingerprint,
        version,
        operation_id,
        cases,
    });
    let summary = execute_deployment_targets(plan, CancellationToken::new(), {
        let context = context.clone();
        move |mac, _cancellation| {
            let context = context.clone();
            async move { run_node(&context, &mac).await.map_err(AppError::Conflict) }
        }
    })
    .await
    .map_err(|error| format!("P0-10生产批执行器失败：{error}"))?;
    if summary.success_count != 2
        || summary.failure_count != 0
        || summary.cancelled_count != 0
        || summary
            .targets
            .iter()
            .any(|target| target.state != DeploymentTargetState::Succeeded)
    {
        return Err(format!("P0-10并发目标未全部成功：{:?}", summary.targets));
    }
    Ok(())
}

async fn run_node(context: &GateContext, mac: &str) -> Result<(), String> {
    let node = context
        .cases
        .get(mac)
        .ok_or_else(|| format!("缺少节点测试上下文：{mac}"))?;
    let index = context
        .plan
        .target_macs
        .iter()
        .position(|candidate| candidate == mac)
        .ok_or_else(|| format!("部署计划缺少目标：{mac}"))?;
    let session = connect_pinned(&context.remote, &node.host).await;
    let snapshot = snapshot(&session).await?;
    let test_release = format!("{DEPLOY_ROOT}/releases/{}", context.version);
    let staging = format!(
        "{DATA_ROOT}/.inxaiot-desk-buddy/{}/{mac}",
        context.operation_id
    );
    let host_info_backup = format!(
        "{DATA_ROOT}/config/.p010-host-info-{}-{index}.json",
        context.operation_id
    );
    ensure_absent(&session, &test_release, "测试Release").await?;
    ensure_absent(&session, &staging, "测试staging").await?;
    command_ok(
        &session,
        "cp",
        vec![
            "--".into(),
            format!("{DATA_ROOT}/config/host-info.json"),
            host_info_backup.clone(),
        ],
        "备份host-info",
    )
    .await?;

    let deployment = execute_connected_deployment(
        &session,
        &context.plan,
        &RemoteDeploymentFiles {
            local_agent: context.remote.agent.clone(),
            local_artifact: context.archive.clone(),
            local_env: Some(node.env_path.clone()),
            local_host_info: Some(node.host_info_path.clone()),
            local_compose: Some(node.compose_path.clone()),
        },
        &RemoteDeploymentConfig {
            operation_id: context.operation_id.clone(),
            release_fingerprint: context.release_fingerprint.clone(),
            mac_normalized: mac.into(),
            data_root: DATA_ROOT.into(),
            deploy_root: DEPLOY_ROOT.into(),
            platform_api_host: context.remote.platform_host.clone(),
            platform_api_port: context
                .remote
                .platform_api_port
                .parse()
                .map_err(|error| format!("平台API端口无效：{error}"))?,
            platform_mqtt_host: context.remote.platform_host.clone(),
            platform_mqtt_port: context
                .remote
                .platform_mqtt_port
                .parse()
                .map_err(|error| format!("平台MQTT端口无效：{error}"))?,
            allow_existing_ports: true,
        },
        &CancellationToken::new(),
    )
    .await
    .map_err(|error| format!("{}生产部署链失败：{error}", node.host));

    let verification = if deployment.is_ok() {
        verify_propagation(
            &session,
            &test_release,
            &node.expected_compose_sha256,
            &node.marker,
        )
        .await
    } else {
        Ok(())
    };
    let cleanup = restore(
        &session,
        &snapshot,
        &context.version,
        &test_release,
        &staging,
        &host_info_backup,
    )
    .await;
    let disconnect = session
        .disconnect()
        .await
        .map_err(|error| format!("{}断开SSH失败：{error}", node.host));

    cleanup.map_err(|error| format!("{}恢复失败：{error}", node.host))?;
    disconnect?;
    deployment?;
    verification.map_err(|error| format!("{}传播验证失败：{error}", node.host))?;
    println!(
        "P010_COMPOSE_PROPAGATION_PASS|host={}|marker={}|compose_sha256={}",
        node.host, node.marker, node.expected_compose_sha256
    );
    Ok(())
}

async fn snapshot(session: &RemoteSession) -> Result<RemoteSnapshot, String> {
    let current_release = command_text(
        session,
        "readlink",
        vec!["-f".into(), format!("{DEPLOY_ROOT}/current")],
        "读取current",
    )
    .await?;
    validate_original_release(&current_release)?;
    let compose_sha256 = remote_sha256(
        session,
        &format!("{current_release}/docker-compose.yml"),
        "读取原Compose哈希",
    )
    .await?;
    let host_info_sha256 = remote_sha256(
        session,
        &format!("{DATA_ROOT}/config/host-info.json"),
        "读取原host-info哈希",
    )
    .await?;
    let marker = container_marker(session).await?;
    let mut images = BTreeMap::new();
    for container in CONTAINERS {
        let state = command_text(
            session,
            "docker",
            vec![
                "inspect".into(),
                "-f".into(),
                "{{.State.Status}}|{{.Config.Image}}".into(),
                (*container).into(),
            ],
            "读取原容器状态",
        )
        .await?;
        let (status, image) = state
            .split_once('|')
            .ok_or_else(|| format!("{container}原状态格式无效"))?;
        if status != "running" || image.is_empty() {
            return Err(format!("{container}验收前不是running或镜像为空"));
        }
        images.insert((*container).into(), image.into());
    }
    Ok(RemoteSnapshot {
        current_release,
        compose_sha256,
        host_info_sha256,
        marker,
        images,
    })
}

async fn verify_propagation(
    session: &RemoteSession,
    test_release: &str,
    expected_compose_sha256: &str,
    marker: &str,
) -> Result<(), String> {
    let current = command_text(
        session,
        "readlink",
        vec!["-f".into(), format!("{DEPLOY_ROOT}/current")],
        "验证测试current",
    )
    .await?;
    if current != test_release {
        return Err(format!("current未指向测试Release：{current}"));
    }
    let actual_sha256 = remote_sha256(
        session,
        &format!("{test_release}/docker-compose.yml"),
        "验证远端Compose哈希",
    )
    .await?;
    if actual_sha256 != expected_compose_sha256 {
        return Err(format!(
            "远端Compose哈希不一致：expected={expected_compose_sha256}; actual={actual_sha256}"
        ));
    }
    let actual_marker = container_marker(session).await?;
    if actual_marker != marker {
        return Err(format!(
            "容器Compose label未传播：expected={marker}; actual={actual_marker}"
        ));
    }
    verify_containers_running(session, None).await
}

async fn restore(
    session: &RemoteSession,
    snapshot: &RemoteSnapshot,
    version: &str,
    test_release: &str,
    staging: &str,
    host_info_backup: &str,
) -> Result<(), String> {
    let _ = execute(
        session,
        "docker",
        vec![
            "compose".into(),
            "-f".into(),
            format!("{test_release}/docker-compose.yml"),
            "down".into(),
            "--remove-orphans".into(),
        ],
    )
    .await;
    command_ok(
        session,
        "cp",
        vec![
            "--".into(),
            host_info_backup.into(),
            format!("{DATA_ROOT}/config/host-info.json"),
        ],
        "恢复host-info",
    )
    .await?;
    command_ok(
        session,
        "ln",
        vec![
            "-sfn".into(),
            snapshot.current_release.clone(),
            format!("{DEPLOY_ROOT}/current"),
        ],
        "恢复current",
    )
    .await?;
    command_ok(
        session,
        "docker",
        vec![
            "compose".into(),
            "-f".into(),
            format!("{}/docker-compose.yml", snapshot.current_release),
            "up".into(),
            "-d".into(),
            "--force-recreate".into(),
            "--remove-orphans".into(),
        ],
        "恢复原Compose",
    )
    .await?;
    command_ok(
        session,
        "rm",
        vec![
            "-rf".into(),
            "--".into(),
            test_release.into(),
            staging.into(),
        ],
        "清理测试Release与staging",
    )
    .await?;
    let operation_staging = staging
        .rsplit_once('/')
        .map(|(parent, _)| parent)
        .ok_or_else(|| "测试staging缺少操作父目录".to_string())?;
    let _ = execute(
        session,
        "rmdir",
        vec!["--".into(), operation_staging.into()],
    )
    .await;
    command_ok(
        session,
        "rm",
        vec!["-f".into(), "--".into(), host_info_backup.into()],
        "清理host-info备份",
    )
    .await?;
    cleanup_backup_directories(session, version).await?;

    let current = command_text(
        session,
        "readlink",
        vec!["-f".into(), format!("{DEPLOY_ROOT}/current")],
        "复核current恢复",
    )
    .await?;
    if current != snapshot.current_release {
        return Err(format!("current恢复不一致：{current}"));
    }
    let compose_sha256 = remote_sha256(
        session,
        &format!("{}/docker-compose.yml", snapshot.current_release),
        "复核原Compose哈希",
    )
    .await?;
    if compose_sha256 != snapshot.compose_sha256 {
        return Err("原Compose哈希恢复不一致".into());
    }
    let host_info_sha256 = remote_sha256(
        session,
        &format!("{DATA_ROOT}/config/host-info.json"),
        "复核原host-info哈希",
    )
    .await?;
    if host_info_sha256 != snapshot.host_info_sha256 {
        return Err("原host-info哈希恢复不一致".into());
    }
    let marker = container_marker(session).await?;
    if marker != snapshot.marker {
        return Err(format!(
            "原容器label恢复不一致：expected={}; actual={marker}",
            snapshot.marker
        ));
    }
    verify_containers_running(session, Some(&snapshot.images)).await?;
    ensure_absent(session, test_release, "测试Release").await?;
    ensure_absent(session, staging, "测试staging").await?;
    ensure_absent(session, operation_staging, "测试操作staging").await?;
    ensure_absent(session, host_info_backup, "host-info备份").await
}

async fn cleanup_backup_directories(session: &RemoteSession, version: &str) -> Result<(), String> {
    let result = execute(
        session,
        "find",
        vec![
            format!("{DATA_ROOT}/backup"),
            "-mindepth".into(),
            "1".into(),
            "-maxdepth".into(),
            "1".into(),
            "-type".into(),
            "d".into(),
            "-name".into(),
            format!("{version}-before-upgrade-*"),
            "-print".into(),
        ],
    )
    .await?;
    if result.exit_status != 0 {
        return Err(format!("枚举测试备份目录失败：exit={}", result.exit_status));
    }
    let prefix = format!("{DATA_ROOT}/backup/{version}-before-upgrade-");
    for path in result
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if !path.starts_with(&prefix)
            || path[prefix.len()..].is_empty()
            || path.contains("/../")
            || path.contains('\0')
        {
            return Err(format!("拒绝清理非本次备份目录：{path}"));
        }
        command_ok(
            session,
            "rm",
            vec!["-rf".into(), "--".into(), path.into()],
            "清理测试备份目录",
        )
        .await?;
    }
    let remaining = execute(
        session,
        "find",
        vec![
            format!("{DATA_ROOT}/backup"),
            "-mindepth".into(),
            "1".into(),
            "-maxdepth".into(),
            "1".into(),
            "-type".into(),
            "d".into(),
            "-name".into(),
            format!("{version}-before-upgrade-*"),
            "-print".into(),
        ],
    )
    .await?;
    if remaining.exit_status != 0 || !remaining.stdout.trim().is_empty() {
        return Err("测试备份目录仍有残留".into());
    }
    Ok(())
}

async fn verify_containers_running(
    session: &RemoteSession,
    expected_images: Option<&BTreeMap<String, String>>,
) -> Result<(), String> {
    for container in CONTAINERS {
        let state = command_text(
            session,
            "docker",
            vec![
                "inspect".into(),
                "-f".into(),
                "{{.State.Status}}|{{.Config.Image}}|{{.RestartCount}}".into(),
                (*container).into(),
            ],
            "验证容器状态",
        )
        .await?;
        let values = state.split('|').collect::<Vec<_>>();
        if values.len() != 3 || values[0] != "running" || values[2] != "0" {
            return Err(format!(
                "{container}未恢复为running/RestartCount=0：{state}"
            ));
        }
        if let Some(images) = expected_images
            && images.get(*container).map(String::as_str) != Some(values[1])
        {
            return Err(format!("{container}镜像恢复不一致：{}", values[1]));
        }
    }
    Ok(())
}

async fn container_marker(session: &RemoteSession) -> Result<String, String> {
    let labels = command_text(
        session,
        "docker",
        vec![
            "inspect".into(),
            "-f".into(),
            "{{json .Config.Labels}}".into(),
            "inx-device-edge".into(),
        ],
        "读取device-edge容器label",
    )
    .await?;
    let value: serde_json::Value =
        serde_json::from_str(&labels).map_err(|error| format!("容器label JSON无效：{error}"))?;
    Ok(value
        .get(MARKER_LABEL)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string())
}

async fn ensure_absent(session: &RemoteSession, path: &str, label: &str) -> Result<(), String> {
    let result = execute(session, "test", vec!["!".into(), "-e".into(), path.into()]).await?;
    if result.exit_status != 0 {
        return Err(format!("{label}仍存在：{path}"));
    }
    Ok(())
}

async fn remote_sha256(session: &RemoteSession, path: &str, stage: &str) -> Result<String, String> {
    let output = command_text(session, "sha256sum", vec!["--".into(), path.into()], stage).await?;
    let hash = output
        .split_whitespace()
        .next()
        .ok_or_else(|| format!("{stage}没有返回哈希"))?
        .to_ascii_lowercase();
    if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("{stage}返回非法SHA-256"));
    }
    Ok(hash)
}

async fn command_text(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
    stage: &str,
) -> Result<String, String> {
    let result = execute(session, program, args).await?;
    if result.exit_status != 0 {
        return Err(format!("{stage}失败：exit={}", result.exit_status));
    }
    Ok(result.stdout.trim().to_string())
}

async fn command_ok(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
    stage: &str,
) -> Result<(), String> {
    command_text(session, program, args, stage)
        .await
        .map(|_| ())
}

async fn execute(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
) -> Result<RemoteCommandResult, String> {
    session
        .run(
            &ExecRequest {
                program: program.into(),
                args,
                env: BTreeMap::new(),
                stdin: None,
                total_timeout: Duration::from_secs(180),
                inactivity_timeout: Duration::from_secs(90),
            },
            &CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await
        .map_err(|error| format!("远端命令执行失败：{error}"))
}

fn validate_original_release(path: &str) -> Result<(), String> {
    let prefix = format!("{DEPLOY_ROOT}/releases/");
    if !path.starts_with(&prefix)
        || path[prefix.len()..].is_empty()
        || path.contains('\0')
        || path.split('/').any(|segment| segment == "..")
    {
        return Err(format!("原current路径不在固定Release目录：{path}"));
    }
    Ok(())
}

fn sha256_file(path: &std::path::Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|error| format!("读取Compose失败：{error}"))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}
