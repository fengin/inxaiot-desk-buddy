#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::{
        project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
    },
    domain::{
        common::{
            project::{PlatformLoginRequest, ProjectInput},
            task::TaskState,
        },
        smart_screen::{
            model::{ResultState, ScreenAsset, ScreenFields},
            operation::ScreenOperationInput,
        },
    },
    formal::{app_state::FormalAppState, workbench_store::WorkbenchStore},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        project_context::project_pools,
        smart_screen::{
            assets_service::ScreenAssetsService,
            device::{AdbDevice, AndroidTools},
            task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
    interface::commands::task_activity::request_task_cancel,
};
use sqlx::ConnectOptions;
use std::{error::Error, time::Duration};
type TestResult<T = ()> = Result<T, Box<dyn Error>>;
fn input(action: &str, ids: Vec<String>) -> ScreenOperationInput {
    ScreenOperationInput {
        action: action.into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 1,
        retry_of_operation_id: None, expected_targets: Default::default(),
    }
}
async fn wait(state: &FormalAppState, id: &str) -> TestResult<TaskState> {
    Ok(tokio::time::timeout(Duration::from_secs(150), async {
        loop {
            let current = state.task_repository.get(id).await.unwrap().state;
            if current.is_terminal() || current == TaskState::FinalizingFailed {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?)
}
async fn inspect(state: &FormalAppState, project: &str, ids: Vec<String>) -> TestResult<String> {
    let request = input("inspect", ids);
    let check = tasks::preflight(state, project, request.clone()).await?;
    Ok(tasks::submit(state, project, &check.id, request).await?)
}

#[tokio::test]
#[ignore = "142隔离库、114屏只读检查；分别关闭测试业务/共享连接、注销测试会话，验证只读降级、补存、部分失败和取消"]
async fn dependencies_fail_independently_and_read_only_results_recover() -> TestResult {
    let cfg = project_test_config::database();
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging();
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone())
        .await?;
    let suffix = uuid::Uuid::now_v7().simple().to_string();
    let business = format!("inxaiot_desk_buddy_s7ab_{suffix}");
    let shared = format!("inxaiot_desk_buddy_s7aw_{suffix}");
    for schema in [&business, &shared] {
        sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
            .execute(&admin)
            .await?;
    }
    let temp = tempfile::tempdir()?;
    let state = support::state_at(temp.path(), true).await;
    let result:TestResult<serde_json::Value>=async {
        let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();let text=std::fs::read_to_string(root.join("test/测试数据说明.txt"))?;
        let value=|label:&str|text.lines().find_map(|line|line.trim().strip_prefix(label)).unwrap().trim();
        let start=text.find('{').unwrap();let end=start+text[start..].find('}').unwrap()+1;let login:serde_json::Value=serde_json::from_str(&text[start..end])?;
        let login_request=||PlatformLoginRequest { username:login["principal"].as_str().unwrap().into(),password:login["credentials"].as_str().unwrap().into(),session_uuid:login["sessionUUID"].as_str().unwrap().into(),image_code:login["imageCode"].as_str().unwrap().into() };
        let mut asset=ScreenAsset { id:"7001".into(),source:"platform".into(),fields:ScreenFields {name:"步骤7访问条件测试屏".into(),ip:value("4寸屏：").split(':').next().unwrap().into(),mac:String::new(),size:"4".into(),space_id:Some("1001".into()),location:"隔离验收".into()},..Default::default() };
        let observed=AdbDevice::new(AndroidTools::discover()?).inspect(&asset,"mac",tokio_util::sync::CancellationToken::new()).await?;
        asset.fields.mac=observed.observed_mac.ok_or("未采集到测试屏MAC")?;
        sqlx::query(&format!("CREATE TABLE `{business}`.smart_terminal_screen LIKE inxvision_iot_dev_demo.smart_terminal_screen")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.t_project_building LIKE inxvision_iot_dev_demo.t_project_building")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1001,777001,0,'访问条件测试空间',3)")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,app_version,version,status,delete_flag) VALUES(7001,?,?,?,'4-inch',1001,'隔离验收','unchanged','keep-h5',0,0)")).bind(&asset.fields.name).bind(&asset.fields.ip).bind(&asset.fields.mac).execute(&admin).await?;
        let ops=sqlx::mysql::MySqlPoolOptions::new().max_connections(2).connect_with(options.database(&shared)).await?;WorkbenchStore::new(ops.clone()).migrate().await?;
        let adapter=Stage75Adapter::new(&state);
        let project=adapter.create_project(ProjectInput {name:"步骤7依赖与取消验收".into(),platform_url:format!("http://{}",value("平台API：")),db_host:cfg.host.clone(),db_port:cfg.port,db_user:cfg.username.clone(),db_tls_enabled:false,db_password:Some(cfg.password.clone()),business_db:business.clone(),workbench_db:shared.clone()}).await?.project.id;
        adapter.login_project(&project,login_request()).await?;
        let service=ScreenAssetsService::new(&state);if !service.snapshot(&project,true).await?.platform_available{return Err("初始平台不可用".into());}
        let mut checks=Vec::new();
        for dependency in ["business","shared","session"] {
            eprintln!("STEP7_ACCESS_CHECK {dependency}");
            let pools=project_pools(&state,&project).await?;
            match dependency {"business"=>pools.platform.close().await,"shared"=>pools.workbench.close().await,_=>{adapter.logout_project(&project).await?;}}
            let snapshot=service.snapshot(&project,true).await?;
            if snapshot.screens.len()!=1{return Err("依赖失败时丢失缓存屏".into());}
            if snapshot.platform_available!=(dependency=="shared"){return Err(format!("{dependency}失败影响了错误的读取范围").into());}
            let write=tasks::preflight(&state,&project,input("time",vec!["7001".into()])).await;
            if write.is_ok_and(|check|check.items.iter().any(|item|item.state=="ready")){return Err("依赖失效时仍开放注册屏写操作".into());}
            let local=ScreenRepository::new(state.local_store.pool().clone());
            let local_id=local.save_local(&project,&ScreenFields{name:"混合批次本机屏".into(),ip:"192.0.2.251".into(),size:"4".into(),..Default::default()},None,None).await?;
            let mixed=tasks::preflight(&state,&project,input("time",vec!["7001".into(),local_id.clone()])).await;
            if !mixed.is_err_and(|e|e.to_string().contains("整批未执行")){return Err("混合批次没有统一阻止并提示另选本机屏".into());}
            local.remove_local(&project,&local_id).await?;
            let id=inspect(&state,&project,vec!["7001".into()]).await?;
            if wait(&state,&id).await?!=TaskState::FinalizingFailed{return Err(format!("{dependency}失败时只读检查未保留待补存结果").into());}
            let before=task_data::read_results(state.local_store.pool(),&project,&id).await?;
            if before.targets["7001"].device!=ResultState::Succeeded{return Err(before.targets["7001"].message.clone().into());}
            let count_before:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
            state.runtime_registry.close(&project).await?;adapter.login_project(&project,login_request()).await?;service.snapshot(&project,true).await?;
            tasks::verify(&state,&project,&id).await?;
            if state.task_repository.get(&id).await?.state!=TaskState::Succeeded{return Err("恢复后未成功补存".into());}
            let count_after:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
            if count_before!=count_after{return Err("补存重复读取了设备".into());}
            checks.push(format!("{dependency}独立失败：缓存保留、注册屏写入受阻、真实只读成功；恢复仅补存"));
            eprintln!("STEP7_ACCESS_DONE {dependency}");
        }
        let repo=ScreenRepository::new(state.local_store.pool().clone());
        let bad=repo.save_local(&project,&ScreenFields {name:"步骤7不可达目标".into(),ip:"192.0.2.254".into(),mac:String::new(),size:"4".into(),space_id:None,location:String::new()},None,None).await?;
        eprintln!("STEP7_ACCESS_CHECK partial_failure");
        let partial=inspect(&state,&project,vec!["7001".into(),bad.clone()]).await?;
        if wait(&state,&partial).await?!=TaskState::PartiallySucceeded{return Err("一个目标不可达时未返回部分成功".into());}
        let results=task_data::read_results(state.local_store.pool(),&project,&partial).await?;
        if results.targets["7001"].device!=ResultState::Succeeded||results.targets[&bad].device!=ResultState::Failed{return Err("逐台结果与实际可达性不一致".into());}
        let cancelled=inspect(&state,&project,vec![bad,"7001".into()]).await?;request_task_cancel(&state,&cancelled).await.map_err(|error|format!("只读任务取消请求失败：{} {}",error.code,error.params.get("summary").cloned().unwrap_or_default()))?;
        if !matches!(wait(&state,&cancelled).await?,TaskState::Cancelled|TaskState::PartiallySucceeded){return Err("取消未收敛到准确终态".into());}
        checks.push("真实可达/不可达混合检查分别保存逐台结果，取消阻止后续目标".into());
        let versions:(String,String)=sqlx::query_as(&format!("SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=7001")).fetch_one(&admin).await?;
        if versions!=("unchanged".into(),"keep-h5".into()){return Err("只读流程修改了平台版本".into());}
        adapter.delete_project(&project).await?;ops.close().await;
        Ok(serde_json::json!({"passed":true,"checks":checks}))
    }.await;
    support::close(state).await;
    for schema in [&shared, &business] {
        sqlx::query(&format!("DROP DATABASE `{schema}`"))
            .execute(&admin)
            .await?;
    }
    admin.close().await;
    let evidence = result?;
    if let Some(path) = std::env::var_os("INX_SCREEN_STEP7_EVIDENCE") {
        std::fs::create_dir_all(&path)?;
        std::fs::write(
            std::path::PathBuf::from(path).join("access-and-cancel.json"),
            serde_json::to_vec_pretty(&evidence)?,
        )?;
    }
    println!("SCREEN_STEP7_ACCESS_PASS {}", evidence["checks"]);
    Ok(())
}
