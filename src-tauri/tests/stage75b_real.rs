use inxaiot_desk_buddy_lib::application::ports::deployment_workflow::{
    DeploymentPreflightPort, DeploymentSubmissionPort, DeploymentTaskQueryPort,
    OperationHistoryQueryPort,
};
use inxaiot_desk_buddy_lib::application::ports::project_management::{
    HostKeyManagementPort, ProjectManagementPort, ReleaseProfileManagementPort,
};
use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::domain::aio::deployment::{DeploymentMode, DeploymentPlanInput};
use inxaiot_desk_buddy_lib::domain::aio::deployment_workflow::{
    OperationHistoryQuery, PreflightStatus,
};
use inxaiot_desk_buddy_lib::domain::aio::release_profile::{
    ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues,
};
use inxaiot_desk_buddy_lib::domain::common::project::{
    ConfirmHostKeyRequest, HostKeyCaptureRequest, PlatformLoginRequest, ProjectInput,
};
use inxaiot_desk_buddy_lib::domain::common::task::{TargetState, TaskState};
use inxaiot_desk_buddy_lib::formal::app_state::FormalAppState;
use inxaiot_desk_buddy_lib::formal::config::AppPaths;
use inxaiot_desk_buddy_lib::formal::job_supervisor::JobSupervisor;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::formal::operation_repository::OperationRepository;
use inxaiot_desk_buddy_lib::formal::runtime_registry::ProjectRuntimeRegistry;
use inxaiot_desk_buddy_lib::formal::secret_store::MemorySecretStore;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::{
    DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig,
};
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::TaskRepository;
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::infrastructure::remote::RusshConnector;
use inxaiot_desk_buddy_lib::infrastructure::stage75_adapter::Stage75Adapter;
use inxaiot_desk_buddy_lib::infrastructure::stage75b_preflight_adapter::Stage75BPreflightAdapter;
use inxaiot_desk_buddy_lib::infrastructure::stage75b_query_adapter::Stage75BQueryAdapter;
use inxaiot_desk_buddy_lib::infrastructure::stage75b_submission_adapter::Stage75BSubmissionAdapter;
use inxaiot_desk_buddy_lib::infrastructure::task_handlers::{
    execute_aio_task, register_aio_task_handlers,
};
use inxaiot_desk_buddy_lib::infrastructure::workbench_aio::{
    ApplyInventoryWrite, InventoryAssetWrite, WorkbenchAioRepository,
};
use inxaiot_desk_buddy_lib::interface::commands::task_activity::request_task_cancel;
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};
use serde_json::Value;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::{Executor, MySqlPool};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

struct TestConfig {
    project_root: PathBuf,
    description: String,
    host: String,
    port: u16,
    username: String,
    password: String,
    platform_schema: String,
    platform_url: String,
    login: Value,
    schema: String,
}

#[derive(Clone)]
struct TestNode {
    name: String,
    ip: String,
    mac_normalized: String,
}

fn real_nodes() -> Vec<TestNode> {
    vec![
        TestNode {
            name: "stage75b-node-79".into(),
            ip: "192.168.3.79".into(),
            mac_normalized: "000C293BB933".into(),
        },
        TestNode {
            name: "stage75b-node-121".into(),
            ip: "192.168.3.121".into(),
            mac_normalized: "000C290B71F4".into(),
        },
    ]
}

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn config_default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = String::from("$") + "{" + key + ":";
    let rest = &text[text.find(&marker).expect("config default") + marker.len()..];
    &rest[..rest.find('}').expect("config default end")]
}

fn config() -> TestConfig {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project")
        .to_path_buf();
    let description =
        std::fs::read_to_string(project_root.join("test/测试数据说明.txt")).expect("test data");
    let start = description.find('{').expect("login json");
    let end = start + description[start..].find('}').expect("login json end") + 1;
    let login = serde_json::from_str(&description[start..end]).expect("login json parse");
    let workspace = project_root
        .parent()
        .and_then(Path::parent)
        .expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    let suffix = Uuid::now_v7().simple().to_string();
    TestConfig {
        project_root,
        platform_schema: line(&description, "平台业务数据库名：").into(),
        platform_url: format!("http://{}", line(&description, "平台API：")),
        host: config_default(&yaml, "MYSQL_HOST").into(),
        port: config_default(&yaml, "MYSQL_PORT")
            .parse()
            .expect("mysql port"),
        username: config_default(&yaml, "MYSQL_USER").into(),
        password: config_default(&yaml, "MYSQL_PASSWORD").into(),
        schema: format!("inxaiot_desk_buddy_stage75b_{}", &suffix[..12]),
        description,
        login,
    }
}

