#[path = "common/project_test_config.rs"]
mod project_test_config;

use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::core::secret::SecretValue;
use inxaiot_desk_buddy_lib::formal::platform_auth::{PlatformAuthAdapter, PlatformLoginSpec};
use inxaiot_desk_buddy_lib::infrastructure::database::{
    DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig,
};
use serde_json::Value;

struct TestConfig {
    mysql: MySqlProjectConfig,
    login: PlatformLoginSpec,
}

fn line<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|item| item.trim().strip_prefix(label))
        .map(str::trim)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn config() -> TestConfig {
    let database = project_test_config::database();
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let start = description.find('{').expect("login json");
    let end = start + description[start..].find('}').expect("login json end") + 1;
    let login: Value = serde_json::from_str(&description[start..end]).expect("login json parse");
    TestConfig {
        mysql: MySqlProjectConfig {
            host: database.host,
            port: database.port,
            username: database.username,
            password: SecretValue::new(database.password),
            platform_schema: line(&description, "平台业务数据库名：").into(),
            workbench_schema: line(&description, "工作台数据库：").into(),
            tls_mode: DatabaseTlsMode::Disabled,
            connect_timeout: Duration::from_secs(10),
        },
        login: PlatformLoginSpec {
            base_url: line(&description, "平台API：").into(),
            principal: login["principal"].as_str().expect("principal").into(),
            password: login["credentials"].as_str().expect("credentials").into(),
            session_uuid: login["sessionUUID"].as_str().expect("session uuid").into(),
            image_code: login["imageCode"].as_str().expect("image code").into(),
            timeout: Duration::from_secs(10),
        },
    }
}

#[test]
fn formal_connection_specs_never_debug_secrets() {
    let config = config();
    assert!(!format!("{:?}", config.mysql).contains(config.mysql.password.expose()));
    assert!(!format!("{:?}", config.login).contains(&config.login.password));
}

#[tokio::test]
#[ignore = "requires authorized project databases"]
async fn production_dual_pool_and_schema_capabilities() {
    async fn session_read_only(pool: &sqlx::MySqlPool) -> i64 {
        match sqlx::query_scalar("SELECT @@session.transaction_read_only")
            .fetch_one(pool)
            .await
        {
            Ok(value) => value,
            Err(error)
                if error
                    .as_database_error()
                    .and_then(|item| item.code())
                    .as_deref()
                    == Some("HY000") =>
            {
                sqlx::query_scalar("SELECT @@session.tx_read_only")
                    .fetch_one(pool)
                    .await
                    .expect("legacy session read-only flag")
            }
            Err(error) => panic!("session read-only flag: {error}"),
        }
    }

    let config = config();
    let pools = DualMySqlPools::connect(&config.mysql)
        .await
        .expect("connect project mysql pools");
    let capabilities = pools
        .probe(&config.mysql)
        .await
        .expect("schema capabilities");
    assert!(!capabilities.server_version.is_empty());
    assert!(capabilities.tls_cipher.is_none());
    inxaiot_desk_buddy_lib::infrastructure::platform_aio::require_aio_schema(&pools.platform)
        .await
        .expect("一体机表结构");
    assert!(capabilities.workbench_charset.is_some());
    let platform_read_only = session_read_only(&pools.platform).await;
    let workbench_read_only = session_read_only(&pools.workbench).await;
    assert_eq!(platform_read_only, 1);
    assert_eq!(workbench_read_only, 0);
    pools.close().await;
}

#[tokio::test]
#[ignore = "requires authorized platform login"]
async fn formal_platform_auth_session_is_redacted() {
    let config = config();
    let adapter = PlatformAuthAdapter::new(config.login.timeout).expect("auth adapter");
    let session = adapter.login(&config.login).await.expect("platform login");
    assert_eq!(session.principal, config.login.principal);
    assert!(!session.access_token.is_empty());
    assert!(!format!("{session:?}").contains(&session.access_token));
    assert!(session.expires_at.is_some());
    assert!(
        adapter
            .validate_access_token(&config.login.base_url, &session.access_token)
            .await
            .expect("validate platform access token")
    );
}
