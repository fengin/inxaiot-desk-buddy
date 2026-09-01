#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor, RemoteCommandResult,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::infrastructure::remote::{RemoteSession, RusshConnector};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct RemoteTestConfig {
    pub hosts: Vec<String>,
    pub user: String,
    pub private_key: String,
    pub platform_host: String,
    pub platform_api_port: String,
    pub platform_mqtt_port: String,
    pub agent: PathBuf,
}

pub fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .to_path_buf()
}

pub fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn split_host_port(value: &str) -> (&str, &str) {
    value.rsplit_once(':').expect("host:port")
}

pub fn config() -> RemoteTestConfig {
    let root = project_root();
    let description =
        std::fs::read_to_string(root.join("test/测试数据说明.txt")).expect("test data");
    let (platform_host, api_port) = split_host_port(line(&description, "平台API："));
    let (_, mqtt_port) = split_host_port(line(&description, "平台mqtt："));
    RemoteTestConfig {
        hosts: line(&description, "一体机IP:")
            .split('/')
            .map(str::trim)
            .map(str::to_string)
            .collect(),
        user: line(&description, "一体机ssh用户：").into(),
        private_key: std::fs::read_to_string(
            std::env::var_os("INX_TEST_SSH_PRIVATE_KEY_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| root.join("test/id_rsa")),
        )
        .expect("private key"),
        platform_host: platform_host.into(),
        platform_api_port: api_port.into(),
        platform_mqtt_port: mqtt_port.into(),
        agent: root.join("src-tauri/resources/agent/edge-node-agent.sh"),
    }
}

pub fn target(host: &str) -> RemoteTarget {
    RemoteTarget {
        host: host.into(),
        port: 22,
        connect_timeout: Duration::from_secs(10),
    }
}

pub fn auth(config: &RemoteTestConfig) -> RemoteAuth {
    RemoteAuth::PrivateKey {
        username: config.user.clone(),
        private_key: SecretValue::new(&config.private_key),
        passphrase: None,
    }
}

pub async fn connect_pinned(config: &RemoteTestConfig, host: &str) -> RemoteSession {
    let connector = RusshConnector::default();
    let target = target(host);
    let auth = auth(config);
    let discovery = connector
        .connect(&target, &auth, HostKeyPolicy::Capture)
        .await
        .expect("capture host key");
    let identity = discovery.host_key().clone();
    assert!(identity.fingerprint.starts_with("SHA256:"));
    discovery.disconnect().await.expect("disconnect discovery");
    connector
        .connect(&target, &auth, HostKeyPolicy::Require(identity))
        .await
        .expect("connect with pinned host key")
}

pub async fn run(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
    env: BTreeMap<String, String>,
) -> RemoteCommandResult {
    run_with_timeout(session, program, args, env, Duration::from_secs(60)).await
}

pub async fn run_with_timeout(
    session: &RemoteSession,
    program: &str,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    total_timeout: Duration,
) -> RemoteCommandResult {
    session
        .run(
            &ExecRequest {
                program: program.into(),
                args,
                env,
                stdin: None,
                total_timeout,
                inactivity_timeout: Duration::from_secs(30),
            },
            &CancellationToken::new(),
            &NoopRemoteOutputSink,
        )
        .await
        .expect("run remote command")
}

pub struct BuiltTestRelease {
    pub _temp: tempfile::TempDir,
    pub release_dir: PathBuf,
    pub archive: PathBuf,
    pub manifest: inxaiot_desk_buddy_lib::domain::aio::release::ReleaseManifest,
    pub env_template: String,
    pub host_template: String,
    pub compose: String,
    pub images: BTreeMap<String, String>,
}

pub fn build_test_release(version: &str) -> BuiltTestRelease {
    use inxaiot_desk_buddy_lib::domain::aio::release::{
        ReleaseImage, ReleaseManifest, ReleaseTemplates, inspect_image_archive,
        inspect_release_directory,
    };
    use inxaiot_desk_buddy_lib::infrastructure::release_archive::create_release_tar;

    let root = project_root();
    let temp = tempfile::tempdir().expect("temp release");
    let release = temp.path().join("release");
    std::fs::create_dir_all(release.join("images")).expect("images");
    std::fs::create_dir_all(release.join("templates")).expect("templates");
    for (source, destination) in [
        (
            root.join("test/docker-compose.yml"),
            release.join("docker-compose.yml"),
        ),
        (
            root.join("test/templates/env.template"),
            release.join("templates/env.template"),
        ),
        (
            root.join("test/templates/host-info.json.template"),
            release.join("templates/host-info.json.template"),
        ),
    ] {
        std::fs::copy(source, destination).expect("release template");
    }
    let image_files = [
        ("emqx", "emqx-5.10.0.tar"),
        ("device-edge", "device-edge-1.0.0.Alpha.20260819.tar"),
        ("rule-engine", "rule-engine-1.0.0.Alpha.20260810.tar"),
        (
            "device-edge-web",
            "device-edge-web-1.0.1.Alpha.20260812.tar",
        ),
    ];
    let mut manifest_images = Vec::new();
    let mut images = BTreeMap::new();
    for (service, file) in image_files {
        let source = root.join("test/images").join(file);
        let destination = release.join("images").join(file);
        std::fs::hard_link(&source, &destination)
            .or_else(|_| std::fs::copy(&source, &destination).map(|_| ()))
            .expect("release image");
        let info = inspect_image_archive(&destination).expect("inspect image");
        let tag = info
            .repo_tags
            .iter()
            .find(|tag| match service {
                "device-edge" => tag.contains("device-edge") && !tag.contains("web"),
                _ => tag.contains(service),
            })
            .cloned()
            .expect("service tag");
        images.insert(service.into(), tag.clone());
        manifest_images.push(ReleaseImage {
            service: service.into(),
            image: tag,
            archive: format!("images/{file}"),
        });
    }
    let manifest = ReleaseManifest {
        schema_version: 1,
        version: version.into(),
        compose_file: "docker-compose.yml".into(),
        images: manifest_images,
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
    let validation = inspect_release_directory(&release).expect("validate");
    assert!(validation.valid, "{:?}", validation.errors);
    let archive = create_release_tar(&release, &temp.path().join("release.tar")).expect("tar");
    BuiltTestRelease {
        env_template: std::fs::read_to_string(release.join("templates/env.template")).expect("env"),
        host_template: std::fs::read_to_string(release.join("templates/host-info.json.template"))
            .expect("host"),
        compose: std::fs::read_to_string(release.join("docker-compose.yml")).expect("compose"),
        _temp: temp,
        release_dir: release,
        archive,
        manifest,
        images,
    }
}