async fn admin_pool(config: &TestConfig) -> MySqlPool {
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .ssl_mode(MySqlSslMode::Disabled);
    MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("connect mysql admin")
}

async fn create_schema(pool: &MySqlPool, schema: &str) {
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75b_"));
    let tick = char::from(96);
    let sql = format!(
        "CREATE DATABASE {tick}{schema}{tick} CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    );
    pool.execute(sql.as_str()).await.expect("create schema");
}

async fn drop_schema(pool: &MySqlPool, schema: &str) {
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75b_"));
    let tick = char::from(96);
    let sql = format!("DROP DATABASE IF EXISTS {tick}{schema}{tick}");
    pool.execute(sql.as_str()).await.expect("drop schema");
}

fn project_input(config: &TestConfig, name: &str) -> ProjectInput {
    ProjectInput {
        name: name.into(),
        platform_url: config.platform_url.clone(),
        db_host: config.host.clone(),
        db_port: config.port,
        db_user: config.username.clone(),
        db_password: Some(config.password.clone()),
        business_db: config.platform_schema.clone(),
        workbench_db: config.schema.clone(),
    }
}

fn login_request(config: &TestConfig) -> PlatformLoginRequest {
    PlatformLoginRequest {
        username: config.login["principal"]
            .as_str()
            .expect("principal")
            .into(),
        password: config.login["credentials"]
            .as_str()
            .expect("credentials")
            .into(),
        session_uuid: config.login["sessionUUID"].as_str().expect("uuid").into(),
        image_code: config.login["imageCode"].as_str().expect("code").into(),
    }
}

fn release_draft(config: &TestConfig) -> ReleaseProfileDraft {
    let (api_host, api_port) = line(&config.description, "平台API：")
        .rsplit_once(':')
        .expect("api host port");
    let (mqtt_host, mqtt_port) = line(&config.description, "平台mqtt：")
        .rsplit_once(':')
        .expect("mqtt host port");
    ReleaseProfileDraft {
        values: ReleaseProfileValues {
            env_template: std::fs::read_to_string(
                config.project_root.join("test/templates/env.template"),
            )
            .expect("env"),
            compose_template: std::fs::read_to_string(
                config.project_root.join("test/docker-compose.yml"),
            )
            .expect("compose"),
            platform_host: api_host.into(),
            platform_api_port: api_port.parse().expect("api port"),
            platform_mqtt_host: mqtt_host.into(),
            platform_mqtt_port: mqtt_port.parse().expect("mqtt port"),
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy/inxvision-edge".into(),
        },
        credentials: ReleaseProfileCredentials {
            platform_auth_key: line(&config.description, "平台API auth Key：").into(),
            platform_mqtt_user: line(&config.description, "平台mqtt账号：").into(),
            platform_mqtt_password: line(&config.description, "平台mqtt密码：").into(),
            aio_mqtt_user: "stage75b-aio".into(),
            aio_mqtt_password: "stage75b-aio-password".into(),
            ssh_user: line(&config.description, "一体机ssh用户：").into(),
            ssh_password: None,
            ssh_private_key: Some(
                std::fs::read_to_string(config.project_root.join("test/id_rsa"))
                    .expect("private key"),
            ),
        },
        expected_version: None,
    }
}

async fn state_at(path: &Path) -> Arc<FormalAppState> {
    let paths = AppPaths::from_data_dir(path).expect("paths");
    paths.ensure().expect("ensure paths");
    let local_store = LocalStore::open(&paths.local_db)
        .await
        .expect("local store");
    let task_event_bus = TaskEventBus::new(2048).expect("event bus");
    let task_repository = TaskRepository::new(local_store.pool().clone());
    let job_supervisor = JobSupervisor::default();
    let task_handler_registry = TaskHandlerRegistry::default();
    let task_queue = TaskQueue::start(8, 2, task_handler_registry.clone(), job_supervisor.clone())
        .await
        .expect("task queue");
    let state = Arc::new(FormalAppState {
        local_store,
        secret_store: Arc::new(MemorySecretStore::default()),
        runtime_registry: ProjectRuntimeRegistry::default(),
        job_supervisor,
        task_handler_registry,
        task_queue,
        task_event_bus: task_event_bus.clone(),
        task_event_pipeline: TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus,
            SensitiveValueRedactor::default(),
        ),
        task_repository,
        paths,
    });
    let handler_state = state.clone();
    register_aio_task_handlers(
        &state.task_handler_registry,
        move |envelope, cancellation| {
            let state = handler_state.clone();
            async move { execute_aio_task(&state, envelope, cancellation).await }
        },
    )
    .expect("aio handlers");
    state
}

