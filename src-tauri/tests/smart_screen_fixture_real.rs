#[path = "common/project_test_config.rs"]
mod project_test_config;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use sqlx::ConnectOptions;

#[tokio::test]
#[ignore = "仅供桌面验收脚本创建/清理带标记的独立测试库"]
async fn desktop_fixture() -> Result<(), Box<dyn std::error::Error>> {
    let action = std::env::var("INX_SCREEN_FIXTURE_ACTION")?;
    let business = std::env::var("INX_SCREEN_FIXTURE_BIZ")?;
    let shared = std::env::var("INX_SCREEN_FIXTURE_OPS")?;
    let suffix = business
        .strip_prefix("inxaiot_desk_buddy_ui_b_")
        .filter(|v| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or("业务测试库名称不合法")?;
    if shared != format!("inxaiot_desk_buddy_ui_w_{suffix}") {
        return Err("工作台测试库与业务测试库不配对".into());
    }
    let cfg = project_test_config::database();
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging();
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(2)
        .connect_with(options.clone())
        .await?;
    if action == "prepare" {
        for schema in [&business, &shared] {
            sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
                .execute(&admin)
                .await?;
            sqlx::query(&format!(
                "CREATE TABLE `{schema}`.codex_screen_fixture(nonce CHAR(32) NOT NULL PRIMARY KEY)"
            ))
            .execute(&admin)
            .await?;
            sqlx::query(&format!(
                "INSERT INTO `{schema}`.codex_screen_fixture(nonce) VALUES(?)"
            ))
            .bind(suffix)
            .execute(&admin)
            .await?;
        }
        sqlx::query(&format!("CREATE TABLE `{business}`.smart_terminal_screen LIKE inxvision_iot_dev_demo.smart_terminal_screen")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.t_project_building LIKE inxvision_iot_dev_demo.t_project_building")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'验收楼幢',3),(1001,777001,1000,'验收楼层',4),(1002,777001,1000,'备用楼层',4)")).execute(&admin).await?;
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(2)
            .connect_with(options.database(&shared))
            .await?;
        WorkbenchStore::new(pool.clone()).migrate().await?;
        pool.close().await;
        eprintln!("SCREEN_FIXTURE_READY");
    } else if action == "cleanup" {
        for schema in [&shared, &business] {
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM information_schema.schemata WHERE schema_name=?",
            )
            .bind(schema)
            .fetch_one(&admin)
            .await?;
            if exists == 0 {
                continue;
            }
            let marker: String = sqlx::query_scalar(&format!(
                "SELECT nonce FROM `{schema}`.codex_screen_fixture"
            ))
            .fetch_one(&admin)
            .await?;
            if marker != suffix {
                return Err("测试标记不符，拒绝清理数据库".into());
            }
            sqlx::query(&format!("DROP DATABASE `{schema}`"))
                .execute(&admin)
                .await?;
        }
        eprintln!("SCREEN_FIXTURE_CLEANED");
    } else {
        return Err("未知测试动作".into());
    }
    admin.close().await;
    Ok(())
}
