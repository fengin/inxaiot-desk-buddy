#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::domain::smart_screen::model::ScreenFields;
use inxaiot_desk_buddy_lib::formal::local_store::LocalStore;
use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::{
    ScreenRepository, now,
};
use inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform;
use sqlx::ConnectOptions;

fn test_description() -> Result<String, Box<dyn std::error::Error>> {
    Ok(std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("test/测试数据说明.txt"),
    )?)
}
fn test_value<'a>(
    description: &'a str,
    label: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    description
        .lines()
        .find_map(|line| {
            line.trim()
                .trim_start_matches('\u{feff}')
                .strip_prefix(label)
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("测试说明缺少字段：{label}").into())
}

#[tokio::test]
#[ignore = "读取已授权 142 平台资产及空间；只写临时本机库，不修改平台业务数据"]
async fn platform_assets_spaces_cache_and_drafts_round_trip()
-> Result<(), Box<dyn std::error::Error>> {
    let cfg = project_test_config::database();
    if cfg.host != "192.168.3.142" {
        return Err("本轮资产实机测试只允许142环境".into());
    }
    let description = test_description()?;
    let business_db = test_value(&description, "平台业务数据库名：")?;
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .database(business_db)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging();
    let pool = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(2)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET SESSION TRANSACTION READ ONLY")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options)
        .await?;
    let projects = platform::projects(&pool).await?;
    let business = projects.first().ok_or("没有可验证的业务项目")?.id.clone();
    let source = platform::source_id(&pool).await?;
    let (assets, spaces) = platform::read(&pool, &business).await?;
    let expected:i64=sqlx::query_scalar("SELECT COUNT(*) FROM smart_terminal_screen s JOIN t_project_building b ON b.id=s.building_id WHERE s.delete_flag=0 AND b.delete_flag='0' AND b.project_info_id=?").bind(&business).fetch_one(&pool).await?;
    assert_eq!(assets.len(), expected as usize);
    assert!(!assets.is_empty());
    assert!(!spaces.is_empty());
    let expected_spaces: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM t_project_building WHERE delete_flag='0' AND project_info_id=?",
    )
    .bind(&business)
    .fetch_one(&pool)
    .await?;
    assert_eq!(spaces.len(), expected_spaces as usize);
    let empty = platform::read(&pool, "-1").await?;
    assert!(empty.0.is_empty());
    assert!(empty.1.is_empty());
    let first = &assets[0];
    let app: Option<String> =
        sqlx::query_scalar("SELECT app_version FROM smart_terminal_screen WHERE id=?")
            .bind(&first.id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(first.app_version, app);
    let dir = support::evidence_dir("platform-assets")?;
    let path = dir.join("screen.sqlite");
    let store = LocalStore::open(&path).await?;
    sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES('screen-real','屏数据验证','','',3306,'','','inxaiot_desk_buddy','',?,?)").bind(now()).bind(now()).execute(store.pool()).await?;
    let repo = ScreenRepository::new(store.pool().clone());
    repo.set_scope("screen-real", &business, &source).await?;
    repo.replace_platform_cache("screen-real", &business, &assets, &spaces)
        .await?;
    let mut draft = first.fields.clone();
    draft.location = "本机待提交测试位置".into();
    repo.save_draft_checked("screen-real", &first.id, &draft, 1, 0)
        .await?;
    assert!(
        repo.save_draft_checked("screen-real", &first.id, &draft, 1, 0)
            .await
            .is_err()
    );
    repo.save_local(
        "screen-real",
        &ScreenFields {
            name: "本机测试屏".into(),
            ip: "192.0.2.10".into(),
            size: "4".into(),
            space_id: Some(spaces[0].id.clone()),
            ..Default::default()
        },
        None,
        None,
    )
    .await?;
    pool.close().await;
    store.close().await;
    let reopened = LocalStore::open(&path).await?;
    let cached = ScreenRepository::new(reopened.pool().clone())
        .snapshot("screen-real")
        .await?;
    assert_eq!(cached.screens.len(), assets.len() + 1);
    assert_eq!(cached.spaces.len(), spaces.len());
    assert!(!cached.platform_available);
    assert!(cached.platform_read_at.is_some());
    assert_eq!(
        cached.platform_drafts[&first.id].values.location,
        "本机待提交测试位置"
    );
    assert_eq!(
        cached
            .screens
            .iter()
            .find(|s| s.id == first.id)
            .unwrap()
            .fields,
        first.fields
    );
    eprintln!(
        "已核对 {} 台平台屏、{} 个空间；本机新增、草稿及离线重开通过",
        assets.len(),
        spaces.len()
    );
    reopened.close().await;
    std::fs::write(
        dir.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"platformScreenCount":assets.len(),"spaceCount":spaces.len(),"businessWrites":false,"localDraftAndReopen":true}),
        )?,
    )?;
    Ok(())
}

