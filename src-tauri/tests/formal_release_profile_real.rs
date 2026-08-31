use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::credential_crypto::ReleaseCredentials;
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::release_profile_repository::{
    ReleaseProfileRepository, ReleaseProfileValues, ReleaseProfileWrite,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use sqlx::Row;
use uuid::Uuid;

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

fn config() -> MySqlConnectionSpec {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let workspace = project.parent().and_then(Path::parent).expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    MySqlConnectionSpec {
        host: default(&yaml, "MYSQL_HOST").into(),
        port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
        username: default(&yaml, "MYSQL_USER").into(),
        password: default(&yaml, "MYSQL_PASSWORD").into(),
        platform_schema: line(&description, "平台业务数据库名：").into(),
        workbench_schema: line(&description, "工作台数据库：").into(),
        tls_mode: MySqlTlsMode::Disabled,
        connect_timeout: Duration::from_secs(10),
    }
}

fn values() -> ReleaseProfileValues {
    ReleaseProfileValues {
        env_template: "PLATFORM_HOST={{ platform.host }}".into(),
        compose_template: "services: {}".into(),
        platform_host: "platform.test".into(),
        platform_api_port: 8055,
        platform_mqtt_host: "mqtt.test".into(),
        platform_mqtt_port: 1883,
        ssh_port: 22,
        ssh_timeout_seconds: 15,
        aio_data_root: "/opt/data".into(),
        aio_deploy_root: "/opt/data/deploy/inxvision-edge".into(),
    }
}

fn credentials() -> ReleaseCredentials {
    ReleaseCredentials {
        platform_auth_key: "isolated-auth-key".into(),
        platform_mqtt_user: "isolated-platform-user".into(),
        platform_mqtt_password: "isolated-platform-password".into(),
        aio_mqtt_user: "isolated-aio-user".into(),
        aio_mqtt_password: "isolated-aio-password".into(),
        ssh_user: "root".into(),
        ssh_password: None,
        ssh_private_key: Some("isolated-private-key".into()),
    }
}

#[tokio::test]
#[ignore = "writes and removes exactly one isolated release profile"]
async fn encrypted_profile_version_audit_and_exact_cleanup() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let store = WorkbenchStore::new(pools.workbench().clone());
    store.migrate().await.expect("workbench migration");
    let repository = ReleaseProfileRepository::new(pools.workbench().clone());
    let profile_key = format!("poc-{}", &Uuid::now_v7().simple().to_string()[..20]);
    let credentials = credentials();

    let first = repository
        .save(
            &config.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values: values(),
                credentials: credentials.clone(),
                expected_version: None,
                operator_name: "phase3-test".into(),
                instance_id: "instance-a".into(),
            },
        )
        .await
        .expect("create profile");
    assert_eq!(first.version, 1);
    assert_eq!(first.credentials, credentials);

    let stale = repository
        .save(
            &config.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values: values(),
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
            &config.password,
            ReleaseProfileWrite {
                profile_key: profile_key.clone(),
                values: values(),
                credentials: credentials.clone(),
                expected_version: Some(1),
                operator_name: "phase3-test".into(),
                instance_id: "instance-b".into(),
            },
        )
        .await
        .expect("update profile");
    assert_eq!(second.version, 2);

    let ciphertext: Vec<u8> = sqlx::query_scalar(
        "SELECT credential_ciphertext FROM aio_release_profile WHERE profile_key = ?",
    )
    .bind(&profile_key)
    .fetch_one(pools.workbench())
    .await
    .expect("ciphertext");
    let ciphertext_text = String::from_utf8_lossy(&ciphertext);
    assert!(!ciphertext_text.contains(&credentials.platform_auth_key));
    assert!(!ciphertext_text.contains(&credentials.platform_mqtt_password));
    assert!(!ciphertext_text.contains("isolated-private-key"));

    let audit_rows = sqlx::query(
        "SELECT CAST(changed_fields_json AS CHAR) AS changed_fields_text FROM audit_event \
         WHERE object_type = 'aio_release_profile' AND object_key = ?",
    )
    .bind(&profile_key)
    .fetch_all(pools.workbench())
    .await
    .expect("audit rows");
    assert_eq!(audit_rows.len(), 2);
    for row in audit_rows {
        let changed: String = row.try_get("changed_fields_text").expect("changed fields");
        assert!(changed.contains("credentials"));
        assert!(!changed.contains("isolated-auth-key"));
    }

    repository
        .delete_test_profile(&profile_key)
        .await
        .expect("exact cleanup");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM aio_release_profile WHERE profile_key = ?) + \
         (SELECT COUNT(*) FROM audit_event WHERE object_type = 'aio_release_profile' AND object_key = ?)",
    )
    .bind(&profile_key)
    .bind(&profile_key)
    .fetch_one(pools.workbench())
    .await
    .expect("verify cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}
