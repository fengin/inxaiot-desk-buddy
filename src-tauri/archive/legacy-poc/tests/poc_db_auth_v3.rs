#[path = "../src/core/mod.rs"]
mod core;
#[path = "../src/infrastructure/database_v2.rs"]
mod database;
#[path = "../src/infrastructure/platform_auth_v2.rs"]
mod platform_auth;

use std::path::{Path, PathBuf};
use std::time::Duration;

use core::secret::SecretValue;
use database::{DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig};
use platform_auth::{PlatformAuthClient, PlatformLoginConfig};
use serde_json::Value;

struct Config {
    mysql: MySqlProjectConfig,
    login: PlatformLoginConfig,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project root")
        .to_path_buf()
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

fn config() -> Config {
    let root = root();
    let description = std::fs::read_to_string(root.join("test/测试数据说明.txt"))
        .expect("test data");
    let start = description.find('{').expect("login json");
    let end = start + description[start..].find('}').expect("login json end") + 1;
    let login: Value = serde_json::from_str(&description[start..end]).expect("login json parse");
    let workspace = root.parent().and_then(Path::parent).expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    Config {
        mysql: MySqlProjectConfig {
            host: default(&yaml, "MYSQL_HOST").into(),
            port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
            username: default(&yaml, "MYSQL_USER").into(),
            password: SecretValue::new(default(&yaml, "MYSQL_PASSWORD")),
            platform_schema: line(&description, "平台业务数据库名：").into(),
            workbench_schema: line(&description, "工作台数据库：").into(),
            connect_timeout: Duration::from_secs(10),
            tls_mode: DatabaseTlsMode::Disabled,
        },
        login: PlatformLoginConfig {
            base_url: line(&description, "平台API：").into(),
            principal: login["principal"].as_str().expect("principal").into(),
            credentials: SecretValue::new(login["credentials"].as_str().expect("credentials")),
            session_uuid: SecretValue::new(login["sessionUUID"].as_str().expect("session uuid")),
            image_code: SecretValue::new(login["imageCode"].as_str().expect("image code")),
            timeout: Duration::from_secs(10),
        },
    }
}

#[test]
fn tls_mode_is_explicit_and_debug_output_is_redacted() {
    let config = config();
    assert_eq!(config.mysql.tls_mode, DatabaseTlsMode::Disabled);
    assert!(!format!("{:?}", config.mysql).contains(config.mysql.password.expose()));
}

#[tokio::test]
#[ignore = "requires authorized project MySQL"]
async fn dual_pool_probe_reports_unencrypted_test_connection() {
    let config = config();
    let pools = DualMySqlPools::connect(&config.mysql)
        .await
        .expect("dual mysql pools");
    let report = pools.probe(&config.mysql).await.expect("schema probe");
    assert!(report.platform_aio_table_exists);
    assert!(report.missing_required_columns.is_empty());
    assert!(report.workbench_charset.is_some());
    assert!(report.tls_cipher.is_none());
    pools.close().await;
}

#[tokio::test]
#[ignore = "requires authorized platform login"]
async fn real_platform_login_accepts_base64_der_public_key() {
    let config = config();
    let client = PlatformAuthClient::new(config.login.timeout).expect("auth client");
    let session = client.login(&config.login).await.expect("platform login");
    assert_eq!(session.principal, config.login.principal);
    assert!(!session.token_type.is_empty());
    assert!(!session.access_token.is_empty());
    assert!(!format!("{session:?}").contains(session.access_token.expose()));
}