async fn platform_snapshot(pool: &MySqlPool) -> (i64, u64) {
    sqlx::query_as::<_, (i64, u64)>(
        "SELECT COUNT(*), CAST(COALESCE(SUM(CRC32(CONCAT_WS('|', id, name, ip, mac, status))), 0) AS UNSIGNED) FROM op_edge_aio_server",
    )
    .fetch_one(pool)
    .await
    .expect("platform snapshot")
}

fn deployment_input(config: &TestConfig, macs: Vec<String>) -> DeploymentPlanInput {
    DeploymentPlanInput {
        mode: DeploymentMode::ServiceUpgrade,
        target_macs: macs,
        artifact_path: config
            .project_root
            .join("test/images/device-edge-1.0.0.Alpha.20260819.tar")
            .to_string_lossy()
            .into_owned(),
        artifact_name: "device-edge".into(),
        artifact_version: "pending".into(),
        service_name: Some("device-edge".into()),
        image_name: None,
        images: BTreeMap::new(),
        batch_size: 2,
        concurrency: 2,
    }
}

async fn wait_for_running(state: &FormalAppState, task_id: &str) {
    for _ in 0..300 {
        if state
            .task_repository
            .get(task_id)
            .await
            .is_ok_and(|task| task.state == TaskState::Running)
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("task did not enter running state: {task_id}");
}

async fn wait_for_terminal(state: &FormalAppState, task_id: &str) {
    for _ in 0..6000 {
        if state
            .task_repository
            .get(task_id)
            .await
            .is_ok_and(|task| task.state.is_terminal())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("task did not reach terminal state: {task_id}");
}

#[tokio::test]
#[ignore = "uses authorized databases/platform/two SSH nodes; performs service upgrade, cancellation and exact isolated cleanup"]
async fn stage75b_real_preflight_async_progress_cancel_history_and_cleanup() {
    let config = config();
    println!("stage75b isolated schema: {}", config.schema);
    let admin = admin_pool(&config).await;
    create_schema(&admin, &config.schema).await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let temp = tempfile::tempdir()?;
        let state = state_at(temp.path()).await;
        let adapter = Stage75Adapter::new(&state);
        let project_id = adapter
            .create_project(project_input(&config, "阶段7.5-B真实集成项目"))
            .await?
            .project
            .id;
        adapter.switch_project(&project_id).await?;

        let mysql = MySqlProjectConfig {
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: inxaiot_desk_buddy_lib::core::secret::SecretValue::new(
                config.password.clone(),
            ),
            platform_schema: config.platform_schema.clone(),
            workbench_schema: config.schema.clone(),
            connect_timeout: Duration::from_secs(10),
            tls_mode: DatabaseTlsMode::Disabled,
        };
        let pools = DualMySqlPools::connect(&mysql).await?;
        let before_platform = platform_snapshot(&pools.platform).await;
        WorkbenchStore::new(pools.workbench.clone())
            .migrate()
            .await
            .map_err(|error| -> Box<dyn std::error::Error> { Box::new(error) })?;
        adapter.switch_project(&project_id).await?;
        adapter
            .login_project(&project_id, login_request(&config))
            .await?;
        adapter
            .save_release_profile(&project_id, release_draft(&config))
            .await?;

        let nodes = real_nodes();
        assert_eq!(nodes.len(), 2);
        WorkbenchAioRepository::new(pools.workbench.clone())
            .apply_inventory(ApplyInventoryWrite {
                file_name: "stage75b-real-gate.csv".into(),
                operator_name: "stage75b-test".into(),
                instance_id: "stage75b-test-instance".into(),
                classification_counts: serde_json::json!({"managed": 2}),
                assets: nodes
                    .iter()
                    .map(|node| InventoryAssetWrite {
                        mac_normalized: node.mac_normalized.clone(),
                        display_mac: node.mac_normalized.clone(),
                        name: node.name.clone(),
                        ip: node.ip.clone(),
                        building_id: None,
                        region_id: None,
                        addr_alias: None,
                        floor: None,
                        location: Some("stage75b-real-gate".into()),
                        remark: None,
                        platform_aio_id: None,
                        management_state: "managed".into(),
                        source: "platform".into(),
                        expected_version: None,
                    })
                    .collect(),
            })
            .await?;

        let mut confirmed = Vec::new();
        for node in &nodes {
            let captured = adapter
                .capture_host_key(
                    &project_id,
                    HostKeyCaptureRequest {
                        host: node.ip.clone(),
                        port: Some(22),
                    },
                )
                .await?;
            let observation = adapter
                .confirm_host_key(
                    &project_id,
                    ConfirmHostKeyRequest {
                        host: captured.host,
                        port: captured.port,
                        algorithm: captured.algorithm,
                        fingerprint: captured.fingerprint,
                        replace_changed: false,
                    },
                )
                .await?;
            confirmed.push((
                node.clone(),
                HostKeyIdentity {
                    algorithm: observation.algorithm,
                    fingerprint: observation.fingerprint,
                },
            ));
        }

        let input = deployment_input(
            &config,
            nodes
                .iter()
                .map(|node| node.mac_normalized.clone())
                .collect(),
        );
        let preflight = Stage75BPreflightAdapter::new(&state)
            .preflight(&project_id, &input)
            .await?;
        assert!(preflight.ready);
        let snapshot = preflight
            .execution_snapshot
            .as_ref()
            .expect("ready preflight execution snapshot");
        snapshot.validate(&project_id)?;
        assert_eq!(snapshot.targets.len(), 2);
        assert_eq!(snapshot.artifact_fingerprint.len(), 64);
        assert!(snapshot.targets.iter().all(|target| target.node.version > 0
            && !target.host_key_fingerprint.is_empty()
            && !target.host_key_accepted_at.is_empty()));
        assert_eq!(
            preflight
                .checks
                .iter()
                .filter(|check| check.code == "ssh_auth" && check.status == PreflightStatus::Passed)
                .count(),
            2
        );
        assert_eq!(
            preflight
                .checks
                .iter()
                .filter(|check| check.code == "docker" || check.code == "docker_compose")
                .filter(|check| check.status == PreflightStatus::Passed)
                .count(),
            4
        );

        let success_submission = Stage75BSubmissionAdapter::new(&state)
            .submit(&project_id, &input)
            .await?;
        assert_eq!(success_submission.state, "queued");
        let success_task_id = success_submission.task_id;
        wait_for_terminal(&state, &success_task_id).await;
        let success_task = Stage75BQueryAdapter::new(&state)
            .task(&project_id, &success_task_id)
            .await?;
        assert_eq!(success_task.state, "succeeded");
        assert_eq!(success_task.success_count, 2);
        assert_eq!(success_task.failure_count, 0);
        assert!(
            success_task
                .targets
                .iter()
                .all(|target| target.progress == 100)
        );
        let log_path = state.task_repository.get(&success_task_id).await?.log_path;
        let logs = tokio::fs::read_to_string(log_path).await?;
        for code in ["SFTP_UPLOAD_PROGRESS", "SSH_STDOUT", "AGENT_EVENT"] {
            assert!(logs.contains(code), "missing log code {code}");
        }
        for secret in [
            line(&config.description, "平台API auth Key："),
            line(&config.description, "平台mqtt密码："),
        ] {
            assert!(!logs.contains(secret));
        }

        let operation_id = success_task.operation_id.clone().expect("operation id");
        let history = Stage75BQueryAdapter::new(&state)
            .list_history(
                &project_id,
                &OperationHistoryQuery {
                    page: 1,
                    page_size: 20,
                    operation_type: Some("service_upgrade".into()),
                    state: None,
                },
            )
            .await?;
        assert!(history.items.iter().any(|item| item.id == operation_id));
        let detail = Stage75BQueryAdapter::new(&state)
            .history_detail(&project_id, &operation_id)
            .await?;
        assert_eq!(detail.targets.len(), 2);
        assert!(
            detail
                .targets
                .iter()
                .all(|target| target.state == "succeeded")
        );

        let cancel_submission = Stage75BSubmissionAdapter::new(&state)
            .submit(&project_id, &input)
            .await?;
        assert_eq!(cancel_submission.state, "queued");
        let cancel_task_id = cancel_submission.task_id;
        wait_for_running(&state, &cancel_task_id).await;
        request_task_cancel(&state, &cancel_task_id)
            .await
            .expect("cancel request");
        wait_for_terminal(&state, &cancel_task_id).await;
        let cancelled = state.task_repository.get(&cancel_task_id).await?;
        assert!(matches!(
            cancelled.state,
            TaskState::Cancelled | TaskState::PartiallySucceeded
        ));
        assert!(
            state
                .task_repository
                .targets(&cancel_task_id)
                .await?
                .iter()
                .any(|target| target.state == TargetState::Cancelled)
        );

        let cancelled_view = Stage75BQueryAdapter::new(&state)
            .task(&project_id, &cancel_task_id)
            .await?;
        let cancelled_operation_id = cancelled_view.operation_id.expect("cancel operation id");
        let private_key = std::fs::read_to_string(config.project_root.join("test/id_rsa"))?;
        for (node, identity) in confirmed {
            let session = RusshConnector::default()
                .connect(
                    &RemoteTarget {
                        host: node.ip,
                        port: 22,
                        connect_timeout: Duration::from_secs(15),
                    },
                    &RemoteAuth::PrivateKey {
                        username: line(&config.description, "一体机ssh用户：").into(),
                        private_key: SecretValue::new(private_key.clone()),
                        passphrase: None,
                    },
                    HostKeyPolicy::Require(identity),
                )
                .await?;
            for current_operation_id in [&operation_id, &cancelled_operation_id] {
                let staging = format!(
                    "/opt/data/.inxaiot-desk-buddy/{current_operation_id}/{}",
                    node.mac_normalized
                );
                let command = session
                    .run(
                        &ExecRequest {
                            program: "test".into(),
                            args: vec!["!".into(), "-e".into(), staging],
                            env: BTreeMap::new(),
                            stdin: None,
                            total_timeout: Duration::from_secs(30),
                            inactivity_timeout: Duration::from_secs(15),
                        },
                        &tokio_util::sync::CancellationToken::new(),
                        &NoopRemoteOutputSink,
                    )
                    .await?;
                assert_eq!(command.exit_status, 0);
            }
            session.disconnect().await?;
        }

        let second_temp = tempfile::tempdir()?;
        let second = state_at(second_temp.path()).await;
        let second_adapter = Stage75Adapter::new(&second);
        let second_project_id = second_adapter
            .create_project(project_input(&config, "阶段7.5-B第二实例"))
            .await?
            .project
            .id;
        second_adapter.switch_project(&second_project_id).await?;
        second_adapter
            .login_project(&second_project_id, login_request(&config))
            .await?;
        let shared_history = Stage75BQueryAdapter::new(&second)
            .list_history(
                &second_project_id,
                &OperationHistoryQuery {
                    page: 1,
                    page_size: 20,
                    operation_type: Some("service_upgrade".into()),
                    state: None,
                },
            )
            .await?;
        assert!(
            shared_history
                .items
                .iter()
                .any(|item| item.id == operation_id)
        );
        assert!(
            second
                .task_repository
                .list_recent(&second_project_id, 20)
                .await?
                .is_empty()
        );
        second.task_queue.shutdown(Duration::from_secs(1)).await;
        second.runtime_registry.close_all().await;
        second.local_store.close().await;

        assert_eq!(platform_snapshot(&pools.platform).await, before_platform);
        state.task_queue.shutdown(Duration::from_secs(1)).await;
        state.runtime_registry.close_all().await;
        pools.close().await;
        state.local_store.close().await;
        Ok(())
    }
    .await;
    drop_schema(&admin, &config.schema).await;
    let remaining = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
    )
    .bind(&config.schema)
    .fetch_one(&admin)
    .await
    .expect("verify schema cleanup");
    assert_eq!(remaining, 0);
    admin.close().await;
    result.expect("stage75b real gate");
}

