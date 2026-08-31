use std::path::Path;

use sqlx::Executor;
use sqlx::mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode};

const PREFIX: &str = "inxaiot_desk_buddy_stage75_";

fn default<'a>(text: &'a str, key: &str) -> &'a str {
    let marker = format!("${{{key}:");
    let rest = &text[text.find(&marker).expect("config default") + marker.len()..];
    &rest[..rest.find('}').expect("config default end")]
}

fn schema_name() -> String {
    let value = std::env::var("INX_STAGE75_SCHEMA").expect("INX_STAGE75_SCHEMA");
    assert!(value.starts_with(PREFIX));
    assert!(value.len() <= 64);
    assert!(
        value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    );
    value
}

async fn admin_pool() -> sqlx::MySqlPool {
    let project_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("project");
    let workspace = project_root
        .parent()
        .and_then(Path::parent)
        .expect("workspace");
    let yaml = std::fs::read_to_string(workspace.join(
        "inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml",
    ))
    .expect("platform config");
    let options = MySqlConnectOptions::new()
        .host(default(&yaml, "MYSQL_HOST"))
        .port(default(&yaml, "MYSQL_PORT").parse().expect("mysql port"))
        .username(default(&yaml, "MYSQL_USER"))
        .password(default(&yaml, "MYSQL_PASSWORD"))
        .ssl_mode(MySqlSslMode::Disabled);
    MySqlPoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .expect("connect mysql admin")
}

#[tokio::test]
#[ignore = "creates one explicitly named isolated workbench schema for the stage 7.5-A UI gate"]
async fn prepare_isolated_stage75_schema() {
    let schema = schema_name();
    let pool = admin_pool().await;
    let statement =
        format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci");
    pool.execute(statement.as_str())
        .await
        .expect("create isolated workbench schema");
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("verify isolated schema");
    assert_eq!(exists, 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "drops only the explicitly named isolated workbench schema created by the stage 7.5-A UI gate"]
async fn cleanup_isolated_stage75_schema() {
    let schema = schema_name();
    let pool = admin_pool().await;
    let profile_table = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = ? AND table_name = 'aio_release_profile'",
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("inspect isolated release profile table");
    if profile_table == 1 {
        let profile = sqlx::query_as::<_, (i64, Option<u64>)>(
            format!("SELECT COUNT(*), MAX(version) FROM `{schema}`.`aio_release_profile`").as_str(),
        )
        .fetch_one(&pool)
        .await
        .expect("inspect isolated release profile");
        let audit_count = sqlx::query_scalar::<_, i64>(
            format!(
                "SELECT COUNT(*) FROM `{schema}`.`audit_event` WHERE object_type = 'aio_release_profile'"
            )
            .as_str(),
        )
        .fetch_one(&pool)
        .await
        .expect("inspect isolated release profile audit");
        let migration_count = sqlx::query_scalar::<_, i64>(
            format!("SELECT COUNT(*) FROM `{schema}`.`_sqlx_migrations` WHERE success = 1")
                .as_str(),
        )
        .fetch_one(&pool)
        .await
        .expect("inspect isolated migrations");
        println!(
            "stage75a isolated evidence: profile_count={}, profile_max_version={}, profile_audit_count={}, migration_count={}",
            profile.0,
            profile.1.unwrap_or_default(),
            audit_count,
            migration_count
        );
    }
    let statement = format!("DROP DATABASE IF EXISTS `{schema}`");
    pool.execute(statement.as_str())
        .await
        .expect("drop isolated workbench schema");
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name = ?",
    )
    .bind(&schema)
    .fetch_one(&pool)
    .await
    .expect("verify isolated schema cleanup");
    assert_eq!(exists, 0);
    pool.close().await;
}
