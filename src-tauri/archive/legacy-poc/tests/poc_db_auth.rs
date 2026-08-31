#[path = "../src/core/mod.rs"]
mod core;
#[path = "../src/infrastructure/database.rs"]
mod database;
#[path = "../src/infrastructure/platform_auth.rs"]
mod platform_auth;

use std::path::{Path, PathBuf};
use std::time::Duration;

use core::secret::SecretValue;
use database::{DualMySqlPools, MySqlProjectConfig};
use platform_auth::{PlatformAuthClient, PlatformLoginConfig};
use serde_json::Value;

struct RealTestConfig {
    mysql: MySqlProjectConfig,
    login: PlatformLoginConfig,
}

fn project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri must have project parent")
        .to_path_buf()
}

fn line_value<'a>(text: &'a str, label: &str) -> &'a str {
    text.lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("missing {label} in test data"))
}

fn env_default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = format!("${{{key}:");
    let start = text
        .find(&marker)
        .unwrap_or_else(|| panic!("missing {key} default in platform config"))
        + marker.len();
    let rest = &text[start..];
    let end = rest
        .find('}')
        .unwrap_or_else(|| panic!("unterminated {key} default in platform config"));
    &rest[..end]
}

fn load_real_config() -> RealTestConfig {
    let root = project_root();
    let test_text = std::fs::read_to_string(root.join("test/测试数据说明.txt"))
        .expect("read test data description");
    let json_start = test_text.find('{').expect("platform login json start");
    let json_end = test_text[json_start..]
        .find('}')
        .map(|index| json_start + index + 1)
        .expect("platform login json end");
    let login_json: Value = serde_json::from_str(&test_text[json_start..json_end])
        .expect("parse platform login json");

    let workspace_root = root
        .parent()
        .and_then(Path::parent)
        .expect("resolve workspace root");
    let platform_config = std::fs::read_to_string(
        workspace_root.join(
            "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
        ),
    )
    .expect("read platform development database config");
    let mysql_host = env_default(&platform_config, "MYSQL_HOST");
    let mysql_port = env_default(&platform_config, "MYSQL_PORT")
        .parse::<u16>()
        .expect("parse mysql port");
    let mysql_user = env_default(&platform_config, "MYSQL_USER");
    let mysql_password = env_default(&platform_config, "MYSQL_PASSWORD");

    RealTestConfig {
        mysql: MySqlProjectConfig {
            host: mysql_host.to_string(),
            port: mysql_port,
            username: mysql_user.to_string(),
            password: SecretValue::new(mysql_password),
            platform_schema: line_value(&test_text, "平台业务数据库名：").to_string(),
            workbench_schema: line_value(&test_text, "工作台数据库：").to_string(),
            connect_timeout: Duration::from_secs(10),
        },
        login: PlatformLoginConfig {
            base_url: line_value(&test_text, "平台API：").to_string(),
            principal: login_json["principal"]
                .as_str()
                .expect("platform principal")
                .to_string(),
            credentials: SecretValue::new(
                login_json["credentials"]
                    .as_str()
                    .expect("platform credentials"),
            ),
            session_uuid: SecretValue::new(
                login_json["sessionUUID"]
                    .as_str()
                    .expect("platform session uuid"),
            ),
            image_code: SecretValue::new(
                login_json["imageCode"]
                    .as_str()
                    .expect("platform image code"),
            ),
            timeout: Duration::from_secs(10),
        },
    }
}

#[tokio::test]
#[ignore = "requires the authorized project MySQL environment"]
async fn dual_mysql_pool_and_schema_probe() {
    let config = load_real_config();
    let pools = DualMySqlPools::connect(&config.mysql)
        .await
        .expect("connect dual mysql pools");
    let report = pools
        .probe(&config.mysql)
        .await
        .expect("probe project schemas");
    assert!(report.platform_aio_table_exists);
    assert!(
        report.missing_required_columns.is_empty(),
        "platform schema is missing required aio columns: {:?}",
        report.missing_required_columns
    );
    assert!(report.workbench_charset.is_some());
    pools.close().await;
}

#[tokio::test]
#[ignore = "requires the authorized project platform login environment"]
async fn real_platform_rsa_login() {
    let config = load_real_config();
    let client = PlatformAuthClient::new(config.login.timeout).expect("create auth client");
    let session = client.login(&config.login).await.expect("platform rsa login");
    assert_eq!(session.principal, config.login.principal);
    assert!(!session.access_token.is_empty());
    assert!(!format!("{session:?}").contains(session.access_token.expose()));
}

