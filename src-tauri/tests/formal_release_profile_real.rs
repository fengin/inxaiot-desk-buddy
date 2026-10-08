#[path = "common/aio_test_config.rs"]
mod aio_test_config;

use inxaiot_desk_buddy_lib::formal::credential_crypto::{
    INXVISION_CREDENTIAL_SCHEME, ReleaseCredentials,
};
use inxaiot_desk_buddy_lib::formal::error::FormalError;
use inxaiot_desk_buddy_lib::formal::release_profile_repository::{
    ReleaseProfileRepository, ReleaseProfileValues, ReleaseProfileWrite,
};
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::database::DualMySqlPools;
use sqlx::Row;
use uuid::Uuid;

fn values() -> ReleaseProfileValues {
    ReleaseProfileValues {
        env_template: "PLATFORM_HOST={{ platform.host }}".into(),
        compose_template: "services:\n  app:\n    image: ${APP_IMAGE}".into(),
        host_info_template: r#"{"mac":"{{ node.mac }}","ip":"{{ node.ip }}","hostname":"{{ node.name }}","authKey":"{{ authKey }}"}"#.into(),
        platform_host: "platform.test".into(),
        platform_api_port: 8055,
        platform_mqtt_host: "mqtt.test".into(),
        platform_mqtt_port: 1883,
        ssh_port: 22,
        ssh_timeout_seconds: 15,
        aio_data_root: "/opt/data".into(),
        aio_deploy_root: "/opt/data/deploy".into(),
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
    let config = aio_test_config::isolated_project();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let store = WorkbenchStore::new(pools.workbench.clone());
    store.migrate().await.expect("workbench migration");
    let repository = ReleaseProfileRepository::new(pools.workbench.clone());
    let profile_key = format!("poc-{}", &Uuid::now_v7().simple().to_string()[..20]);
    let credentials = credentials();
    let first = repository
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values: values(),
            credentials: credentials.clone(),
            expected_version: None,
            operator_name: "phase3-test".into(),
            instance_id: "instance-a".into(),
        })
        .await
        .expect("create profile");
    assert_eq!(first.version, 1);
    assert_eq!(first.credentials, credentials);

    let stale = repository
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values: values(),
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
            values: values(),
            credentials: credentials.clone(),
            expected_version: Some(1),
            operator_name: "phase3-test".into(),
            instance_id: "instance-b".into(),
        })
        .await
        .expect("update profile");
    assert_eq!(second.version, 2);

    let ciphertext: Vec<u8> = sqlx::query_scalar(
        "SELECT credential_ciphertext FROM aio_release_profile WHERE profile_key = ?",
    )
    .bind(&profile_key)
    .fetch_one(&pools.workbench)
    .await
    .expect("ciphertext");
    let ciphertext_text = String::from_utf8_lossy(&ciphertext);
    assert!(!ciphertext_text.contains(&credentials.platform_auth_key));
    assert!(!ciphertext_text.contains(&credentials.platform_mqtt_password));
    assert!(!ciphertext_text.contains("isolated-private-key"));
    let scheme: String = sqlx::query_scalar(
        "SELECT credential_scheme FROM aio_release_profile WHERE profile_key = ?",
    )
    .bind(&profile_key)
    .fetch_one(&pools.workbench)
    .await
    .expect("credential scheme");
    assert_eq!(scheme, INXVISION_CREDENTIAL_SCHEME);

    let audit_rows = sqlx::query(
        "SELECT CAST(changed_fields_json AS CHAR) AS changed_fields_text FROM audit_event \
         WHERE object_type = 'aio_release_profile' AND object_key = ?",
    )
    .bind(&profile_key)
    .fetch_all(&pools.workbench)
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
    .fetch_one(&pools.workbench)
    .await
    .expect("verify cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}

#[tokio::test]
#[ignore = "reads and exactly removes one isolated fixed-key release profile"]
async fn fixed_credential_profile_reads_from_an_independent_client() {
    let config = aio_test_config::isolated_project();
    let pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .expect("workbench migration");
    let repository = ReleaseProfileRepository::new(pools.workbench.clone());
    let suffix = &Uuid::now_v7().simple().to_string()[..16];
    let profile_key = format!("fixed-key-poc-{suffix}");
    let credentials = credentials();
    let created = repository
        .save(ReleaseProfileWrite {
            profile_key: profile_key.clone(),
            values: values(),
            credentials: credentials.clone(),
            expected_version: None,
            operator_name: "fixed-key-test".into(),
            instance_id: "instance-create".into(),
        })
        .await
        .expect("create fixed-key encrypted profile");
    assert_eq!(created.credentials, credentials);
    let second_pools = DualMySqlPools::connect(&config)
        .await
        .expect("connect independent client mysql");
    let second_repository = ReleaseProfileRepository::new(second_pools.workbench.clone());
    assert_eq!(
        second_repository
            .get(&profile_key)
            .await
            .expect("independent client decrypts profile")
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
    .fetch_one(&pools.workbench)
    .await
    .expect("verify cleanup");
    assert_eq!(remaining, 0);
    second_pools.close().await;
    pools.close().await;
}
