mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::host_key_repository::HostKeyRepository;
use inxaiot_desk_buddy_lib::infrastructure::remote::{RusshConnector, observed::ObservedConnector};
use russh::server::{Auth, Server as _};

async fn store() -> (tempfile::TempDir, LocalStore) {
    let temp = tempfile::tempdir().unwrap();
    let store = LocalStore::open(temp.path().join("local.db"))
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO local_project \
         (id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) \
         VALUES ('observed-project','自动指纹测试','http://localhost','localhost',3306,'fixture','business','workbench','fixture-ref','1','1')",
    ).execute(store.pool()).await.unwrap();
    (temp, store)
}

struct PasswordServer;
impl russh::server::Server for PasswordServer {
    type Handler = PasswordHandler;
    fn new_client(&mut self, _: Option<SocketAddr>) -> Self::Handler {
        PasswordHandler
    }
}
struct PasswordHandler;
impl russh::server::Handler for PasswordHandler {
    type Error = russh::Error;
    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "fixture-user" && password == "unit-password" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
}

#[tokio::test]
async fn first_use_and_changed_host_keys_are_automatic_but_bad_credentials_still_fail() {
    let (_temp, store) = store().await;
    let repository = HostKeyRepository::new(store.pool().clone());
    let changes = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed_changes = changes.clone();
    let connector = ObservedConnector::new(
        RusshConnector::default(),
        repository.clone(),
        "observed-project",
    )
    .with_change_handler(Arc::new(move |observation| {
        observed_changes.lock().unwrap().push(observation.message());
        Ok(())
    }));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let target = RemoteTarget {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        connect_timeout: Duration::from_secs(5),
    };
    let auth = RemoteAuth::Password {
        username: "fixture-user".into(),
        password: SecretValue::new("unit-password"),
    };
    assert!(
        repository
            .get("observed-project", &target)
            .await
            .unwrap()
            .is_none()
    );
    for (index, seed) in [42, 43].into_iter().enumerate() {
        let key = russh::keys::PrivateKey::from(
            russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[seed; 32]),
        );
        let config = Arc::new(russh::server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::from_millis(10),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let mut server = PasswordServer;
        let running = server.run_on_socket(config, &listener);
        let shutdown = running.handle();
        let client = async {
            let (session, observed) = connector.connect_observed(&target, &auth).await.unwrap();
            assert_eq!(observed.changed(), index == 1);
            assert_eq!(observed.previous.is_none(), index == 0);
            assert_eq!(session.host_key(), &observed.record.identity);
            if let Some(previous) = &observed.previous {
                assert!(observed.message().contains(&previous.fingerprint));
                assert!(
                    observed
                        .message()
                        .contains(&observed.record.identity.fingerprint)
                );
                assert!(observed.message().contains("继续连接"));
            }
            session.disconnect().await.unwrap();
            let same = connector
                .connect(&target, &auth, HostKeyPolicy::Capture)
                .await
                .unwrap();
            same.disconnect().await.unwrap();
            let before_failure = repository
                .get("observed-project", &target)
                .await
                .unwrap()
                .unwrap();
            let bad = RemoteAuth::Password {
                username: "fixture-user".into(),
                password: SecretValue::new("wrong-password"),
            };
            assert!(
                connector
                    .connect(&target, &bad, HostKeyPolicy::Capture)
                    .await
                    .is_err()
            );
            let after_failure = repository
                .get("observed-project", &target)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(before_failure.identity, after_failure.identity);
            assert_eq!(before_failure.accepted_at, after_failure.accepted_at);
            assert_eq!(changes.lock().unwrap().len(), index);
            shutdown.shutdown("unit test complete".into());
        };
        let (result, ()) = tokio::join!(running, client);
        result.unwrap();
    }
    assert_eq!(changes.lock().unwrap().len(), 1);
    store.close().await;
}

#[tokio::test]
async fn automatic_observations_are_scoped_and_concurrent_updates_do_not_require_confirmation() {
    let (_temp, store) = store().await;
    let repository = HostKeyRepository::new(store.pool().clone());
    let target = RemoteTarget {
        host: "192.0.2.1".into(),
        port: 22,
        connect_timeout: Duration::from_secs(5),
    };
    let old = HostKeyIdentity {
        algorithm: "Ed25519".into(),
        fingerprint: "SHA256:old-unit-value".into(),
    };
    let new = HostKeyIdentity {
        algorithm: "Ed25519".into(),
        fingerprint: "SHA256:new-unit-value".into(),
    };
    repository
        .observe("observed-project", &target, &old)
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        repository.observe("observed-project", &target, &new),
        repository.observe("observed-project", &target, &new)
    );
    assert_eq!(
        [a.unwrap(), b.unwrap()]
            .iter()
            .filter(|value| value.changed())
            .count(),
        1
    );
    assert_eq!(
        repository
            .get("observed-project", &target)
            .await
            .unwrap()
            .unwrap()
            .identity,
        new
    );
    let another_port = RemoteTarget {
        port: 2222,
        ..target
    };
    assert!(
        repository
            .observe("observed-project", &another_port, &old)
            .await
            .unwrap()
            .previous
            .is_none()
    );
    assert!(
        repository
            .get("other-project", &another_port)
            .await
            .unwrap()
            .is_none()
    );
    store.close().await;
}

#[tokio::test]
#[ignore = "只读连接两个已授权节点，使用原RSA私钥；仅在临时SQLite中模拟上次不同指纹，不修改远端密钥或部署"]
async fn existing_rsa_keys_connect_to_both_nodes_and_refresh_changed_local_observations() {
    let config = common::config();
    assert_eq!(config.hosts.len(), 2);
    let (_temp, store) = store().await;
    let repository = HostKeyRepository::new(store.pool().clone());
    let connector = ObservedConnector::new(
        RusshConnector::default(),
        repository.clone(),
        "observed-project",
    );
    for host in &config.hosts {
        assert!(["192.168.3.79", "192.168.3.121"].contains(&host.as_str()));
        let target = common::target(host);
        repository
            .observe(
                "observed-project",
                &target,
                &HostKeyIdentity {
                    algorithm: "Ed25519".into(),
                    fingerprint: "SHA256:isolated-previous-observation".into(),
                },
            )
            .await
            .unwrap();
        let (session, observed) = connector
            .connect_observed(&target, &common::auth(&config))
            .await
            .unwrap();
        assert!(observed.changed());
        assert_eq!(
            repository
                .get("observed-project", &target)
                .await
                .unwrap()
                .unwrap()
                .identity,
            *session.host_key()
        );
        session.disconnect().await.unwrap();
    }
    store.close().await;
}
