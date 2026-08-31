use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyIdentity, HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::infrastructure::remote::RusshConnector;
use russh::server::{Auth, Server as _};

#[derive(Clone)]
struct PasswordServer;

impl russh::server::Server for PasswordServer {
    type Handler = PasswordHandler;

    fn new_client(&mut self, _peer_addr: Option<SocketAddr>) -> Self::Handler {
        PasswordHandler
    }
}

struct PasswordHandler;

impl russh::server::Handler for PasswordHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        Ok(if user == "phase4" && password == "phase4-password" {
            Auth::Accept
        } else {
            Auth::reject()
        })
    }
}

#[tokio::test]
async fn password_authentication_and_host_key_change_are_verified_in_process() {
    let host_key = russh::keys::PrivateKey::from(
        russh::keys::ssh_key::private::Ed25519Keypair::from_seed(&[42_u8; 32]),
    );
    let config = Arc::new(russh::server::Config {
        auth_rejection_time: Duration::from_millis(10),
        auth_rejection_time_initial: Some(Duration::ZERO),
        keys: vec![host_key],
        ..Default::default()
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind password ssh server");
    let port = listener.local_addr().expect("server address").port();
    let mut server = PasswordServer;
    let running = server.run_on_socket(config, &listener);
    let shutdown = running.handle();
    let client = async {
        let target = RemoteTarget {
            host: "127.0.0.1".into(),
            port,
            connect_timeout: Duration::from_secs(5),
        };
        let connector = RusshConnector::default();
        let auth = RemoteAuth::Password {
            username: "phase4".into(),
            password: SecretValue::new("phase4-password"),
        };
        let session = connector
            .connect(&target, &auth, HostKeyPolicy::Capture)
            .await
            .expect("password authentication");
        let actual_key = session.host_key().clone();
        session
            .disconnect()
            .await
            .expect("disconnect password session");
        assert!(
            connector
                .connect(
                    &target,
                    &RemoteAuth::Password {
                        username: "phase4".into(),
                        password: SecretValue::new("wrong-password"),
                    },
                    HostKeyPolicy::Require(actual_key.clone()),
                )
                .await
                .is_err()
        );
        assert!(matches!(
            connector
                .connect(
                    &target,
                    &auth,
                    HostKeyPolicy::Require(HostKeyIdentity {
                        algorithm: actual_key.algorithm,
                        fingerprint: "SHA256:changed".into(),
                    }),
                )
                .await,
            Err(AppError::HostKeyChanged { .. })
        ));
        shutdown.shutdown("test complete".into());
    };
    let (server_result, ()) = tokio::join!(running, client);
    server_result.expect("password server shutdown");
}