#[tokio::test]
#[ignore = "drops only the explicitly named failed stage75b isolated schema"]
async fn cleanup_stage75b_schema() {
    let schema = std::env::var("INX_STAGE75B_SCHEMA").expect("INX_STAGE75B_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75b_"));
    let config = config();
    let admin = admin_pool(&config).await;
    drop_schema(&admin, &schema).await;
    let remaining = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
    )
    .bind(&schema)
    .fetch_one(&admin)
    .await
    .expect("verify cleanup");
    assert_eq!(remaining, 0);
    admin.close().await;
}

#[tokio::test]
#[ignore = "seeds only two authorized nodes into the explicitly named isolated UI schema"]
async fn seed_stage75b_ui_nodes() {
    let schema = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75_"));
    let config = config();
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&schema)
        .ssl_mode(MySqlSslMode::Disabled);
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("isolated workbench");
    WorkbenchAioRepository::new(pool.clone())
        .apply_inventory(ApplyInventoryWrite {
            file_name: "stage75b-ui-gate.csv".into(),
            operator_name: "stage75b-ui".into(),
            instance_id: "stage75b-ui-instance".into(),
            classification_counts: serde_json::json!({"managed": 2}),
            assets: real_nodes()
                .into_iter()
                .map(|node| InventoryAssetWrite {
                    display_mac: node.mac_normalized.clone(),
                    mac_normalized: node.mac_normalized,
                    name: node.name,
                    ip: node.ip,
                    building_id: None,
                    region_id: None,
                    addr_alias: None,
                    floor: None,
                    location: Some("stage75b-ui-gate".into()),
                    remark: None,
                    platform_aio_id: None,
                    management_state: "managed".into(),
                    source: "ui-gate".into(),
                    expected_version: None,
                })
                .collect(),
        })
        .await
        .expect("seed nodes");
    assert_eq!(
        WorkbenchAioRepository::new(pool.clone())
            .list_snapshots()
            .await
            .expect("nodes")
            .len(),
        2
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "read-only aggregate evidence from the explicitly named stage75b UI schema"]
async fn inspect_stage75b_ui_schema() {
    let schema = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75_"));
    let config = config();
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&schema)
        .ssl_mode(MySqlSslMode::Disabled);
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("isolated workbench");
    let service_operations = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_record WHERE operation_type = 'service_upgrade'",
    )
    .fetch_one(&pool)
    .await
    .expect("service operations");
    let succeeded = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_record WHERE operation_type = 'service_upgrade' AND state = 'succeeded'",
    )
    .fetch_one(&pool)
    .await
    .expect("succeeded operations");
    let cancelled_or_partial = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_record WHERE operation_type = 'service_upgrade' AND state IN ('cancelled', 'partially_succeeded')",
    )
    .fetch_one(&pool)
    .await
    .expect("cancelled operations");
    let terminal_targets = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result result JOIN operation_record operation ON operation.id = result.operation_id WHERE operation.operation_type = 'service_upgrade' AND result.result_state IN ('succeeded', 'failed', 'cancelled', 'interrupted', 'unknown')",
    )
    .fetch_one(&pool)
    .await
    .expect("terminal targets");
    let pending_targets = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result result JOIN operation_record operation ON operation.id = result.operation_id WHERE operation.operation_type = 'service_upgrade' AND result.result_state = 'pending'",
    )
    .fetch_one(&pool)
    .await
    .expect("pending targets");
    let active_leases = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM resource_lease WHERE lease_state = 'active' AND expires_at > UTC_TIMESTAMP(6)",
    )
    .fetch_one(&pool)
    .await
    .expect("active leases");
    let nodes = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM aio_node")
        .fetch_one(&pool)
        .await
        .expect("nodes");
    let profile_version =
        sqlx::query_scalar::<_, Option<u64>>("SELECT MAX(version) FROM aio_release_profile")
            .fetch_one(&pool)
            .await
            .expect("profile version")
            .unwrap_or_default();
    let migrations = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migrations");
    println!(
        "stage75b ui evidence: operations={service_operations}, succeeded={succeeded}, cancelled_or_partial={cancelled_or_partial}, terminal_targets={terminal_targets}, pending_targets={pending_targets}, active_leases={active_leases}, nodes={nodes}, profile_version={profile_version}, migrations={migrations}"
    );
    assert!(service_operations >= 2);
    assert!(succeeded >= 1);
    assert!(cancelled_or_partial >= 1);
    assert!(terminal_targets >= 4);
    assert_eq!(pending_targets, 0);
    assert_eq!(active_leases, 0);
    assert!(nodes >= 2);
    assert_eq!(profile_version, 2);
    assert_eq!(migrations, 2);
    pool.close().await;
}

