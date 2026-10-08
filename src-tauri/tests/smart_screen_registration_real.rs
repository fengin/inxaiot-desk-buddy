#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::{
        project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
    },
    domain::{
        common::project::{PlatformLoginRequest, ProjectInput},
        smart_screen::{
            model::ScreenFields, operation::ScreenOperationInput,
            registration::RegistrationSubmission,
        },
    },
    formal::workbench_store::WorkbenchStore,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{assets_service::ScreenAssetsService, registration, task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
use sqlx::{ConnectOptions, Row};
use std::time::Duration;

async fn start_lock_test(context: &inxaiot_desk_buddy_lib::infrastructure::smart_screen::write_context::ScreenWriteContext, operation: &str, instance: &str, screen: &str) -> Result<(),Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::infrastructure::smart_screen::shared_results::{ScreenSharedResults,SharedScreenOperation};
    ScreenSharedResults::new(context.shared.clone()).start(&SharedScreenOperation { id:operation,business_project_id:&context.business,action:"register",name:"限定写入验收",operator:"test",instance_id:instance,targets:&[format!("{}:{screen}",context.business)],started_at:None }).await?;
    Ok(())
}

async fn wait(
    state: &inxaiot_desk_buddy_lib::formal::app_state::FormalAppState,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let operation = state.task_repository.get(id).await?.operation_type;
    let limit = if operation == "install" { 420 } else { 150 };
    let started = std::time::Instant::now();
    eprintln!("SCREEN_TEST_WAIT action={operation} task={id}");
    let waited = tokio::time::timeout(Duration::from_secs(limit), async {
        loop {
            let task = state.task_repository.get(id).await.unwrap();
            if task.state.is_terminal()
                || task.state
                    == inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    if waited.is_err() {
        let current=state.task_repository.get(id).await?;
        let targets=state.task_repository.targets(id).await?;
        return Err(format!("等待{operation}超过{limit}秒，任务状态={:?}，目标={targets:?}",current.state).into());
    }
    eprintln!("SCREEN_TEST_DONE action={operation} seconds={:.1}",started.elapsed().as_secs_f32());
    Ok(())
}

#[tokio::test]
#[ignore = "授权142环境的隔离业务/工作台测试库；按测试说明真实采集屏MAC；测试后清理两库"]
async fn register_update_and_registered_inspection_share_results_without_touching_h5()
-> Result<(), Box<dyn std::error::Error>> {
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
    let business = format!("inxaiot_desk_buddy_s4b_{suffix}");
    let shared = format!("inxaiot_desk_buddy_s4w_{suffix}");
    sqlx::query(&format!(
        "CREATE DATABASE `{business}` CHARACTER SET utf8mb4"
    ))
    .execute(&admin)
    .await?;
    sqlx::query(&format!("CREATE DATABASE `{shared}` CHARACTER SET utf8mb4"))
        .execute(&admin)
        .await?;
    let temp = tempfile::tempdir()?;
    let state = support::state_at(temp.path(), true).await;
    let result:Result<(),Box<dyn std::error::Error>>=async{
        sqlx::query(&format!("CREATE TABLE `{business}`.smart_terminal_screen LIKE inxvision_iot_dev_demo.smart_terminal_screen")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.t_project_building LIKE inxvision_iot_dev_demo.t_project_building")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.op_device_service_area LIKE inxvision_iot_dev_demo.op_device_service_area")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'验收楼幢',3),(1001,777001,1000,'验收楼层',4)")).execute(&admin).await?;
        let ops=sqlx::mysql::MySqlPoolOptions::new().max_connections(3).connect_with(options.clone().database(&shared)).await?;
        WorkbenchStore::new(ops.clone()).migrate().await?;
        let description=std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("test/测试数据说明.txt"))?;
        let value=|label:&str|description.lines().find_map(|line|line.trim().strip_prefix(label)).unwrap().trim();
        let start=description.find('{').unwrap();let end=start+description[start..].find('}').unwrap()+1;
        let login:serde_json::Value=serde_json::from_str(&description[start..end])?;
        let adapter=Stage75Adapter::new(&state);
        let project=adapter.create_project(ProjectInput{name:"智能屏登记隔离验收".into(),platform_url:format!("http://{}",value("平台API：")),db_host:cfg.host.clone(),db_port:cfg.port,db_user:cfg.username.clone(),db_tls_enabled:false,db_password:Some(cfg.password.clone()),business_db:business.clone(),workbench_db:shared.clone()}).await?.project.id;
        adapter.login_project(&project,PlatformLoginRequest{username:login["principal"].as_str().unwrap().into(),password:login["credentials"].as_str().unwrap().into(),session_uuid:login["sessionUUID"].as_str().unwrap().into(),image_code:login["imageCode"].as_str().unwrap().into()}).await?;
        let service=ScreenAssetsService::new(&state);let loaded=service.snapshot(&project,true).await?;
        if !loaded.platform_available{return Err(loaded.platform_message.unwrap_or("平台读取失败".into()).into());}
        let repo=ScreenRepository::new(state.local_store.pool().clone());
        let local_id=repo.save_local(&project,&ScreenFields{name:"登记测试屏".into(),ip:support::live_screen("4")?.ip,mac:String::new(),size:"4".into(),space_id:Some("1001".into()),location:"本机位置".into()},None,None).await?;
        let preview=registration::preview(&state,&project,vec![local_id.clone()]).await?;
        if preview.items[0].state!="ready"{return Err(preview.items[0].reason.clone().into());}
        if preview.items[0].mac_source!="collected"{return Err("实机MAC未采集成功".into());}
        let task=registration::submit(&state,&project,RegistrationSubmission{preview_id:preview.id.clone(),screen_ids:vec![local_id.clone()],mac_confirmations:Default::default(),space_confirmations:vec![]}).await?;
        wait(&state,&task).await?;
        let result=task_data::read_results(state.local_store.pool(),&project,&task).await?;
        if result.targets[&local_id].business!=inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Succeeded{return Err(result.targets[&local_id].message.clone().into());}
        if state.task_repository.results_protected(&task).await?{return Err("登记完成后结果保护未解除".into());}
        let snapshot=service.snapshot(&project,true).await?;
        if snapshot.screens.len()!=1||snapshot.screens[0].source!="platform"||!snapshot.screens[0].aliases.contains(&local_id){return Err("登记后不是一条有原本机别名的平台记录".into());}
        let platform_id=snapshot.screens[0].id.clone();
        let actual=sqlx::query(&format!("SELECT app_version,version,mac FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if actual.try_get::<Option<String>,_>("app_version")?.is_some()||actual.try_get::<Option<String>,_>("version")?.is_some(){return Err("注册不应填写两个版本字段".into());}
        let history:i64=sqlx::query_scalar("SELECT COUNT(*) FROM operation_record WHERE domain_type='smart_screen'").fetch_one(&ops).await?;
        let audit:i64=sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE domain_type='smart_screen'").fetch_one(&ops).await?;
        if history!=1||audit!=1{return Err(format!("共享历史/审计数量不正确：{history}/{audit}").into());}

        // H5与应用版本不同，普通资料更新必须同时保留。
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET version='light-h5',app_version='older-app' WHERE id=?")).bind(&platform_id).execute(&admin).await?;
        let fresh=service.snapshot(&project,true).await?;let screen=&fresh.screens[0];let mut fields=screen.fields.clone();fields.name="更新后的屏".into();
        repo.save_draft_checked(&project,&platform_id,&fields,screen.revision,0).await?;
        let checked=registration::preview(&state,&project,vec![platform_id.clone()]).await?;
        if checked.items[0].mac_source!="unchanged"{return Err("纯名称更新不应强制读设备".into());}
        // B 编辑和检查期间不加锁；实际提交遇到 A 的锁，返回当前页面可用的确认信息。
        let active:i64=sqlx::query_scalar("SELECT COUNT(*) FROM resource_lease WHERE lease_state='active'").fetch_one(&ops).await?;
        if active!=0{return Err("保存草稿或预览时不应持有共享锁".into());}
        let takeover_context=inxaiot_desk_buddy_lib::infrastructure::smart_screen::write_context::open(&state,&project).await?;
        let source_operation=uuid::Uuid::now_v7().to_string();start_lock_test(&takeover_context,&source_operation,"takeover-source",&platform_id).await?;
        let source_held=inxaiot_desk_buddy_lib::infrastructure::smart_screen::leases::HeldScreenLeases::acquire(&takeover_context,&source_operation,"takeover-source",&[platform_id.clone()],true,false).await?;
        let before_tasks:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_task WHERE domain_type='smart_screen' AND operation_type='register'").fetch_one(state.local_store.pool()).await?;
        let blocked=registration::submit(&state,&project,RegistrationSubmission{preview_id:checked.id.clone(),screen_ids:vec![platform_id.clone()],mac_confirmations:Default::default(),space_confirmations:vec![]}).await;
        let conflicts=match blocked {
            Err(inxaiot_desk_buddy_lib::core::error::AppError::ConfirmationRequired{code,details})=>{
                if code!="SCREEN_TAKEOVER_REQUIRED"{return Err("锁冲突没有独立确认类型".into());}
                serde_json::from_value::<Vec<inxaiot_desk_buddy_lib::infrastructure::smart_screen::takeover::TakeoverConflict>>(details["conflicts"].clone())?
            },
            _=>return Err("提交时未返回可接手的锁冲突".into()),
        };
        if conflicts[0].owner_instance_id!="takeover-source"{return Err("确认信息没有原电脑".into());}
        let after_tasks:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_task WHERE domain_type='smart_screen' AND operation_type='register'").fetch_one(state.local_store.pool()).await?;
        if before_tasks!=after_tasks{return Err("等待接手确认时不应创建执行任务".into());}
        inxaiot_desk_buddy_lib::infrastructure::smart_screen::takeover::release(&state,&project,&conflicts,true).await?;
        if source_held.valid().await.is_ok(){return Err("接手后 A 的原占用仍然有效".into());}
        service.snapshot(&project,true).await?;
        let refreshed=registration::preview(&state,&project,vec![platform_id.clone()]).await?;
        if refreshed.id==checked.id{return Err("接手后没有重新检查资料".into());}
        let checked=refreshed;drop(source_held);
        let update=registration::submit(&state,&project,RegistrationSubmission{preview_id:checked.id,screen_ids:vec![platform_id.clone()],mac_confirmations:Default::default(),space_confirmations:vec![]}).await?;
        wait(&state,&update).await?;
        let record=sqlx::query(&format!("SELECT name,app_version,version FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if record.get::<String,_>("name")!="更新后的屏"||record.get::<String,_>("app_version")!="older-app"||record.get::<String,_>("version")!="light-h5"{return Err("资料更新越过字段范围或未生效".into());}

        // 补齐步骤3：已注册屏真实检查只保存实测/共享摘要，不自动更新平台版本。
        let input=ScreenOperationInput{action:"inspect".into(),target_ids:vec![platform_id.clone()],application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),reinstall:false,concurrency:1,retry_of_operation_id: None, expected_targets:Default::default()};
        let check=tasks::preflight(&state,&project,input.clone()).await?;
        let inspection=tasks::submit(&state,&project,&check.id,input).await?;wait(&state,&inspection).await?;
        if state.task_repository.get(&inspection).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::Succeeded {
            let details=task_data::read_results(state.local_store.pool(),&project,&inspection).await?;
            let summaries=details.targets.values().map(|target|format!("设备={:?}，共享={:?}，{}",target.device,target.shared,target.message)).collect::<Vec<_>>().join("；");
            return Err(format!("已注册屏检查未通过：{summaries}").into());
        }
        let versions:(Option<String>,Option<String>)=sqlx::query_as(&format!("SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if versions!=(Some("older-app".into()),Some("light-h5".into())){return Err("只读检查修改了平台版本".into());}
        let shared_count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM operation_record WHERE domain_type='smart_screen'").fetch_one(&ops).await?;
        if shared_count!=4{return Err(format!("登记、更新、检查及接手来源应有4条共享结果，实际{shared_count}").into());}

        let mut duplicate=repo.asset(&project,&platform_id).await?.fields;duplicate.name="本机合并名称".into();duplicate.location="本机选定位置".into();duplicate.mac=String::new();duplicate.space_id=Some("1000".into());
        let duplicate_id=repo.save_local(&project,&duplicate,None,None).await?;
        let read_input=ScreenOperationInput{action:"inspect".into(),target_ids:vec![duplicate_id.clone()],application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),reinstall:false,concurrency:1,retry_of_operation_id: None, expected_targets:Default::default()};
        let check=tasks::preflight(&state,&project,read_input.clone()).await?;let local_inspection=tasks::submit(&state,&project,&check.id,read_input).await?;wait(&state,&local_inspection).await?;
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET mac='02:aa:bb:cc:dd:ee',point_x='5',point_y='6' WHERE id=?")).bind(&platform_id).execute(&admin).await?;
        let snapshot=service.snapshot(&project,true).await?;
        let mut local_candidate=snapshot.screens.iter().find(|s|s.id==duplicate_id).unwrap().clone();
        local_candidate.app_version=snapshot.observations[&duplicate_id][0].observed_app_version.clone();
        let observed_app_version=local_candidate.app_version.clone().ok_or("没有读取到小新实际版本")?;
        let platform_candidate=snapshot.screens.iter().find(|s|s.id==platform_id).unwrap().clone();
        let candidate=serde_json::json!({"local":local_candidate,"platform":platform_candidate});
        let choices=std::collections::BTreeMap::from([("name".into(),"local".into()),("ip".into(),"platform".into()),("mac".into(),"local".into()),("size".into(),"platform".into()),("space".into(),"local".into()),("location".into(),"local".into()),("appVersion".into(),"local".into())]);
        let decision=inxaiot_desk_buddy_lib::infrastructure::smart_screen::merge::MergeDecision{kind:"merge".into(),choices,identity_confirmed:false};
        if inxaiot_desk_buddy_lib::infrastructure::smart_screen::merge::merge(&state,&project,candidate.clone(),decision.clone()).await.is_ok(){return Err("未确认同一设备时不应合并".into());}
        let merged=inxaiot_desk_buddy_lib::infrastructure::smart_screen::merge::merge(&state,&project,candidate,inxaiot_desk_buddy_lib::infrastructure::smart_screen::merge::MergeDecision{identity_confirmed:true,..decision}).await?.ok_or("缺少合并结果")?;
        if merged.platform_id!=platform_id||merged.result_pending{return Err("合并没有保持原平台编号或结果未保存".into());}
        let merged_space:(String,String,String)=sqlx::query_as(&format!("SELECT CAST(building_id AS CHAR),point_x,point_y FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if merged_space!=("1000".into(),"5".into(),"6".into()){return Err("合并没有独立采用新空间，或修改了原布点".into());}
        let after=service.snapshot(&project,true).await?;
        if after.screens.len()!=1||after.screens[0].aliases.len()!=2||!after.observations.contains_key(&duplicate_id){return Err("合并丢失了身份或本机历史".into());}
        let values:(String,String,String)=sqlx::query_as(&format!("SELECT name,app_version,version FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if values!=("本机合并名称".into(),observed_app_version.clone(),"light-h5".into()){return Err("合并选源或H5保护不正确".into());}
        let old_history:i64=sqlx::query_scalar("SELECT COUNT(*) FROM operation_record WHERE id=?").bind(&local_inspection).fetch_one(&ops).await?;
        if old_history!=0{return Err("合并不应上传未注册阶段的旧检查历史".into());}
        let total:i64=sqlx::query_scalar("SELECT COUNT(*) FROM operation_record WHERE domain_type='smart_screen'").fetch_one(&ops).await?;
        if total!=5{return Err(format!("共享结果数量应为5，实际{total}").into());}

        // 平台已创建但本机关联失败：保留原编号，核实后只补关联和结果。
        let second_id=repo.save_local(&project,&ScreenFields{name:"关联失败测试屏".into(),ip:support::live_screen("10")?.ip,mac:String::new(),size:"10".into(),space_id:Some("1001".into()),location:"故障恢复测试".into()},None,None).await?;
        sqlx::query("CREATE TRIGGER simulate_binding_failure BEFORE INSERT ON local_screen_binding BEGIN SELECT RAISE(ABORT,'simulated binding failure'); END").execute(state.local_store.pool()).await?;
        let check=registration::preview(&state,&project,vec![second_id.clone()]).await?;
        if check.items[0].state!="ready"{return Err(check.items[0].reason.clone().into());}
        let interrupted=registration::submit(&state,&project,RegistrationSubmission{preview_id:check.id,screen_ids:vec![second_id.clone()],mac_confirmations:Default::default(),space_confirmations:vec![]}).await?;wait(&state,&interrupted).await?;
        if state.task_repository.get(&interrupted).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed{return Err("本机关联失败必须保留待核实任务".into());}
        let count_before:i64=sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{business}`.smart_terminal_screen")).fetch_one(&admin).await?;
        let original_intent=repo.intents(&project).await?.into_iter().find(|i|i.screen_id==second_id).ok_or("缺少原请求编号")?;
        if original_intent.state!="submitted"{return Err("本机关联失败不能把请求标为完成".into());}
        if repo.remove_local(&project,&second_id).await.is_ok(){return Err("待核实记录不应被移除".into());}
        sqlx::query("DROP TRIGGER simulate_binding_failure").execute(state.local_store.pool()).await?;
        tokio::time::timeout(Duration::from_secs(5),async{while state.job_supervisor.contains(&interrupted).await{tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
        let observations_before:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
        tasks::verify(&state,&project,&interrupted).await?;
        let count_after:i64=sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{business}`.smart_terminal_screen")).fetch_one(&admin).await?;
        let observations_after:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
        if count_before!=count_after||observations_before!=observations_after{return Err("补存关联不应重新注册或采集设备".into());}
        let recovered=repo.intents(&project).await?.into_iter().find(|i|i.request_id==original_intent.request_id).ok_or("原请求丢失")?;
        if recovered.platform_screen_id!=original_intent.platform_screen_id||recovered.state!="confirmed"{return Err("恢复没有沿用原平台编号".into());}

        // 人工制造主键碰撞，验证限定写入不会认领或覆盖原有平台记录。
        let context=inxaiot_desk_buddy_lib::infrastructure::smart_screen::write_context::open(&state,&project).await?;
        let op=uuid::Uuid::now_v7().to_string();
        start_lock_test(&context,&op,"collision-test",&platform_id).await?;
        let held=inxaiot_desk_buddy_lib::infrastructure::smart_screen::leases::HeldScreenLeases::acquire(&context,&op,"collision-test",&[platform_id.clone()],true,false).await?;
        let collision=inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform_write::insert(&context,&held.grants,&platform_id,&ScreenFields{name:"不得覆盖原记录".into(),ip:"192.0.2.240".into(),mac:String::new(),size:"4".into(),space_id:Some("1001".into()),location:String::new()}).await;
        let other_lock=uuid::Uuid::now_v7().to_string();start_lock_test(&context,&other_lock,"another-computer",&platform_id).await?;
        if inxaiot_desk_buddy_lib::infrastructure::smart_screen::leases::HeldScreenLeases::acquire(&context,&other_lock,"another-computer",&[platform_id.clone()],false,false).await.is_ok(){return Err("另一电脑不应获得同一屏的有效占用".into());}
        if !matches!(collision,Err(inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform_write::MutationError::Rejected(_))){return Err("主键碰撞未被明确拒绝".into());}
        held.release().await?;
        let existing_name:String=sqlx::query_scalar(&format!("SELECT name FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if existing_name!="本机合并名称"{return Err("碰撞请求覆盖了原记录".into());}

        // 预览后被其他人修改的同一字段不覆盖；空间修改不依赖布点。
        let baseline=inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform_write::record(&context.read,&platform_id).await?.ok_or("平台记录缺失")?.asset.fields;
        let mut proposed=baseline.clone();proposed.name="不得覆盖并发修改".into();
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET name='他人修改' WHERE id=?")).bind(&platform_id).execute(&admin).await?;
        let op=uuid::Uuid::now_v7().to_string();start_lock_test(&context,&op,"field-conflict-test",&platform_id).await?;let held=inxaiot_desk_buddy_lib::infrastructure::smart_screen::leases::HeldScreenLeases::acquire(&context,&op,"field-conflict-test",&[platform_id.clone()],true,false).await?;
        let conflict=inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform_write::update(&context,&held.grants,&platform_id,&baseline,&proposed,None,false).await;
        if !matches!(conflict,Err(inxaiot_desk_buddy_lib::infrastructure::smart_screen::platform_write::MutationError::Rejected(_))){return Err("并发字段变化未阻止".into());}
        held.release().await?;
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET name=?,point_x='1',point_y='2' WHERE id=?")).bind(&baseline.name).bind(&platform_id).execute(&admin).await?;
        sqlx::query(&format!("UPDATE `{business}`.t_project_building SET building_image='image',device_cat='[1]',screen_version='full',screen_style_type=1 WHERE id IN (1000,1001)")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.op_device_service_area(building_id,device_id,device_alias_name) VALUES(1001,1,'测试别名')")).execute(&admin).await?;
        let current=service.snapshot(&project,true).await?;
        let screen=current.screens.iter().find(|s|s.id==platform_id).ok_or("待修改屏缺失")?;
        let mut moved=screen.fields.clone();moved.space_id=Some("1001".into());
        repo.save_draft_checked(&project,&platform_id,&moved,screen.revision,0).await?;
        let checked=registration::preview(&state,&project,vec![platform_id.clone()]).await?;
        if checked.items[0].state!="ready"{return Err(format!("已有布点和空间配置不应阻止预览：{}",checked.items[0].reason).into());}
        let movement=registration::submit(&state,&project,RegistrationSubmission{preview_id:checked.id,screen_ids:vec![platform_id.clone()],mac_confirmations:Default::default(),space_confirmations:vec![platform_id.clone()]}).await?;
        wait(&state,&movement).await?;
        if state.task_repository.get(&movement).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::Succeeded{return Err("空间修改任务未成功".into());}
        let actual:(String,String,String)=sqlx::query_as(&format!("SELECT CAST(building_id AS CHAR),point_x,point_y FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if actual!=("1001".into(),"1".into(),"2".into()){return Err("空间未更新或布点坐标被改动".into());}

        // 已注册屏检查完成但共享保存失败：不清理原结果，恢复时不再次连接设备。
        sqlx::raw_sql("CREATE TRIGGER simulate_shared_failure BEFORE INSERT ON operation_target_result FOR EACH ROW SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='simulated shared failure'").execute(&ops).await?;
        let input=ScreenOperationInput{action:"inspect".into(),target_ids:vec![platform_id.clone()],application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),reinstall:false,concurrency:1,retry_of_operation_id:None,expected_targets:Default::default()};
        let check=tasks::preflight(&state,&project,input.clone()).await?;let pending=tasks::submit(&state,&project,&check.id,input).await?;wait(&state,&pending).await?;
        if state.task_repository.get(&pending).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed||!state.task_repository.results_protected(&pending).await?{return Err("共享保存失败没有保护已完成的检查结果".into());}
        let observed_before:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
        state.task_repository.clear_terminal_for_project(&project).await?;
        if state.task_repository.get(&pending).await.is_err(){return Err("待补存结果被清理".into());}
        if adapter.delete_project(&project).await.is_ok(){return Err("存在待保存结果时不应删除项目".into());}
        sqlx::raw_sql("DROP TRIGGER simulate_shared_failure").execute(&ops).await?;
        tokio::time::timeout(Duration::from_secs(5),async{while state.job_supervisor.contains(&pending).await{tokio::time::sleep(Duration::from_millis(20)).await;}}).await?;
        tasks::verify(&state,&project,&pending).await?;
        let observed_after:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation WHERE local_project_id=?").bind(&project).fetch_one(state.local_store.pool()).await?;
        if observed_before!=observed_after||state.task_repository.results_protected(&pending).await?{return Err("共享补存重做了检查或未解除保护".into());}
        let local_start=state.task_repository.get(&pending).await?.started_at.ok_or("没有原开始时间")?;
        let shared_start:time::OffsetDateTime=sqlx::query_scalar("SELECT started_at FROM operation_record WHERE id=?").bind(&pending).fetch_one(&ops).await?;
        if shared_start.unix_timestamp()!=time::OffsetDateTime::from_unix_timestamp_nanos(local_start.parse::<i128>()?)?.unix_timestamp(){return Err("补存把保存时间冒充原操作开始时间".into());}

        // 采集失败的分支用受控设备接口模拟，不探测额外地址。
        let empty_mac=repo.save_local(&project,&ScreenFields{name:"离线登记确认".into(),ip:"192.0.2.220".into(),mac:String::new(),size:"4".into(),space_id:Some("1001".into()),location:String::new()},None,None).await?;
        let unavailable=registration::preview_with_mac(&state,&project,vec![empty_mac.clone()],|_|async{Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict("测试模拟设备未授权".into()))}).await?;
        if unavailable.items[0].required_mac_confirmation.as_deref()!=Some("empty")||unavailable.items[0].state!="ready"{return Err("采集失败没有要求逐屏空MAC确认".into());}
        let submission=RegistrationSubmission{preview_id:unavailable.id,screen_ids:vec![empty_mac.clone()],mac_confirmations:Default::default(),space_confirmations:vec![]};
        if registration::submit(&state,&project,submission.clone()).await.is_ok(){return Err("未确认空MAC就提交了登记".into());}
        let confirmed=registration::submit(&state,&project,RegistrationSubmission{mac_confirmations:std::collections::BTreeMap::from([(empty_mac.clone(),"empty".into())]),..submission}).await?;wait(&state,&confirmed).await?;
        if state.task_repository.get(&confirmed).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::Succeeded{return Err("空MAC确认后的登记失败".into());}
        let historical=repo.save_local(&project,&ScreenFields{name:"历史MAC确认".into(),ip:"192.0.2.221".into(),mac:"02:11:22:33:44:55".into(),size:"4".into(),space_id:Some("1001".into()),location:String::new()},None,None).await?;
        let history=registration::preview_with_mac(&state,&project,vec![historical.clone()],|_|async{Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict("测试模拟超时".into()))}).await?;
        if history.items[0].required_mac_confirmation.as_deref()!=Some("existing"){return Err("可信历史MAC未要求确认".into());}
        let conflict=registration::preview_with_mac(&state,&project,vec![historical],|target|async move{Ok(inxaiot_desk_buddy_lib::domain::smart_screen::model::ScreenObservation{id:uuid::Uuid::now_v7().to_string(),screen_id:target.id,observed_ip:target.fields.ip,observed_at:inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::now(),operation_type:"mac".into(),observed_mac:Some("02:11:22:33:44:56".into()),mac_candidates:vec!["eth0=02:11:22:33:44:56".into()],..Default::default()})}).await?;
        if conflict.items[0].state!="blocked"||conflict.items[0].required_mac_confirmation.is_some(){return Err("身份冲突不应允许空MAC绕过".into());}

        // 独立版本同步与状态覆盖不修改资料草稿、H5或其他业务字段。
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET app_version='stale-record' WHERE id=?")).bind(&platform_id).execute(&admin).await?;
        let current=service.snapshot(&project,true).await?;let current_screen=current.screens.iter().find(|s|s.id==platform_id).unwrap();
        let mut pending_name=current_screen.fields.clone();pending_name.name="仍待提交的名称".into();
        repo.save_draft_checked(&project,&platform_id,&pending_name,current_screen.revision,0).await?;
        let version_preview=inxaiot_desk_buddy_lib::infrastructure::smart_screen::value_updates::version_preview(&state,&project,vec![platform_id.clone()]).await?;
        if version_preview.items[0].state!="ready"||version_preview.items[0].device_version.as_deref()!=Some(observed_app_version.as_str()){return Err(version_preview.items[0].reason.clone().into());}
        let version_task=inxaiot_desk_buddy_lib::infrastructure::smart_screen::value_updates::version_submit(&state,&project,&version_preview.id,vec![platform_id.clone()]).await?;wait(&state,&version_task).await?;
        if state.task_repository.get(&version_task).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::Succeeded{return Err("独立版本同步未完成".into());}
        let after_sync=repo.snapshot(&project).await?;
        if after_sync.platform_drafts[&platform_id].values.name!="仍待提交的名称"||after_sync.platform_drafts[&platform_id].base.name=="仍待提交的名称"{return Err("版本同步改变了资料草稿".into());}
        let versions:(String,String)=sqlx::query_as(&format!("SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if versions!=(observed_app_version.clone(),"light-h5".into()){return Err("独立同步没有区分应用与H5版本".into());}
        repo.discard_draft(&project,&platform_id,after_sync.platform_drafts[&platform_id].revision).await?;
        sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET status=0 WHERE id=?")).bind(&platform_id).execute(&admin).await?;
        let status_snapshot=service.snapshot(&project,true).await?;let target=status_snapshot.screens.iter().find(|s|s.id==platform_id).unwrap();
        let status_result=inxaiot_desk_buddy_lib::infrastructure::smart_screen::value_updates::cover_status(&state,&project,vec![inxaiot_desk_buddy_lib::infrastructure::smart_screen::value_updates::StatusChange{id:platform_id.clone(),ip:target.fields.ip.clone(),expected:"offline".into(),next:"online".into(),revision:target.revision}]).await?;
        if !status_result[0].ok{return Err(status_result[0].message.clone().into());}
        let status:String=sqlx::query_scalar(&format!("SELECT CAST(status AS CHAR) FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;
        if status!="1"{return Err("状态覆盖未生效".into());}
        let history=inxaiot_desk_buddy_lib::infrastructure::smart_screen::history::list(&state,&project,&inxaiot_desk_buddy_lib::domain::common::operation_history::OperationHistoryQuery{page:1,page_size:100,operation_type:None,state:None}).await?;
        if !history.items.iter().any(|record|record.id==version_task){return Err("项目共享历史缺少版本同步".into());}
        let detail=inxaiot_desk_buddy_lib::infrastructure::smart_screen::history::detail(&state,&project,&version_task).await?;
        if detail.targets[0].details.as_ref().and_then(|d|d["business"].as_str())!=Some("succeeded"){return Err("共享历史没有分开保存业务结果".into());}
        let other=uuid::Uuid::now_v7().to_string();
        sqlx::query("INSERT INTO operation_record(id,domain_type,operation_type,operation_name,operator_name,instance_id,business_project_id,state,started_at,heartbeat_at) VALUES(?,'smart_screen','inspect','其他项目测试','fixture','fixture','888002','succeeded',UTC_TIMESTAMP(6),UTC_TIMESTAMP(6))").bind(&other).execute(&ops).await?;
        let scoped=inxaiot_desk_buddy_lib::infrastructure::smart_screen::history::list(&state,&project,&inxaiot_desk_buddy_lib::domain::common::operation_history::OperationHistoryQuery{page:1,page_size:100,operation_type:None,state:None}).await?;
        if scoped.total!=history.total||inxaiot_desk_buddy_lib::infrastructure::smart_screen::history::detail(&state,&project,&other).await.is_ok(){return Err("共享历史跨业务项目泄漏".into());}
        if std::env::var("INX_SCREEN_TEST_INSTALL").as_deref()==Ok("1"){
            use inxaiot_desk_buddy_lib::infrastructure::smart_screen::{apk,maintenance,device::{AdbDevice,AndroidTools}};
            use tokio_util::sync::CancellationToken;
            sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET app_version='before-install' WHERE id=?")).bind(&platform_id).execute(&admin).await?;
            let snapshot=service.snapshot(&project,true).await?;let screen=snapshot.screens.iter().find(|s|s.id==platform_id).unwrap();let ip=screen.fields.ip.clone();
            let package=apk::inspect(&support::live_apk_path()?,CancellationToken::new()).await?;
            let input=ScreenOperationInput{action:"install".into(),target_ids:vec![platform_id.clone()],application_id:Some("xiaoxin".into()),apk:Some(serde_json::to_value(&package)?),app_version:package.app_version.clone(),abi:"universal".into(),reinstall:true,concurrency:1,retry_of_operation_id: None, expected_targets:Default::default()};
            let check=maintenance::preflight(&state,&project,input.clone()).await?;if check.items[0].state!="ready"{return Err(check.items[0].reason.clone().into());}
            sqlx::raw_sql("CREATE TRIGGER fixture_fail_install_audit BEFORE INSERT ON audit_event FOR EACH ROW BEGIN IF NEW.action='install' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='fixture install audit failure'; END IF; END").execute(&ops).await?;
            let task=maintenance::submit(&state,&project,&check.id,input).await?;wait(&state,&task).await?;
            let results=task_data::read_results(state.local_store.pool(),&project,&task).await?;let row=&results.targets[&platform_id];
            if state.task_repository.get(&task).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed||row.device!=inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Succeeded||row.business!=inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Succeeded{return Err(format!("安装分段结果不符：{}",row.message).into());}
            let device=AdbDevice::new(AndroidTools::discover()?);
            let before=device.shell(&ip,&["dumpsys","package","chat.xiaoxin.app"],CancellationToken::new()).await?.lines().find(|line|line.trim().starts_with("lastUpdateTime=")).unwrap_or("").to_string();
            sqlx::raw_sql("DROP TRIGGER fixture_fail_install_audit").execute(&ops).await?;
            tokio::time::timeout(Duration::from_secs(5),async{while state.job_supervisor.contains(&task).await{tokio::time::sleep(Duration::from_millis(50)).await;}}).await?;
            tasks::verify(&state,&project,&task).await?;
            let after=device.shell(&ip,&["dumpsys","package","chat.xiaoxin.app"],CancellationToken::new()).await?.lines().find(|line|line.trim().starts_with("lastUpdateTime=")).unwrap_or("").to_string();
            if before.is_empty()||before!=after{return Err("补存结果时不应再次安装应用".into());}
            if state.task_repository.get(&task).await?.state!=inxaiot_desk_buddy_lib::domain::common::task::TaskState::Succeeded{return Err("安装共享结果未完成补存".into());}
            let versions:(String,String)=sqlx::query_as(&format!("SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=?")).bind(&platform_id).fetch_one(&admin).await?;if versions!=(package.app_version.clone(),"light-h5".into()){return Err("安装登记误改H5版本".into());}
            let audits:i64=sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE action='install'").fetch_one(&ops).await?;if audits!=1{return Err("安装业务修改记录未按原任务补存一次".into());}
            eprintln!("注册屏实机覆盖安装：设备成功、业务登记成功、共享保存失败分别展示；补存未再次安装，H5未变");
        }
        adapter.delete_project(&project).await?;
        ops.close().await;
        eprintln!("登记、更新、合并、碰撞、本机关联失败恢复及已注册屏共享失败补存均通过；H5保留，未重复登记/检查，旧本机历史未上传");
        Ok(())
    }.await;
    support::close(state).await;
    sqlx::query(&format!("DROP DATABASE `{shared}`"))
        .execute(&admin)
        .await?;
    sqlx::query(&format!("DROP DATABASE `{business}`"))
        .execute(&admin)
        .await?;
    admin.close().await;
    result
}
