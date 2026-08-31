use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::credential_crypto::{
    ProjectMasterKey, ReleaseCredentials, encrypt_release_credentials_legacy,
};
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::release_master_key::{
    ReleaseKeyProjectBinding, ReleaseMasterKeyManager,
};
use inxaiot_desk_buddy_lib::formal::release_profile_repository::{
    ReleaseProfileRepository, ReleaseProfileValues, ReleaseProfileWrite,
};
use inxaiot_desk_buddy_lib::formal::secret_store::MemorySecretStore;
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
    let key = ProjectMasterKey::generate(1).expect("project master key");

    let first = repository
        .save(
            &key,
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
            &key,
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
            &key,
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

#[tokio::test]
#[ignore = "migrates, rotates, transfers and exactly removes one isolated release profile"]
async fn project_master_key_migration_rotation_failure_and_cross_machine_transfer() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench().clone())
        .migrate()
        .await
        .expect("workbench migration");
    let repository = ReleaseProfileRepository::new(pools.workbench().clone());
    let suffix = &Uuid::now_v7().simple().to_string()[..20];
    let profile_key = format!("key-poc-{suffix}");
    let first_project_id = format!("project-{suffix}");
    let second_project_id = format!("project-copy-{suffix}");
    let credentials = credentials();
    let legacy_values = values();
    let legacy_envelope = encrypt_release_credentials_legacy(&config.password, &credentials)
        .expect("encrypt isolated legacy credentials");
    sqlx::query(
        "INSERT INTO aio_release_profile \
         (profile_key, env_template, compose_template, platform_host, platform_api_port, \
          platform_mqtt_host, platform_mqtt_port, ssh_port, ssh_timeout_seconds, \
          aio_data_root, aio_deploy_root, credential_scheme, credential_key_version, \
          credential_salt, credential_nonce, credential_ciphertext, version, updated_by, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'key-migration-test', UTC_TIMESTAMP(6))",
    )
    .bind(&profile_key)
    .bind(&legacy_values.env_template)
    .bind(&legacy_values.compose_template)
    .bind(&legacy_values.platform_host)
    .bind(u32::from(legacy_values.platform_api_port))
    .bind(&legacy_values.platform_mqtt_host)
    .bind(u32::from(legacy_values.platform_mqtt_port))
    .bind(u32::from(legacy_values.ssh_port))
    .bind(legacy_values.ssh_timeout_seconds)
    .bind(&legacy_values.aio_data_root)
    .bind(&legacy_values.aio_deploy_root)
    .bind(&legacy_envelope.scheme)
    .bind(legacy_envelope.key_version)
    .bind(legacy_envelope.salt.as_slice())
    .bind(legacy_envelope.nonce.as_slice())
    .bind(&legacy_envelope.ciphertext)
    .execute(pools.workbench())
        .await
        .expect("create isolated legacy profile");

    let first_store = Arc::new(MemorySecretStore::default());
    let first_manager = ReleaseMasterKeyManager::new(first_store);
    let migrated = first_manager
        .load_profile(
            &repository,
            &first_project_id,
            &profile_key,
            &config.password,
            "key-migration-test",
            "instance-migrate",
        )
        .await
        .expect("migrate legacy ciphertext");
    assert_eq!(migrated.credentials, credentials);
    let metadata = repository
        .credential_metadata(&profile_key)
        .await
        .expect("metadata")
        .expect("profile metadata");
    assert!(metadata.is_project_key());
    assert_eq!(metadata.key_version, 1);

    let after_database_password_change = first_manager
        .load_profile(
            &repository,
            &first_project_id,
            &profile_key,
            "completely-different-database-password",
            "key-migration-test",
            "instance-read",
        )
        .await
        .expect("database password is no longer the credential key");
    assert_eq!(after_database_password_change.credentials, credentials);

    let injected_failure = first_manager
        .rotate(
            &repository,
            &first_project_id,
            &profile_key,
            &config.password,
            "key-migration-test",
            &"x".repeat(1000),
        )
        .await;
    assert!(injected_failure.is_err());
    assert_eq!(
        repository
            .credential_metadata(&profile_key)
            .await
            .expect("metadata after rollback")
            .expect("profile metadata")
            .key_version,
        1
    );
    assert!(first_manager.load_key(&first_project_id, 2).is_err());
    assert_eq!(
        first_manager
            .load_profile(
                &repository,
                &first_project_id,
                &profile_key,
                &config.password,
                "key-migration-test",
                "instance-after-failure",
            )
            .await
            .expect("old key remains usable")
            .credentials,
        credentials
    );

    let rotated_version = first_manager
        .rotate(
            &repository,
            &first_project_id,
            &profile_key,
            &config.password,
            "key-migration-test",
            "instance-rotate",
        )
        .await
        .expect("rotate key");
    assert_eq!(rotated_version, 2);
    assert!(first_manager.load_key(&first_project_id, 1).is_err());

    let binding = ReleaseKeyProjectBinding {
        platform_url: "http://isolated-platform.example:8055".into(),
        db_host: config.host.clone(),
        db_port: config.port,
        workbench_db: config.workbench_schema.clone(),
    };
    let temporary = tempfile::tempdir().expect("transfer directory");
    let package_path = temporary.path().join("project-key.inxkey");
    first_manager
        .export_key_package(
            &repository,
            &first_project_id,
            &profile_key,
            &config.password,
            "key-migration-test",
            "instance-export",
            &binding,
            &package_path,
            "isolated-strong-passphrase",
        )
        .await
        .expect("export key package");
    let package_text = std::fs::read_to_string(&package_path).expect("read package");
    assert!(
        !package_text.contains(&hex::encode(
            first_manager
                .load_key(&first_project_id, 2)
                .expect("current key")
                .material()
        ))
    );

    let second_manager = ReleaseMasterKeyManager::new(Arc::new(MemorySecretStore::default()));
    second_manager
        .import_key_package(
            &repository,
            &second_project_id,
            &profile_key,
            &binding,
            &package_path,
            "isolated-strong-passphrase",
        )
        .await
        .expect("import key package on another machine");
    assert_eq!(
        second_manager
            .load_profile(
                &repository,
                &second_project_id,
                &profile_key,
                "unrelated-database-password",
                "key-migration-test",
                "instance-copy-read",
            )
            .await
            .expect("second machine decrypts profile")
            .credentials,
        credentials
    );

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