#[tokio::test]
#[ignore = "read-only stage75c evidence: one executed operation and one queued cancellation without shared side effects"]
async fn inspect_stage75c_ui_schema() {
    let schema = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75_"));
    let config = config();
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&schema)
        .ssl_mode(MySqlSslMode::Disabled);
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("isolated workbench");
    let service_operations = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_record WHERE operation_type = 'service_upgrade'",
    )
    .fetch_one(&pool)
    .await
    .expect("operations");
    let succeeded = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_record WHERE operation_type = 'service_upgrade' AND state = 'succeeded'",
    )
    .fetch_one(&pool)
    .await
    .expect("succeeded");
    let terminal_targets = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result result JOIN operation_record operation ON operation.id = result.operation_id WHERE operation.operation_type = 'service_upgrade' AND result.result_state = 'succeeded'",
    )
    .fetch_one(&pool)
    .await
    .expect("targets");
    let pending_targets = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result result JOIN operation_record operation ON operation.id = result.operation_id WHERE operation.operation_type = 'service_upgrade' AND result.result_state = 'pending'",
    )
    .fetch_one(&pool)
    .await
    .expect("pending");
    let active_leases = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM resource_lease WHERE lease_state = 'active' AND expires_at > UTC_TIMESTAMP(6)",
    )
    .fetch_one(&pool)
    .await
    .expect("leases");
    let nodes = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM aio_node")
        .fetch_one(&pool)
        .await
        .expect("nodes");
    let profile_version =
        sqlx::query_scalar::<_, Option<u64>>("SELECT MAX(version) FROM aio_release_profile")
            .fetch_one(&pool)
            .await
            .expect("profile")
            .unwrap_or_default();
    let migrations = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _sqlx_migrations")
        .fetch_one(&pool)
        .await
        .expect("migrations");
    println!(
        "stage75c ui evidence: executed_operations={service_operations}, succeeded={succeeded}, succeeded_targets={terminal_targets}, pending_targets={pending_targets}, active_leases={active_leases}, nodes={nodes}, profile_version={profile_version}, migrations={migrations}"
    );
    assert_eq!(service_operations, 1);
    assert_eq!(succeeded, 1);
    assert_eq!(terminal_targets, 2);
    assert_eq!(pending_targets, 0);
    assert_eq!(active_leases, 0);
    assert_eq!(nodes, 2);
    assert_eq!(profile_version, 2);
    assert_eq!(migrations, 2);
    pool.close().await;
}

