use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use inxaiot_desk_buddy_lib::application::ports::project_management::{
    HostKeyManagementPort, ProjectManagementPort, ReleaseProfileManagementPort,
};
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::domain::aio::release_profile::{
    ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues,
};
use inxaiot_desk_buddy_lib::domain::common::project::{
    ConfirmHostKeyRequest, HostKeyCaptureRequest, HostKeyState, PlatformLoginRequest,
    ProjectConnectionState, ProjectConnectionTestRequest, ProjectInput, ProjectSessionState,
};
use inxaiot_desk_buddy_lib::formal::app_state::FormalAppState;
use inxaiot_desk_buddy_lib::formal::config::AppPaths;
use inxaiot_desk_buddy_lib::formal::job_supervisor::JobSupervisor;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::formal::runtime_registry::ProjectRuntimeRegistry;
use inxaiot_desk_buddy_lib::formal::secret_store::MemorySecretStore;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::{
    DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig,
};
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::TaskRepository;
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use inxaiot_desk_buddy_lib::infrastructure::stage75_adapter::Stage75Adapter;
use inxaiot_desk_buddy_lib::runtime::event_bus::TaskEventBus;
use inxaiot_desk_buddy_lib::runtime::task_queue::{TaskHandlerRegistry, TaskQueue};
use serde_json::Value;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use sqlx::{Executor, MySqlPool};
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

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = format!("${{{key}:");
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
        host: default(&yaml, "MYSQL_HOST").into(),
        port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
        username: default(&yaml, "MYSQL_USER").into(),
        password: default(&yaml, "MYSQL_PASSWORD").into(),
        schema: format!("inxaiot_desk_buddy_stage75int_{}", &suffix[..12]),
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
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75int_"));
    assert!(
        schema
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    pool.execute(
        format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci")
            .as_str(),
    )
    .await
    .expect("create integration schema");
}

async fn drop_schema(pool: &MySqlPool, schema: &str) {
    assert!(schema.starts_with("inxaiot_desk_buddy_stage75int_"));
    pool.execute(format!("DROP DATABASE IF EXISTS `{schema}`").as_str())
        .await
        .expect("drop integration schema");
}

fn project_input(config: &TestConfig) -> ProjectInput {
    ProjectInput {
        name: "阶段7.5-A真实集成项目".into(),
        platform_url: config.platform_url.clone(),
        db_host: config.host.clone(),
        db_port: config.port,
        db_user: config.username.clone(),
        db_tls_enabled: false,
        db_password: Some(config.password.clone()),
        business_db: config.platform_schema.clone(),
        workbench_db: config.schema.clone(),
    }
}

fn release_draft(config: &TestConfig, expected_version: Option<u64>) -> ReleaseProfileDraft {
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
            .expect("env template"),
            compose_template: std::fs::read_to_string(
                config.project_root.join("test/docker-compose.yml"),
            )
            .expect("compose template"),
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
            aio_mqtt_user: "stage75-aio".into(),
            aio_mqtt_password: "stage75-aio-password".into(),
            ssh_user: line(&config.description, "一体机ssh用户：").into(),
            ssh_password: None,
            ssh_private_key: Some(
                std::fs::read_to_string(config.project_root.join("test/id_rsa"))
                    .expect("ssh private key"),
            ),
        },
        expected_version,
    }
}

async fn platform_snapshot(pool: &MySqlPool) -> (i64, u64) {
    sqlx::query_as::<_, (i64, u64)>(
        "SELECT COUNT(*), CAST(COALESCE(SUM(CRC32(CONCAT_WS('|', id, name, ip, mac, status))), 0) AS UNSIGNED) \
         FROM op_edge_aio_server",
    )
    .fetch_one(pool)
    .await
    .expect("platform snapshot")
}

