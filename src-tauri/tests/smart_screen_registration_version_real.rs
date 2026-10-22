//! 只使用随机隔离 MySQL 库、临时本机状态和合成设备观测，不连接设备或平台 HTTP。
#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::{
        project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
    },
    domain::{
        common::{project::ProjectInput, task::TaskState},
        smart_screen::{model::*, operation::ScreenResults, registration::*},
    },
    formal::{project_repository::LocalProjectRepository, workbench_store::WorkbenchStore},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            assets_service::ScreenAssetsService,
            leases::HeldScreenLeases,
            platform_write, registration,
            shared_results::{ScreenSharedResults, SharedScreenOperation},
            task_data, tasks, write_context,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use sqlx::ConnectOptions;
type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
fn check(condition: bool, message: &str) -> TestResult {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn observed(target: &ScreenAsset, version: &str) -> ScreenObservation {
    ScreenObservation {
        id: uuid::Uuid::now_v7().to_string(),
        screen_id: target.id.clone(),
        operation_type: "inspect".into(),
        observed_ip: target.fields.ip.clone(),
        observed_at: inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::now(),
        adb_available: true,
        observed_mac: Some(target.fields.mac.clone()),
        mac_candidates: vec![format!("eth0={}", target.fields.mac)],
        app_installed: Some(true),
        package_id: Some("chat.xiaoxin.app".into()),
        observed_app_version: Some(version.into()),
        app_version_code: Some(18031),
        ..Default::default()
    }
}

async fn shared_operation(
    context: &write_context::ScreenWriteContext,
    task: &str,
    instance: &str,
    id: &str,
) -> TestResult {
    ScreenSharedResults::new(context.shared.clone())
        .start(&SharedScreenOperation {
            id: task,
            business_project_id: &context.business,
            action: "register",
            name: "版本注册隔离回归",
            operator: "fixture",
            instance_id: instance,
            targets: &[format!("{}:{id}", context.business)],
            started_at: None,
        })
        .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "仅INX_SCREEN_REGISTRATION_VERSION_SQL=YES；授权142随机隔离库，无设备/平台HTTP/生产表访问"]
async fn registration_version_preview_atomic_write_and_recovery() -> TestResult {
    check(
        std::env::var("INX_SCREEN_REGISTRATION_VERSION_SQL").as_deref() == Ok("YES"),
        "未获得隔离SQL测试门禁",
    )?;
    let cfg = project_test_config::database();
    check(cfg.host == "192.168.3.142", "仅允许测试说明中的142测试主机")?;
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
    let business = format!("inxaiot_regvb_{suffix}");
    let shared = format!("inxaiot_regvw_{suffix}");
    let mut created = Vec::new();
    let temp = tempfile::tempdir()?;
    let state = support::state_at(temp.path(), false).await;
    let result:TestResult=async {
        for schema in [&business,&shared] {
            sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4")).execute(&admin).await?;
            created.push(schema.clone());
        }
        // 自包含最小平台契约，不从任何现存业务库复制结构或行。
        sqlx::query(&format!("CREATE TABLE `{business}`.t_project_building (id BIGINT PRIMARY KEY,project_info_id BIGINT NOT NULL,parent_id BIGINT NOT NULL,area_name VARCHAR(64),area_level INT,building_sort INT DEFAULT 0,delete_flag CHAR(1) DEFAULT '0') ENGINE=InnoDB")).execute(&admin).await?;
        sqlx::query(&format!("CREATE TABLE `{business}`.smart_terminal_screen (id BIGINT PRIMARY KEY,name VARCHAR(64) NOT NULL,ip VARCHAR(32) NOT NULL,mac VARCHAR(32),size VARCHAR(16) NOT NULL,building_id BIGINT,install_address VARCHAR(64) NOT NULL DEFAULT '',app_version VARCHAR(32) DEFAULT 'schema-default',version VARCHAR(32) DEFAULT 'h5-unchanged',status INT DEFAULT 0,delete_flag INT DEFAULT 0) ENGINE=InnoDB")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'隔离楼幢',3),(1001,777001,1000,'隔离楼层',4)")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,app_version) VALUES(100,'版本差异屏','192.0.2.10','02:11:22:33:44:55','4-inch',1001,'2.0.10')")).execute(&admin).await?;
        let ops=sqlx::mysql::MySqlPoolOptions::new().max_connections(3).connect_with(options.clone().database(&shared)).await?;
        WorkbenchStore::new(ops.clone()).migrate().await?;
        let project=Stage75Adapter::new(&state).create_project(ProjectInput { name:"注册版本隔离测试".into(), platform_url:"http://127.0.0.1:9".into(),
            db_host:cfg.host.clone(),db_port:cfg.port,db_user:cfg.username.clone(),db_tls_enabled:false,db_password:Some(cfg.password.clone()),business_db:business.clone(),workbench_db:shared.clone(),
        }).await?.project.id;
        // 在临时本机库建立合成有效会话；不向真实平台登录。
        LocalProjectRepository::new(state.local_store.pool().clone(),state.secret_store.clone())
            .save_session(&project,"fixture","isolated-synthetic-session",Some((time::OffsetDateTime::now_utc().unix_timestamp()+3600).to_string())).await?;
        let service=ScreenAssetsService::new(&state);
        let snapshot=service.snapshot(&project,true).await?;
        if !snapshot.platform_available {
            return Err(snapshot.platform_message.clone().unwrap_or_else(|| "隔离平台读取不可用".into()).into());
        }
        let screen=snapshot.screens.iter().find(|s|s.id=="100").ok_or("合成屏缺失")?.clone();
        let repo=ScreenRepository::new(state.local_store.pool().clone());
        repo.append_observation(&project,&observed(&screen,"2.1.3-review-test")).await?;
        let preview=registration::preview_with_mac(&state,&project,vec![screen.id.clone()],|s|async move{Ok(observed(&s,"2.1.3-review-test"))}).await?;
        let item=&preview.items[0];
        check(item.state=="ready"&&item.diffs.len()==1&&item.diffs[0].field=="appVersion","六字段相同的版本差异未进入ready预览")?;
        check(item.platform_app_version.as_deref()==Some("2.0.10")&&item.proposed_app_version.as_deref()==Some("2.1.3-review-test"),"预览版本前后值错误")?;
        let (plan,_)=task_data::read_preflight_plan(state.local_store.pool(),&project,&preview.id).await?;
        let proof:RegistrationPlan=serde_json::from_value(plan.detail)?;
        check(proof.registration_versions[&screen.id].observation.app_version_code==Some(18031),"计划未保存现场版本证据")?;
        let same=registration::preview_with_mac(&state,&project,vec![screen.id.clone()],|s|async move{Ok(observed(&s,"2.0.10"))}).await?;
        check(same.items[0].state=="skip"&&same.items[0].proposed_app_version.is_none(),"现场版本同值不应产生写入")?;
        repo.append_observation(&project,&observed(&screen,"2.1.3-review-test")).await?;
        let unavailable=registration::preview_with_mac(&state,&project,vec![screen.id.clone()],|_|async{Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict("合成离线".into()))}).await?;
        check(unavailable.items[0].state=="blocked"&&!unavailable.items[0].version_message.is_empty(),"版本读取失败不应无提示skip")?;
        let mut edited=screen.fields.clone(); edited.name="离线资料修改".into();
        repo.save_draft_checked(&project,&screen.id,&edited,screen.revision,0).await?;
        let offline=registration::preview_with_mac(&state,&project,vec![screen.id.clone()],|_|async{Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict("合成离线".into()))}).await?;
        check(offline.items[0].state=="ready"&&offline.items[0].proposed_app_version.is_none()&&offline.items[0].diffs.iter().all(|d|d.field!="appVersion"),"普通资料离线更新受到版本失败影响")?;
        let context=write_context::open(&state,&project).await?;
        let operation=uuid::Uuid::now_v7().to_string(); shared_operation(&context,&operation,"sql-test","100").await?;
        let held=HeldScreenLeases::acquire(&context,&operation,"sql-test",&["100".into()],true,false).await?;
        let saved=platform_write::update(&context,&held.grants,"100",&screen.fields,&screen.fields,Some((Some("2.0.10"),Some("2.1.3-review-test"))),false).await.map_err(|e|format!("版本SQL写入失败: {e:?}"))?;
        check(saved.wrote&&saved.after.app_version.as_deref()==Some("2.1.3-review-test"),"版本单独更新未回读成功")?;
        let same=platform_write::update(&context,&held.grants,"100",&screen.fields,&screen.fields,Some((Some("2.0.10"),Some("2.1.3-review-test"))),false).await.map_err(|e|format!("同值SQL失败: {e:?}"))?;
        check(!same.wrote,"平台已是确认后值不应重复写")?;
        let conflict=platform_write::update(&context,&held.grants,"100",&screen.fields,&edited,Some((Some("2.0.10"),Some("2.1.2"))),false).await;
        check(matches!(conflict,Err(platform_write::MutationError::Rejected(_))),"版本并发冲突未拒绝")?;
        let row=platform_write::record(&context.read,"100").await?.ok_or("屏缺失")?;
        check(row.asset.fields.name==screen.fields.name&&row.asset.app_version==saved.after.app_version,"版本冲突未回滚同事务的资料修改")?;
        let untouched:(String,i32)=sqlx::query_as(&format!("SELECT version,status FROM `{business}`.smart_terminal_screen WHERE id=100")).fetch_one(&admin).await?;
        check(untouched==("h5-unchanged".into(),0),"误改H5版本或在线状态")?;
        held.release().await?;
        for (id,version) in [("101",Some("2.1.3-review-test")),("102",None)] {
            let op=uuid::Uuid::now_v7().to_string(); shared_operation(&context,&op,"insert-test",id).await?;
            let held=HeldScreenLeases::acquire(&context,&op,"insert-test",&[id.into()],true,false).await?;
            let mut fields=screen.fields.clone(); fields.ip=format!("192.0.2.{id}"); fields.mac=format!("02:11:22:33:44:{}",if id=="101"{"66"}else{"77"});
            let saved=platform_write::insert_with_version(&context,&held.grants,id,&fields,version).await.map_err(|e|format!("新增SQL失败: {e:?}"))?;
            check(saved.after.app_version.as_deref()==Some(version.unwrap_or("schema-default")),"新增版本未写入或未确认时覆盖了平台默认值")?;
            held.release().await?;
        }
        // 以实际预览/任务/意图模拟事务已提交但回包丢失，只执行既有恢复核实。
        for matches_version in [true,false] {
            service.snapshot(&project,true).await?;
            let current=repo.asset(&project,"100").await?;
            repo.append_observation(&project,&observed(&current,"2.1.2")).await?;
            let preview=registration::preview_with_mac(&state,&project,vec!["100".into()],|s|async move{Ok(observed(&s,"2.1.2"))}).await?;
            check(preview.items[0].state=="ready","恢复用例预览失败")?;
            let task=registration::submit(&state,&project,RegistrationSubmission {preview_id:preview.id,screen_ids:vec!["100".into()],mac_confirmations:Default::default(),space_confirmations:vec![]}).await?;
            let (plan,_)=task_data::read_plan(state.local_store.pool(),&project,&task).await?;
            let data:RegistrationPlan=serde_json::from_value(plan.detail.clone())?;
            let item=&data.preview.items[0]; let request=&data.request_ids["100"];
            repo.prepare_intent(&project,&WriteIntent {request_id:request.clone(),business_project_id:context.business.clone(),screen_id:"100".into(),platform_screen_id:"100".into(),operation_type:"update".into(),payload:serde_json::json!({"item":item,"taskId":task}),state:"prepared".into(),result:None}).await?;
            repo.update_intent(&project,request,"prepared","submitted",None).await?;
            shared_operation(&context,&task,&plan.instance_id,"100").await?;
            let held=HeldScreenLeases::acquire(&context,&task,&plan.instance_id,&["100".into()],true,false).await?;
            let change=data.version_change("100").ok_or("缺少恢复版本意图")?;
            platform_write::update(&context,&held.grants,"100",item.before.as_ref().ok_or("缺少原值")?,&item.after,Some((change.before.as_deref(),change.after.as_deref())),false).await.map_err(|e|format!("恢复前SQL失败: {e:?}"))?;
            held.release().await?;
            if !matches_version { sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET app_version='another-writer' WHERE id=100")).execute(&admin).await?; }
            state.task_repository.transition(&task,TaskState::Queued,TaskState::Running,None,None).await?;
            state.task_repository.transition(&task,TaskState::Running,TaskState::FinalizingFailed,None,None).await?;
            task_data::save_results(state.local_store.pool(),&project,&task,&ScreenResults { targets:[("100".into(),ScreenTargetResult {screen_id:"100".into(),business:ResultState::Unknown,shared:ResultState::Pending,..Default::default()})].into(),finished:false }).await?;
            let count_before:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation").fetch_one(state.local_store.pool()).await?;
            tasks::verify(&state,&project,&task).await?;
            let results=task_data::read_results(state.local_store.pool(),&project,&task).await?;
            let result=&results.targets["100"];
            if matches_version {
                check(result.business==ResultState::Succeeded&&result.after_app_version.as_deref()==Some("2.1.2")&&result.before_app_version==change.before,"恢复成功未保留版本摘要")?;
            } else {
                check(result.business==ResultState::Failed,"恢复不能仅凭六资料字段吻合假报版本成功")?;
                let value:Option<String>=sqlx::query_scalar(&format!("SELECT app_version FROM `{business}`.smart_terminal_screen WHERE id=100")).fetch_one(&admin).await?;
                check(value.as_deref()==Some("another-writer"),"恢复核实不应重放覆盖其他版本")?;
            }
            let count_after:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_observation").fetch_one(state.local_store.pool()).await?;
            check(count_before==count_after,"恢复期间重新采集了设备")?;
            sqlx::query(&format!("UPDATE `{business}`.smart_terminal_screen SET app_version='2.1.3-review-test' WHERE id=100")).execute(&admin).await?;
        }
        ops.close().await;
        println!("REGISTRATION_VERSION_SQL_PASSED preview_version_only,suffix,same_value,offline_fields,atomic_conflict,create_version,create_default,recovery_match,recovery_version_conflict,no_device_access");
        Ok(())
    }.await;
    support::close(state).await;
    for schema in created.iter().rev() {
        check(
            schema == &business || schema == &shared,
            "清理目录不属于本次创建的随机库",
        )?;
        sqlx::query(&format!("DROP DATABASE `{schema}`"))
            .execute(&admin)
            .await?;
    }
    admin.close().await;
    println!("REGISTRATION_VERSION_SQL_CLEANED {}", created.len());
    result
}
