use std::path::{Path, PathBuf};
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::credential_crypto::{self, ReleaseCredentials};
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::release_profile_repository::{
    ReleaseAgentScriptWrite, ReleaseProfileRepository, ReleaseProfileValues, ReleaseProfileWrite,
    StoredReleaseAgentScript,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::embedded_agent_asset;
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
            host_info_template: std::fs::read_to_string(
                config
                    .project_root
                    .join("test/templates/host-info.json.template"),
            )
            .expect("host-info template"),
            platform_host: api_host.into(),
            platform_api_port: api_port,
            platform_mqtt_host: mqtt_host.into(),
            platform_mqtt_port: mqtt_port,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy".into(),
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
fn encrypted_release_credentials_round_trip_is_authenticated() {
    let config = config();
    let (_, credentials) = release_values(&config);
    let envelope =
        credential_crypto::encrypt_release_credentials(&credentials).expect("encrypt credentials");
    let decrypted =
        credential_crypto::decrypt_release_credentials(&envelope).expect("decrypt credentials");
    assert_eq!(decrypted, credentials);
    let mut tampered = envelope;
    tampered.ciphertext[0] ^= 1;
    assert!(credential_crypto::decrypt_release_credentials(&tampered).is_err());
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
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values: values.clone(),
            credentials: credentials.clone(),
            expected_version: None,
            operator_name: "phase3-test".into(),
            instance_id: "instance-a".into(),
        })
        .await
        .expect("create release profile");
    assert_eq!(first.version, 1);
    assert_eq!(first.credentials, credentials);
    assert_eq!(first.values.host_info_template, values.host_info_template);

    let stale = repository
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values: values.clone(),
            credentials: credentials.clone(),
            expected_version: Some(0),
            operator_name: "phase3-test".into(),
            instance_id: "instance-b".into(),
        })
        .await;
    assert!(matches!(stale, Err(FormalError::Conflict(_))));

    let second = repository
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values,
            credentials: credentials.clone(),
            expected_version: Some(1),
            operator_name: "phase3-test".into(),
            instance_id: "instance-b".into(),
        })
        .await
        .expect("update release profile");
    assert_eq!(second.version, 2);
    assert!(second.agent_script.is_none());

    let asset = embedded_agent_asset().expect("embedded agent");
    let third = repository
        .replace_agent_script(ReleaseAgentScriptWrite {
            profile_key: profile_key.clone(),
            script: StoredReleaseAgentScript {
                content: asset.content,
                version: asset.version,
                protocol_version: asset.protocol_version,
                sha256: asset.sha256.clone(),
            },
            expected_version: second.version,
            operator_name: "phase3-test".into(),
            instance_id: "instance-c".into(),
        })
        .await
        .expect("replace project agent");
    assert_eq!(third.version, 3);
    assert_eq!(
        third.agent_script.expect("project agent").sha256,
        asset.sha256
    );

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
        "SELECT CAST(changed_fields_json AS CHAR) AS changed_fields_json FROM audit_event \
         WHERE object_type = 'aio_release_profile' AND object_key = ?",
    )
    .bind(&profile_key)
    .fetch_all(pools.workbench())
    .await
    .expect("read audit rows");
    assert_eq!(audit_rows.len(), 3);
    let mut changed_sets = Vec::new();
    for row in audit_rows {
        let changed: String = row.try_get("changed_fields_json").expect("changed fields");
        assert!(!changed.contains(&credentials.platform_auth_key));
        changed_sets.push(changed);
    }
    assert!(
        changed_sets
            .iter()
            .any(|changed| changed.contains("credentials"))
    );
    assert!(
        changed_sets
            .iter()
            .any(|changed| changed.contains("agent_script"))
    );

    repository
        .delete_test_profile(&profile_key)
        .await
        .expect("cleanup test profile");
    pools.close().await;
}

#[tokio::test]
#[ignore = "widens source columns in the authorized workbench database; test rows are rolled back"]
async fn readable_client_instance_round_trips_without_truncation() {
    let config = config();
    assert_eq!(config.mysql.host, "192.168.3.6");
    assert_eq!(config.mysql.workbench_schema, "inxaiot_desk_buddy");
    let pools = ProjectMySqlPools::connect(&config.mysql)
        .await
        .expect("connect workbench");
    WorkbenchStore::new(pools.workbench().clone())
        .migrate()
        .await
        .expect("widen instance columns");
    for (table, column) in [
        ("operation_record", "instance_id"),
        ("audit_event", "instance_id"),
        ("resource_lease", "owner_instance_id"),
    ] {
        let length: u64 = sqlx::query_scalar(
            "SELECT CHARACTER_MAXIMUM_LENGTH FROM information_schema.columns WHERE table_schema = ? AND table_name = ? AND column_name = ?"
        ).bind(&config.mysql.workbench_schema).bind(table).bind(column)
            .fetch_one(pools.workbench()).await.expect("instance column length");
        assert_eq!(length, 512);
    }
    let id = Uuid::now_v7().to_string();
    let resource = format!("test:readable-instance:{id}");
    let instance = format!(
        "LF-PC-{}-001122AABBCC-192.168.3.142",
        "完整电脑名称".repeat(12)
    );
    assert!(instance.chars().count() > 64);
    let mut transaction = pools
        .workbench()
        .begin()
        .await
        .expect("isolated source test");
    sqlx::query("INSERT INTO operation_record (id, domain_type, operation_type, operation_name, operator_name, instance_id, state, started_at, heartbeat_at) VALUES (?, 'aio', 'source_test', '实例来源回归', 'source-test', ?, 'succeeded', UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))")
        .bind(&id).bind(&instance).execute(&mut *transaction).await.expect("operation source");
    sqlx::query("INSERT INTO audit_event (id, domain_type, object_type, object_key, action, operator_name, instance_id, changed_fields_json, created_at) VALUES (?, 'aio', 'source_test', ?, 'create', 'source-test', ?, '[]', UTC_TIMESTAMP(6))")
        .bind(&id).bind(&resource).bind(&instance).execute(&mut *transaction).await.expect("audit source");
    sqlx::query("INSERT INTO resource_lease (resource_type, resource_key, domain_type, operation_id, owner_instance_id, owner_user, lease_token, fencing_token, acquired_at, heartbeat_at, expires_at) VALUES ('source_test', ?, 'aio', ?, ?, 'source-test', ?, 1, UTC_TIMESTAMP(6), UTC_TIMESTAMP(6), UTC_TIMESTAMP(6))")
        .bind(&resource).bind(&id).bind(&instance).bind(&id).execute(&mut *transaction).await.expect("lease source");
    for query in [
        "SELECT instance_id FROM operation_record WHERE id = ?",
        "SELECT instance_id FROM audit_event WHERE id = ?",
        "SELECT owner_instance_id FROM resource_lease WHERE operation_id = ? AND resource_type = 'source_test'",
    ] {
        let stored: String = sqlx::query_scalar(query)
            .bind(&id)
            .fetch_one(&mut *transaction)
            .await
            .expect("source round trip");
        assert_eq!(stored, instance);
    }
    transaction.rollback().await.expect("rollback test rows");
    for query in [
        "SELECT COUNT(*) FROM operation_record WHERE id = ?",
        "SELECT COUNT(*) FROM audit_event WHERE id = ?",
        "SELECT COUNT(*) FROM resource_lease WHERE operation_id = ? AND resource_type = 'source_test'",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .bind(&id)
            .fetch_one(pools.workbench())
            .await
            .expect("no test row retained");
        assert_eq!(count, 0);
    }
    pools.close().await;
}
