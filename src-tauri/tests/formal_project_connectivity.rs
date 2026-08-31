use std::path::Path;
use std::time::Duration;

use inxaiot_desk_buddy_lib::formal::mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use inxaiot_desk_buddy_lib::formal::platform_auth::{PlatformAuthAdapter, PlatformLoginSpec};
use serde_json::Value;

struct TestConfig {
    mysql: MySqlConnectionSpec,
    login: PlatformLoginSpec,
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

fn config() -> TestConfig {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let start = description.find('{').expect("login json");
    let end = start + description[start..].find('}').expect("login json end") + 1;
    let login: Value = serde_json::from_str(&description[start..end]).expect("login json parse");
    let workspace = project.parent().and_then(Path::parent).expect("workspace");
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
            platform_schema: line(&description, "平台业务数据库名：").into(),
            workbench_schema: line(&description, "工作台数据库：").into(),
            tls_mode: MySqlTlsMode::Disabled,
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
    assert!(!format!("{:?}", config.mysql).contains(&config.mysql.password));
    assert!(!format!("{:?}", config.login).contains(&config.login.password));
}

#[tokio::test]
#[ignore = "requires authorized project databases"]
async fn formal_dual_pool_and_schema_capabilities() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config.mysql)
        .await
        .expect("connect project mysql pools");
    let capabilities = pools
        .probe(&config.mysql)
        .await
        .expect("schema capabilities");
    assert!(!capabilities.server_version.is_empty());
    assert!(!capabilities.connection_encrypted);
    assert!(capabilities.missing_platform_aio_columns.is_empty());
    assert!(capabilities.workbench_charset.is_some());
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
