mod common;

use common::{auth, config, target};
use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyPolicy, RemoteConnection, RemoteConnector,
};
use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use inxaiot_desk_buddy_lib::infrastructure::remote::RusshConnector;

#[tokio::test]
#[ignore = "requires both authorized Linux edge nodes"]
async fn tofu_repository_drives_pinned_connection_and_changed_key_blocking() {
    let config = config();
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
    let connector = RusshConnector::default();
    for host in &config.hosts {
        let target = target(host);
        let auth = auth(&config);
        assert_eq!(
            repository
                .policy("project-a", &target)
                .await
                .expect("policy"),
            HostKeyPolicy::Capture
        );
        let discovery = connector
            .connect(&target, &auth, HostKeyPolicy::Capture)
            .await
            .expect("capture actual host key");
        let identity = discovery.host_key().clone();
        repository
            .confirm("project-a", &target, &identity, false)
            .await
            .expect("confirm actual host key");
        discovery.disconnect().await.expect("disconnect discovery");
        let pinned = connector
            .connect(
                &target,
                &auth,
                repository
                    .policy("project-a", &target)
                    .await
                    .expect("policy"),
            )
            .await
            .expect("connect with stored host key");
        pinned.disconnect().await.expect("disconnect pinned");
        let mut changed = identity;
        changed.fingerprint = "SHA256:deliberately-changed".into();
        assert!(matches!(
            connector
                .connect(&target, &auth, HostKeyPolicy::Require(changed))
                .await,
            Err(AppError::HostKeyChanged { .. })
        ));
    }
    store.close().await;
}
