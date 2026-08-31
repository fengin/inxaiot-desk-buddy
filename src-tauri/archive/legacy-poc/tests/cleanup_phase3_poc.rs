#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/mysql_v2.rs"]
mod mysql;

use std::path::Path;
use std::time::Duration;

use mysql::{MySqlConnectionSpec, MySqlTlsMode, ProjectMySqlPools};

const EXACT_PROFILE_KEY: &str = "poc-01a041bf4a3b77438ccf";

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
#[ignore = "removes the exact isolated profile left by the previous failed assertion"]
async fn remove_exact_profile_and_its_audit_rows() {
    let pools = ProjectMySqlPools::connect(&config())
        .await
        .expect("connect project mysql");
    let mut transaction = pools.workbench().begin().await.expect("begin cleanup");
    sqlx::query(
        "DELETE FROM audit_event WHERE object_type = 'aio_release_profile' AND object_key = ?",
    )
    .bind(EXACT_PROFILE_KEY)
    .execute(&mut *transaction)
    .await
    .expect("delete exact audit rows");
    sqlx::query("DELETE FROM aio_release_profile WHERE profile_key = ?")
        .bind(EXACT_PROFILE_KEY)
        .execute(&mut *transaction)
        .await
        .expect("delete exact profile");
    transaction.commit().await.expect("commit cleanup");
    let remaining: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM aio_release_profile WHERE profile_key = ?) + \
         (SELECT COUNT(*) FROM audit_event WHERE object_type = 'aio_release_profile' AND object_key = ?)",
    )
    .bind(EXACT_PROFILE_KEY)
    .bind(EXACT_PROFILE_KEY)
    .fetch_one(pools.workbench())
    .await
    .expect("verify cleanup");
    assert_eq!(remaining, 0);
    pools.close().await;
}

