#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/mysql_v2.rs"]
mod mysql;

use std::path::Path;
use std::time::Duration;

use mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};

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

fn config() -> MySqlConnectionSpec {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("project");
    let description =
        std::fs::read_to_string(project.join("test/测试数据说明.txt")).expect("test data");
    let workspace = project.parent().and_then(Path::parent).expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    MySqlConnectionSpec {
        host: default(&yaml, "MYSQL_HOST").into(),
        port: default(&yaml, "MYSQL_PORT").parse().expect("mysql port"),
        username: default(&yaml, "MYSQL_USER").into(),
        password: default(&yaml, "MYSQL_PASSWORD").into(),
        platform_schema: line(&description, "平台业务数据库名：").into(),
        workbench_schema: line(&description, "工作台数据库：").into(),
        tls_mode: MySqlTlsMode::Disabled,
        connect_timeout: Duration::from_secs(10),
    }
}

#[tokio::test]
#[ignore = "read-only inspection of isolated phase3 rows"]
async fn print_exact_poc_profile_keys() {
    let config = config();
    let pools = ProjectMySqlPools::connect(&config)
        .await
        .expect("connect project mysql");
    let keys = sqlx::query_scalar::<_, String>(
        "SELECT profile_key FROM aio_release_profile WHERE profile_key LIKE 'poc-%' ORDER BY profile_key",
    )
    .fetch_all(pools.workbench())
    .await
    .expect("read poc profile keys");
    println!("phase3_poc_keys={keys:?}");
    pools.close().await;
}

