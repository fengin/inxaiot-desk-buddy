#[path = "common/project_test_config.rs"]
mod project_test_config;

use inxaiot_desk_buddy_lib::domain::smart_screen::model::*;
use inxaiot_desk_buddy_lib::formal::workbench_store::WorkbenchStore;
use inxaiot_desk_buddy_lib::infrastructure::smart_screen::shared_results::{
    ScreenSharedResults, SharedScreenOperation,
};
use sqlx::ConnectOptions;

#[tokio::test]
#[ignore = "在已授权 142 环境创建并清理独立工作台测试库，不修改平台业务库"]
async fn shared_screen_results_use_existing_tables_and_are_idempotent()
-> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("error")
        .with_test_writer()
        .try_init();
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
    let schema = format!(
        "inxaiot_desk_buddy_screen_{}",
        &uuid::Uuid::now_v7().simple().to_string()[..16]
    );
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
        .execute(&admin)
        .await?;
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let pool = sqlx::mysql::MySqlPoolOptions::new()
            .max_connections(3)
            .connect_with(options.database(&schema))
            .await?;
        let store = WorkbenchStore::new(pool.clone());
        store.migrate().await?;
        store.migrate().await?;
        if !store.schema_status(&schema).await?.is_ready() {
            return Err("共享表结构未就绪".into());
        }
        let repo = ScreenSharedResults::new(pool.clone());
        repo.bind_source("source-a", "business").await?;
        if repo.bind_source("source-b", "business").await.is_ok() {
            return Err("不能混用两个平台数据源".into());
        }
        let id = uuid::Uuid::now_v7().to_string();
        let targets = vec!["business:screen-1".into()];
        let op = SharedScreenOperation {
            id: &id,
            business_project_id: "business",
            action: "inspect",
            name: "智能屏结果验收",
            operator: "test",
            instance_id: "test-instance",
            targets: &targets,
            started_at: None,
        };
        repo.start(&op).await?;
        repo.start(&op).await?;
        if repo.finish(&id, "business").await.is_ok() {
            return Err("未完成目标不能结束操作".into());
        }
        let detail = ScreenTargetResult {
            format_version: 1,
            screen_id: targets[0].clone(),
            device: ResultState::Succeeded,
            business: ResultState::NotRequired,
            shared: ResultState::Pending,
            message: "检查完成".into(),
            ..Default::default()
        };
        if repo
            .save_target(&id, "other-business", &detail, "succeeded")
            .await
            .is_ok()
        {
            return Err("跨项目保存未被阻止".into());
        }
        repo.save_target(&id, "business", &detail, "succeeded")
            .await?;
        repo.save_target(&id, "business", &detail, "succeeded")
            .await?;
        let mut changed = detail;
        changed.message = "替换旧结果".into();
        if repo
            .save_target(&id, "business", &changed, "succeeded")
            .await
            .is_ok()
        {
            return Err("已有结果不应被覆盖".into());
        }
        repo.finish(&id, "business").await?;
        repo.finish(&id, "business").await?;
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_record")
            .fetch_one(&pool)
            .await?;
        let target_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_target_result")
            .fetch_one(&pool)
            .await?;
        let aio_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM aio_node")
            .fetch_one(&pool)
            .await?;
        if count != 1 || target_count != 1 || aio_count != 0 {
            return Err("屏结果不应重复或写入一体机资产".into());
        }
        pool.close().await;
        Ok(())
    }
    .await;
    sqlx::query(&format!("DROP DATABASE `{schema}`"))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}
