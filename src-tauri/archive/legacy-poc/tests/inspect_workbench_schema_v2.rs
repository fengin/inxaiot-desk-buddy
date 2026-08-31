#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/mysql_v2.rs"]
mod mysql;

use std::path::Path;
use std::time::Duration;

use mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};
use sqlx::Row;

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
#[ignore = "read-only post-migration audit"]
async fn migration_two_and_all_isolated_rows_are_clean() {
    let pools = ProjectMySqlPools::connect(&config())
        .await
        .expect("connect project mysql");
    let migrations: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(pools.workbench())
    .await
    .expect("migration count");
    assert_eq!(migrations, 2);
    let lease_state_column: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns WHERE table_schema = DATABASE() \
         AND table_name = 'resource_lease' AND column_name = 'lease_state'",
    )
    .fetch_one(pools.workbench())
    .await
    .expect("lease state column");
    assert_eq!(lease_state_column, 1);
    let rows = sqlx::query(
        "SELECT \
         (SELECT COUNT(*) FROM aio_release_profile WHERE profile_key LIKE 'poc-%') AS profiles, \
         (SELECT COUNT(*) FROM audit_event WHERE operator_name = 'phase3-test') AS audits, \
         (SELECT COUNT(*) FROM resource_lease WHERE resource_key LIKE 'poc:%') AS leases",
    )
    .fetch_one(pools.workbench())
    .await
    .expect("isolated row audit");
    assert_eq!(rows.try_get::<i64, _>("profiles").expect("profiles"), 0);
    assert_eq!(rows.try_get::<i64, _>("audits").expect("audits"), 0);
    assert_eq!(rows.try_get::<i64, _>("leases").expect("leases"), 0);
    pools.close().await;
}