#[tokio::test]
#[ignore = "read-only SSH inspection of authorized stage75b nodes"]
async fn inspect_stage75b_node_interfaces() {
    let config = config();
    let private_key =
        std::fs::read_to_string(config.project_root.join("test/id_rsa")).expect("private key");
    for host in ["192.168.3.79", "192.168.3.121"] {
        let session = RusshConnector::default()
            .connect(
                &RemoteTarget {
                    host: host.into(),
                    port: 22,
                    connect_timeout: Duration::from_secs(15),
                },
                &RemoteAuth::PrivateKey {
                    username: line(&config.description, "一体机ssh用户：").into(),
                    private_key: SecretValue::new(private_key.clone()),
                    passphrase: None,
                },
                HostKeyPolicy::Capture,
            )
            .await
            .expect("ssh");
        let result = session
            .run(
                &ExecRequest {
                    program: "ip".into(),
                    args: vec!["-o".into(), "link".into(), "show".into()],
                    env: BTreeMap::new(),
                    stdin: None,
                    total_timeout: Duration::from_secs(30),
                    inactivity_timeout: Duration::from_secs(15),
                },
                &tokio_util::sync::CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await
            .expect("ip link");
        assert_eq!(result.exit_status, 0);
        println!("{host} interfaces: {}", result.stdout);
        session.disconnect().await.expect("disconnect");
    }
}

#[tokio::test]
#[ignore = "read-only verification of exact authorized stage75b remote staging paths"]
async fn inspect_stage75b_staging_absent() {
    let operation_id = std::env::var("INX_STAGE75_OPERATION_ID").expect("INX_STAGE75_OPERATION_ID");
    uuid::Uuid::parse_str(&operation_id).expect("operation UUID");
    let config = config();
    let private_key =
        std::fs::read_to_string(config.project_root.join("test/id_rsa")).expect("private key");
    for node in real_nodes() {
        let session = RusshConnector::default()
            .connect(
                &RemoteTarget {
                    host: node.ip,
                    port: 22,
                    connect_timeout: Duration::from_secs(15),
                },
                &RemoteAuth::PrivateKey {
                    username: line(&config.description, "一体机ssh用户：").into(),
                    private_key: SecretValue::new(private_key.clone()),
                    passphrase: None,
                },
                HostKeyPolicy::Capture,
            )
            .await
            .expect("ssh");
        let staging = format!(
            "/opt/data/.inxaiot-desk-buddy/{operation_id}/{}",
            node.mac_normalized
        );
        let result = session
            .run(
                &ExecRequest {
                    program: "test".into(),
                    args: vec!["!".into(), "-e".into(), staging],
                    env: BTreeMap::new(),
                    stdin: None,
                    total_timeout: Duration::from_secs(30),
                    inactivity_timeout: Duration::from_secs(15),
                },
                &tokio_util::sync::CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await
            .expect("staging check");
        assert_eq!(result.exit_status, 0);
        session.disconnect().await.expect("disconnect");
    }
}

#[tokio::test]
#[ignore = "removes only exact authorized stage75b staging paths for one operation UUID"]
async fn cleanup_exact_stage75b_staging() {
    let operation_id = std::env::var("INX_STAGE75_OPERATION_ID").expect("INX_STAGE75_OPERATION_ID");
    uuid::Uuid::parse_str(&operation_id).expect("operation UUID");
    let config = config();
    let private_key =
        std::fs::read_to_string(config.project_root.join("test/id_rsa")).expect("private key");
    for node in real_nodes() {
        let session = RusshConnector::default()
            .connect(
                &RemoteTarget {
                    host: node.ip,
                    port: 22,
                    connect_timeout: Duration::from_secs(15),
                },
                &RemoteAuth::PrivateKey {
                    username: line(&config.description, "一体机ssh用户：").into(),
                    private_key: SecretValue::new(private_key.clone()),
                    passphrase: None,
                },
                HostKeyPolicy::Capture,
            )
            .await
            .expect("ssh");
        let staging = format!(
            "/opt/data/.inxaiot-desk-buddy/{operation_id}/{}",
            node.mac_normalized
        );
        let removed = session
            .run(
                &ExecRequest {
                    program: "rm".into(),
                    args: vec!["-rf".into(), "--".into(), staging.clone()],
                    env: BTreeMap::new(),
                    stdin: None,
                    total_timeout: Duration::from_secs(60),
                    inactivity_timeout: Duration::from_secs(30),
                },
                &tokio_util::sync::CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await
            .expect("remove staging");
        assert_eq!(removed.exit_status, 0);
        let verified = session
            .run(
                &ExecRequest {
                    program: "test".into(),
                    args: vec!["!".into(), "-e".into(), staging],
                    env: BTreeMap::new(),
                    stdin: None,
                    total_timeout: Duration::from_secs(30),
                    inactivity_timeout: Duration::from_secs(15),
                },
                &tokio_util::sync::CancellationToken::new(),
                &NoopRemoteOutputSink,
            )
            .await
            .expect("verify staging");
        assert_eq!(verified.exit_status, 0);
        session.disconnect().await.expect("disconnect");
    }
}

#[tokio::test]
#[ignore = "read-only verification of one exact isolated operation finalization"]
async fn inspect_stage75_operation_finalized() {
    let schema = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75_"));
    let operation_id = std::env::var("INX_STAGE75_OPERATION_ID").expect("INX_STAGE75_OPERATION_ID");
    uuid::Uuid::parse_str(&operation_id).expect("operation UUID");
    let config = config();
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&schema)
        .ssl_mode(MySqlSslMode::Disabled);
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("isolated workbench");
    let operation_state =
        sqlx::query_scalar::<_, String>("SELECT state FROM operation_record WHERE id = ?")
            .bind(&operation_id)
            .fetch_one(&pool)
            .await
            .expect("operation state");
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ? AND result_state = 'pending'",
    )
    .bind(&operation_id)
    .fetch_one(&pool)
    .await
    .expect("pending targets");
    let active_leases = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM resource_lease WHERE operation_id = ? AND lease_state = 'active' AND expires_at > UTC_TIMESTAMP(6)",
    )
    .bind(&operation_id)
    .fetch_one(&pool)
    .await
    .expect("active leases");
    println!(
        "stage75 operation evidence: state={operation_state}, pending={pending}, active_leases={active_leases}"
    );
    assert!(matches!(
        operation_state.as_str(),
        "succeeded" | "partially_succeeded" | "failed" | "cancelled" | "interrupted"
    ));
    assert_eq!(pending, 0);
    assert_eq!(active_leases, 0);
    pool.close().await;
}

