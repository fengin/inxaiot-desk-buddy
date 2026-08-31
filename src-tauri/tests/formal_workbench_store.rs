use std::path::{Path, PathBuf};
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::credential_crypto::{self, ReleaseCredentials};
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::release_profile_repository::{
    ReleaseProfileRepository, ReleaseProfileValues, ReleaseProfileWrite,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use sqlx::Row;
use uuid::Uuid;

struct TestConfig {
    mysql: MySqlConnectionSpec,
    project_root: PathBuf,
    test_description: String,
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

fn split_host_port(value: &str) -> (&str, u16) {
    let (host, port) = value.rsplit_once(':').expect("host:port");
    (host, port.parse().expect("port"))
}

fn config() -> TestConfig {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project")
        .to_path_buf();
    let test_description =
        std::fs::read_to_string(project_root.join("test/测试数据说明.txt")).expect("test data");
    let workspace = project_root
        .parent()
        .and_then(Path::parent)
        .expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    TestConfig {
        mysql: MySqlConnectionSpec {
            host: default(&yaml, "MYSQL_HOST").into(),
            port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
            username: default(&yaml, "MYSQL_USER").into(),
            password: default(&yaml, "MYSQL_PASSWORD").into(),
            platform_schema: line(&test_description, "平台业务数据库名：").into(),
            workbench_schema: line(&test_description, "工作台数据库：").into(),
            tls_mode: MySqlTlsMode::Disabled,
            connect_timeout: Duration::from_secs(10),
        },
        project_root,
        test_description,
    }
}

fn release_values(config: &TestConfig) -> (ReleaseProfileValues, ReleaseCredentials) {
    let (api_host, api_port) = split_host_port(line(&config.test_description, "平台API："));
    let (mqtt_host, mqtt_port) = split_host_port(line(&config.test_description, "平台mqtt："));
    (
        ReleaseProfileValues {
            env_template: std::fs::read_to_string(
                config.project_root.join("test/templates/env.template"),
            )
            .expect("env template"),
            compose_template: std::fs::read_to_string(
                config.project_root.join("test/docker-compose.yml"),
            )
            .expect("compose template"),
            platform_host: api_host.into(),
            platform_api_port: api_port,
            platform_mqtt_host: mqtt_host.into(),
            platform_mqtt_port: mqtt_port,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy/inxvision-edge".into(),
        },
        ReleaseCredentials {
            platform_auth_key: line(&config.test_description, "平台API auth Key：").into(),
            platform_mqtt_user: line(&config.test_description, "平台mqtt账号：").into(),
            platform_mqtt_password: line(&config.test_description, "平台mqtt密码：").into(),
            aio_mqtt_user: "inxvision-local".into(),
            aio_mqtt_password: "inxvision-local-test".into(),
            ssh_user: line(&config.test_description, "一体机ssh用户：").into(),
            ssh_password: None,
            ssh_private_key: Some(
                std::fs::read_to_string(config.project_root.join("test/id_rsa"))
                    .expect("ssh private key"),
            ),
        },
    )
}

#[test]
fn encrypted_release_credentials_round_trip_and_wrong_key_rejection() {
    let config = config();
    let (_, credentials) = release_values(&config);
    let envelope =
        credential_crypto::encrypt_release_credentials(&config.mysql.password, &credentials)
            .expect("encrypt credentials");
    let decrypted =
        credential_crypto::decrypt_release_credentials(&config.mysql.password, &envelope)
            .expect("decrypt credentials");
    assert_eq!(decrypted, credentials);
    assert!(credential_crypto::decrypt_release_credentials("wrong-password", &envelope).is_err());
}

#[tokio::test]
#[ignore = "runs SQLx migration in the authorized workbench database"]
async fn workbench_schema_matches_documented_data_boundary() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config.mysql)
        .await
        .expect("connect project mysql");
    let store = WorkbenchStore::new(pools.workbench().clone());
    store.migrate().await.expect("workbench migration");
    let audit = store
        .audit_schema(&config.mysql.workbench_schema)
        .await
        .expect("schema audit");
    let status = store
        .schema_status(&config.mysql.workbench_schema)
        .await
        .expect("workbench schema status");
    assert!(status.is_ready());
    assert_eq!(
        status.current_version,
        Some(status.latest_available_version)
    );
    assert!(
        audit.missing_tables.is_empty(),
        "{:?}",
        audit.missing_tables
    );
    assert!(
        audit.forbidden_tables.is_empty(),
        "{:?}",
        audit.forbidden_tables
    );
    assert_eq!(audit.business_tables.len(), 7);
    assert!(audit.migration_count >= 1);
    assert!(audit.release_credentials_are_encrypted);
    pools.close().await;
}

#[tokio::test]
#[ignore = "writes and removes one isolated release profile in the authorized workbench database"]
async fn release_profile_uses_optimistic_version_and_encrypted_credentials() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config.mysql)
        .await
        .expect("connect project mysql");
    let store = WorkbenchStore::new(pools.workbench().clone());
    store.migrate().await.expect("workbench migration");
    let repository = ReleaseProfileRepository::new(pools.workbench().clone());
    let profile_key = format!("poc-{}", &Uuid::now_v7().simple().to_string()[..20]);
    let (values, credentials) = release_values(&config);
    let first = repository
        .save(
            &config.mysql.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values: values.clone(),
                credentials: credentials.clone(),
                expected_version: None,
                operator_name: "phase3-test".into(),
                instance_id: "instance-a".into(),
            },
        )
        .await
        .expect("create release profile");
    assert_eq!(first.version, 1);
    assert_eq!(first.credentials, credentials);

    let stale = repository
        .save(
            &config.mysql.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values: values.clone(),
                credentials: credentials.clone(),
                expected_version: Some(0),
                operator_name: "phase3-test".into(),
                instance_id: "instance-b".into(),
            },
        )
        .await;
    assert!(matches!(stale, Err(FormalError::Conflict(_))));

    let second = repository
        .save(
            &config.mysql.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values,
                credentials: credentials.clone(),
                expected_version: Some(1),
                operator_name: "phase3-test".into(),
                instance_id: "instance-b".into(),
            },
        )
        .await
        .expect("update release profile");
    assert_eq!(second.version, 2);

    let row =
        sqlx::query("SELECT credential_ciphertext FROM aio_release_profile WHERE profile_key = ?")
            .bind(&profile_key)
            .fetch_one(pools.workbench())
            .await
            .expect("read release ciphertext");
    let ciphertext: Vec<u8> = row.try_get("credential_ciphertext").expect("ciphertext");
    let ciphertext_text = String::from_utf8_lossy(&ciphertext);
    assert!(!ciphertext_text.contains(&credentials.platform_auth_key));
    assert!(!ciphertext_text.contains(&credentials.platform_mqtt_password));
    assert!(!ciphertext_text.contains("PRIVATE KEY"));

    let audit_rows = sqlx::query(
        "SELECT changed_fields_json FROM audit_event \
         WHERE object_type = 'aio_release_profile' AND object_key = ?",
    )
    .bind(&profile_key)
    .fetch_all(pools.workbench())
    .await
    .expect("read audit rows");
    assert_eq!(audit_rows.len(), 2);
    for row in audit_rows {
        let changed: String = row.try_get("changed_fields_json").expect("changed fields");
        assert!(changed.contains("credentials"));
        assert!(!changed.contains(&credentials.platform_auth_key));
    }

    repository
        .delete_test_profile(&profile_key)
        .await
        .expect("cleanup test profile");
    pools.close().await;
}
