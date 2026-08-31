use std::time::Duration;

use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;

#[tokio::test]
async fn host_key_tofu_requires_explicit_reconfirmation_on_change() {
    let temp = tempfile::tempdir().expect("temporary app data");
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .expect("local store");
    sqlx::query(
        "INSERT INTO local_project \
         (id, name, platform_url, db_host, db_port, db_user, business_db, workbench_db, \
          db_password_secret_ref, created_at, updated_at) \
         VALUES ('project-a', 'Project A', 'http://platform.test', 'db.test', 3306, \
                 'user', 'business', 'workbench', 'secret-ref', '1', '1')",
    )
    .execute(store.pool())
    .await
    .expect("project fixture");
    let repository = HostKeyRepository::new(store.pool().clone());
    let target = RemoteTarget {
        host: "192.0.2.10".into(),
        port: 22,
        connect_timeout: Duration::from_secs(5),
    };
    assert_eq!(
        repository
            .policy("project-a", &target)
            .await
            .expect("policy"),
        HostKeyPolicy::Capture
    );
    let first = HostKeyIdentity {
        algorithm: "Ed25519".into(),
        fingerprint: "SHA256:first".into(),
    };
    repository
        .confirm("project-a", &target, &first, false)
        .await
        .expect("confirm first key");
    assert_eq!(
        repository
            .policy("project-a", &target)
            .await
            .expect("policy"),
        HostKeyPolicy::Require(first.clone())
    );
    let changed = HostKeyIdentity {
        algorithm: "Ed25519".into(),
        fingerprint: "SHA256:changed".into(),
    };
    assert!(matches!(
        repository
            .confirm("project-a", &target, &changed, false)
            .await,
        Err(AppError::HostKeyChanged { .. })
    ));
    repository
        .confirm("project-a", &target, &changed, true)
        .await
        .expect("explicitly replace changed key");
    assert_eq!(
        repository
            .policy("project-a", &target)
            .await
            .expect("policy"),
        HostKeyPolicy::Require(changed)
    );
    assert!(
        repository
            .delete("project-a", &target)
            .await
            .expect("delete key")
    );
    assert_eq!(
        repository
            .policy("project-a", &target)
            .await
            .expect("policy"),
        HostKeyPolicy::Capture
    );
    store.close().await;
}