#[tokio::test]
#[ignore = "复现本机项目由6改为142后残留空关联；真实平台登录和只读查询，不修改平台库"]
async fn edited_project_connection_refreshes_unused_screen_scope()
-> Result<(), Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::{
        application::ports::{
            project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
        },
        domain::common::project::{PlatformLoginRequest, ProjectInput},
        infrastructure::{
            project_context::project_pools, smart_screen::assets_service::ScreenAssetsService,
            stage75_adapter::Stage75Adapter,
        },
    };
    let cfg = project_test_config::database();
    if cfg.host != "192.168.3.142" {
        return Err("本轮项目连接测试只允许142环境".into());
    }
    let description = test_description()?;
    let start = description.find('{').ok_or("缺少平台测试登录信息")?;
    let end = start + description[start..].find('}').ok_or("登录测试信息不完整")? + 1;
    let login: serde_json::Value = serde_json::from_str(&description[start..end])?;
    let dir = support::evidence_dir("platform-connection-change")?;
    let state = support::state_at(&dir, false).await;
    let adapter = Stage75Adapter::new(&state);
    let mut input = ProjectInput {
        name: "平台地址修改回归".into(),
        platform_url: "http://192.168.3.6:8055".into(),
        db_host: "192.168.3.6".into(),
        db_port: cfg.port,
        db_user: cfg.username.clone(),
        db_tls_enabled: false,
        db_password: Some(cfg.password.clone()),
        business_db: "inxvision_iot_dev".into(),
        workbench_db: "inxaiot_desk_buddy".into(),
    };
    // 旧6地址仅作为本机历史配置，不连接它；真实登录和读取在更新为142后执行。
    let project = adapter.create_project(input.clone()).await?.project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    repo.set_scope(
        &project,
        "1839512626364936193",
        "8094678c-895c-11f0-8fc1-c2d40b4b4614:inxvision_iot_dev",
    )
    .await?;
    input.platform_url = format!("http://{}", test_value(&description, "平台API：")?);
    input.db_host = cfg.host;
    input.business_db = test_value(&description, "平台业务数据库名：")?.into();
    input.db_password = None;
    adapter.update_project(&project, input).await?;
    adapter
        .login_project(
            &project,
            PlatformLoginRequest {
                username: login["principal"].as_str().unwrap().into(),
                password: login["credentials"].as_str().unwrap().into(),
                session_uuid: login["sessionUUID"].as_str().unwrap().into(),
                image_code: login["imageCode"].as_str().unwrap().into(),
            },
        )
        .await?;
    let snapshot = ScreenAssetsService::new(&state)
        .snapshot(&project, true)
        .await?;
    if !snapshot.platform_available {
        return Err(snapshot
            .platform_message
            .unwrap_or("刷新未恢复".into())
            .into());
    }
    assert!(!snapshot.screens.is_empty());
    assert!(snapshot.spaces_available);
    assert!(snapshot.platform_message.is_none());
    let pools = project_pools(&state, &project).await?;
    let source = platform::source_id(&pools.platform).await?;
    assert_eq!(repo.scope(&project).await?.unwrap().1, source);
    eprintln!(
        "修改项目连接后自动识别142平台：{}台屏、{}个空间；未改平台业务数据",
        snapshot.screens.len(),
        snapshot.spaces.len()
    );
    support::close(state).await;
    std::fs::write(
        dir.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"platformScreenCount":snapshot.screens.len(),"spaceCount":snapshot.spaces.len(),"businessWrites":false,"oldHostContacted":false,"scopeReplaced":true}),
        )?,
    )?;
    Ok(())
}