#[tokio::test]
#[ignore = "recovers only one exact stale operation in the explicitly named isolated schema"]
async fn recover_exact_stage75_operation() {
    let schema = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75_"));
    let operation_id = std::env::var("INX_STAGE75_OPERATION_ID").expect("INX_STAGE75_OPERATION_ID");
    uuid::Uuid::parse_str(&operation_id).expect("operation UUID");
    let config = config();
    let options = MySqlConnectOptions::new()
        .host(&config.host)
        .port(config.port)
        .username(&config.username)
        .password(&config.password)
        .database(&schema)
        .ssl_mode(MySqlSslMode::Disabled);
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .expect("isolated workbench");
    let repository = OperationRepository::new(pool.clone());
    let current = repository.get(&operation_id).await.expect("operation");
    let state = if current.state == "running" {
        repository
            .interrupt_stale(&operation_id, current.version, Duration::from_secs(1))
            .await
            .expect("recover stale operation")
            .state
    } else {
        current.state
    };
    assert_eq!(state, "interrupted");
    let pending = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM operation_target_result WHERE operation_id = ? AND result_state = 'pending'",
    )
    .bind(&operation_id)
    .fetch_one(&pool)
    .await
    .expect("pending");
    let active = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM resource_lease WHERE operation_id = ? AND lease_state = 'active' AND expires_at > UTC_TIMESTAMP(6)",
    )
    .bind(&operation_id)
    .fetch_one(&pool)
    .await
    .expect("leases");
    println!(
        "stage75 recovered operation: state={state}, pending={pending}, active_leases={active}"
    );
    assert_eq!(pending, 0);
    assert_eq!(active, 0);
    pool.close().await;
}
