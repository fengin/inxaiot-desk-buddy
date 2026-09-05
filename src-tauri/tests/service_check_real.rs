//! 显式运行：只读检查79/121和工作台库，检查结果仅写入隔离本机SQLite。
use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::application::agent_protocol::{
    inspect_services_request, parse_service_check_report,
};
use inxaiot_desk_buddy_lib::application::ports::remote_command::{
    ExecRequest, NoopRemoteOutputSink, RemoteCommandExecutor,
};
use inxaiot_desk_buddy_lib::application::ports::remote_session::{
    HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::formal::release_profile_repository::ReleaseProfileRepository;
use inxaiot_desk_buddy_lib::infrastructure::agent_asset::AGENT_SOURCE;
use inxaiot_desk_buddy_lib::infrastructure::remote::RusshConnector;
use inxaiot_desk_buddy_lib::infrastructure::service_check_repository::ServiceCheckRepository;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;

fn setting(config: &str, name: &str) -> String {
    let key = format!("${{{name}:");
    config
        .split_once(&key)
        .expect("配置字段")
        .1
        .split_once('}')
        .expect("配置默认值")
        .0
        .trim()
        .into()
}

#[tokio::test]
#[ignore = "需显式授权只读访问79/121；结果仅存隔离本机库"]
async fn inspect_79_121_and_save_real_local_observations() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .unwrap();
    let config = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .unwrap();
    assert_eq!(setting(&config, "MYSQL_HOST"), "192.168.3.6");
    let temp = tempfile::tempdir().unwrap();
    let local = LocalStore::open(&temp.path().join("local.db"))
        .await
        .unwrap();
    sqlx::query("INSERT INTO local_project (id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES ('inspection-test','检查测试','http://test','test',3306,'test','business','workbench','ref','1','1')")
        .execute(local.pool()).await.unwrap();
    let pool = MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(10))
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET SESSION TRANSACTION READ ONLY")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(
            MySqlConnectOptions::new()
                .host("192.168.3.6")
                .port(3306)
                .username(&setting(&config, "MYSQL_USER"))
                .password(&setting(&config, "MYSQL_PASSWORD"))
                .database("inxaiot_desk_buddy")
                .ssl_mode(MySqlSslMode::Disabled),
        )
        .await
        .unwrap();
    let profile = ReleaseProfileRepository::new(pool.clone())
        .get("default")
        .await
        .unwrap();
    let auth = if let Some(key) = profile
        .credentials
        .ssh_private_key
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        RemoteAuth::PrivateKey {
            username: profile.credentials.ssh_user.clone(),
            private_key: SecretValue::new(key),
            passphrase: None,
        }
    } else {
        RemoteAuth::Password {
            username: profile.credentials.ssh_user.clone(),
            password: SecretValue::new(profile.credentials.ssh_password.as_ref().unwrap()),
        }
    };
    let expected_before: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT mac_normalized, service_name, expected_image_name, expected_version FROM aio_node_service_version ORDER BY mac_normalized, service_name"
    ).fetch_all(&pool).await.unwrap();
    for (host, mac) in [
        ("192.168.3.79", "000C293BB933"),
        ("192.168.3.121", "000C290B71F4"),
    ] {
        let ip: String = sqlx::query_scalar("SELECT ip FROM aio_node WHERE mac_normalized = ?")
            .bind(mac)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(ip, host);
        let started = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        let token = CancellationToken::new();
        let session = RusshConnector::default()
            .connect(
                &RemoteTarget {
                    host: host.into(),
                    port: profile.values.ssh_port,
                    connect_timeout: Duration::from_secs(10),
                },
                &auth,
                HostKeyPolicy::Capture,
            )
            .await
            .unwrap();
        let identity = ExecRequest { program: "sh".into(), args: vec!["-s".into()], env: Default::default(),
            stdin: Some(b"ids=$(docker ps -aq --no-trunc)\n[ -z \"$ids\" ] || docker inspect --format '{{.Id}} {{.State.StartedAt}} {{.Image}}' $ids\n".to_vec()),
            total_timeout: Duration::from_secs(20), inactivity_timeout: Duration::from_secs(15) };
        let before = session
            .run(&identity, &token, &NoopRemoteOutputSink)
            .await
            .unwrap();
        let request = inspect_services_request(
            AGENT_SOURCE,
            &profile.values.aio_deploy_root,
            "manual",
            None,
        )
        .unwrap();
        let result = session
            .run(&request, &token, &NoopRemoteOutputSink)
            .await
            .unwrap();
        assert_eq!(result.exit_status, 0, "只读采集命令失败");
        let mut report = parse_service_check_report(&result.stdout).unwrap();
        assert_eq!(report.state, "succeeded");
        assert_eq!(report.expected_services.len(), 4);
        assert_eq!(report.services.len(), 4);
        assert!(
            report
                .services
                .iter()
                .all(|item| item.actual_image.is_some() && item.image_id.is_some())
        );
        report.started_at = started;
        report.checked_at = OffsetDateTime::now_utc().format(&Rfc3339).unwrap();
        for service in &mut report.services {
            service.checked_at = report.checked_at.clone();
        }
        let after = session
            .run(&identity, &token, &NoopRemoteOutputSink)
            .await
            .unwrap();
        assert_eq!(
            before.stdout, after.stdout,
            "只读检查不得改变容器、启动时间或镜像ID"
        );
        session.disconnect().await.unwrap();
        let repository = ServiceCheckRepository::new(local.pool().clone(), "inspection-test");
        let saved = repository.save_report(mac, &report).await.unwrap();
        assert_eq!(saved, repository.get(mac).await.unwrap().unwrap());
        assert_eq!(saved.services.len(), 4);
        println!(
            "{host}: {}；4个真实镜像ID已保存至本机隔离库；容器和启动时间未变化",
            saved.summary().1
        );
    }
    let expected_after: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT mac_normalized, service_name, expected_image_name, expected_version FROM aio_node_service_version ORDER BY mac_normalized, service_name"
    ).fetch_all(&pool).await.unwrap();
    assert_eq!(expected_before, expected_after, "检查不得修改已部署版本");
    pool.close().await;
    local.close().await;
}