#[tokio::test]
#[ignore = "uses authorized databases/platform/two SSH nodes; creates and drops one random isolated workbench schema"]
async fn stage75a_real_adapter_project_session_profile_and_host_key_contract() {
    let config = config();
    println!("stage75a isolated schema: {}", config.schema);
    let admin = admin_pool(&config).await;
    create_schema(&admin, &config.schema).await;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let temp = tempfile::tempdir()?;
        let paths = AppPaths::from_data_dir(temp.path())?;
        paths.ensure()?;
        let local_store = LocalStore::open(&paths.local_db).await?;
        let task_event_bus = TaskEventBus::new(32)?;
        let task_repository = TaskRepository::new(local_store.pool().clone());
        let task_event_pipeline = TaskEventPipeline::new(
            task_repository.clone(),
            task_event_bus.clone(),
            SensitiveValueRedactor::default(),
        );
        let job_supervisor = JobSupervisor::default();
        let task_handler_registry = TaskHandlerRegistry::default();
        let task_queue =
            TaskQueue::start(8, 2, task_handler_registry.clone(), job_supervisor.clone()).await?;
        let state = FormalAppState {
            local_store,
            secret_store: Arc::new(MemorySecretStore::default()),
            runtime_registry: ProjectRuntimeRegistry::default(),
            job_supervisor,
            task_handler_registry,
            task_queue,
            task_event_bus,
            task_repository,
            task_event_pipeline,
            paths,
        };
        let adapter = Stage75Adapter::new(&state);
        assert!(adapter.list_projects().await?.is_empty());
        let project = adapter.create_project(project_input(&config)).await?;
        let project_id = project.project.id.clone();
        let connection = adapter
            .test_project_connection(ProjectConnectionTestRequest {
                existing_project_id: Some(project_id.clone()),
                project: ProjectInput {
                    db_password: None,
                    ..project_input(&config)
                },
            })
            .await?;
        assert!(connection.successful);
        assert_eq!(connection.workbench_schema_state, "uninitialized");
        let switched = adapter.switch_project(&project_id).await?;
        assert_eq!(
            switched.connection_state,
            ProjectConnectionState::SchemaRequired
        );

        let mysql = MySqlProjectConfig {
            host: config.host.clone(),
            port: config.port,
            username: config.username.clone(),
            password: SecretValue::new(config.password.clone()),
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
        let switched = adapter.switch_project(&project_id).await?;
        assert_eq!(
            switched.connection_state,
            ProjectConnectionState::LoginRequired
        );

        let session = adapter
            .login_project(
                &project_id,
                PlatformLoginRequest {
                    username: config.login["principal"]
                        .as_str()
                        .expect("principal")
                        .into(),
                    password: config.login["credentials"]
                        .as_str()
                        .expect("credentials")
                        .into(),
                    session_uuid: config.login["sessionUUID"]
                        .as_str()
                        .expect("session uuid")
                        .into(),
                    image_code: config.login["imageCode"]
                        .as_str()
                        .expect("image code")
                        .into(),
                },
            )
            .await?;
        assert_eq!(session.state, ProjectSessionState::Active);
        assert_eq!(
            adapter.check_project_session(&project_id).await?.state,
            ProjectSessionState::Active
        );

        let created = adapter
            .save_release_profile(&project_id, release_draft(&config, None))
            .await?;
        assert_eq!(created.version, 1);
        let updated = adapter
            .save_release_profile(&project_id, release_draft(&config, Some(1)))
            .await?;
        assert_eq!(updated.version, 2);
        let conflict = adapter
            .save_release_profile(&project_id, release_draft(&config, Some(1)))
            .await;
        assert!(matches!(
            conflict,
            Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict(_))
        ));

        for host in ["192.168.3.79", "192.168.3.121"] {
            let captured = adapter
                .capture_host_key(
                    &project_id,
                    HostKeyCaptureRequest {
                        host: host.into(),
                        port: Some(22),
                    },
                )
                .await?;
            assert_eq!(captured.state, HostKeyState::Unconfirmed);
            let confirmed = adapter
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
            assert_eq!(confirmed.state, HostKeyState::Confirmed);
        }
        assert_eq!(adapter.list_host_keys(&project_id).await?.len(), 2);
        assert_eq!(platform_snapshot(&pools.platform).await, before_platform);

        adapter.logout_project(&project_id).await?;
        assert_eq!(
            adapter.get_project_session(&project_id).await?.state,
            ProjectSessionState::Missing
        );
        adapter.delete_project(&project_id).await?;
        assert!(adapter.list_projects().await?.is_empty());
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
    .expect("verify integration schema cleanup");
    assert_eq!(remaining, 0);
    println!("stage75a isolated schema cleanup remaining: {remaining}");
    admin.close().await;
    if let Err(error) = result {
        panic!("stage 7.5-A real adapter gate failed: {error}");
    }
}
