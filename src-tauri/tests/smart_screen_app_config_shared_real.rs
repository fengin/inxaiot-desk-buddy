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
            app_config::AppConfigPatch,
            model::{ResultState, ScreenFields},
            operation::ScreenOperationInput,
            registration::RegistrationSubmission,
        },
    },
    formal::workbench_store::WorkbenchStore,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            app_config,
            assets_service::ScreenAssetsService,
            device::{AdbDevice, AndroidTools},
            leases::HeldScreenLeases,
            maintenance, registration,
            shared_results::{ScreenSharedResults, SharedScreenOperation},
            takeover, task_data, tasks, write_context,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{ConnectOptions, Row};
use std::{collections::BTreeMap, time::Duration};

// 只保留文件状态和摘要；不把私有配置内容、地址或凭据写入测试输出。
async fn configuration_fingerprint(ip: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let cancel = tokio_util::sync::CancellationToken::new();
    let pid = device
        .shell(ip, &["pidof", "chat.xiaoxin.app"], cancel.clone())
        .await?;
    if pid.trim().is_empty() {
        return Err("没有读到小新应用进程".into());
    }
    let file = "/data/data/chat.xiaoxin.app/shared_prefs/FlutterSharedPreferences.xml";
    let stamp = device
        .shell(
            ip,
            &["su", "0", "stat", "-c", "%s:%Y", file],
            cancel.clone(),
        )
        .await?;
    let content = device.shell(ip, &["su", "0", "cat", file], cancel).await?;
    if content.trim().is_empty() || !regex::Regex::new(r"^\d+:\d+$")?.is_match(stamp.trim()) {
        return Err("没有读到有效的配置文件状态".into());
    }
    Ok(
        json!({"pid":pid.trim(),"storageStamp":stamp.trim(),"storageSha256":hex::encode(Sha256::digest(content.as_bytes()))}),
    )
}

async fn config_action_log_counts(
    state: &inxaiot_desk_buddy_lib::formal::app_state::FormalAppState,
    task: &str,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let logs = inxaiot_desk_buddy_lib::interface::commands::task_activity::query_task_logs(
        state,
        task,
        &[],
        None,
        0,
        500,
        false,
    )
    .await
    .map_err(|_| "读取配置任务日志失败")?;
    Ok((
        logs.items
            .iter()
            .filter(|row| row.message.contains("正在保存明确选择的字段"))
            .count(),
        logs.items
            .iter()
            .filter(|row| row.message.contains("正在重启小新应用"))
            .count(),
    ))
}

async fn shared_failure_recovers_without_repeating_device_actions(
    state: &inxaiot_desk_buddy_lib::formal::app_state::FormalAppState,
    project: &str,
    asset: &inxaiot_desk_buddy_lib::domain::smart_screen::model::ScreenAsset,
    input: &ScreenOperationInput,
    ops: &sqlx::MySqlPool,
    directory: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    let context = write_context::open(state, project).await?;
    let key = format!("{}:{}", context.business, asset.id);
    if !key
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b":-_".contains(&byte))
    {
        return Err("测试资源编号含意外字符，不建立测试触发器".into());
    }
    let name = format!("共享补存验收-{}", uuid::Uuid::now_v7());
    let patch: AppConfigPatch =
        serde_json::from_value(json!({"set":{"customDeviceName":name},"clear":[]}))?;
    let checked = maintenance::preflight_with_config(
        state,
        project,
        input.clone(),
        Some(BTreeMap::from([(asset.id.clone(), patch)])),
    )
    .await?;
    if checked.items[0].state != "ready" {
        return Err("共享失败验收的设备检查未通过".into());
    }

    // 开始记录及占用照常可写，仅拦住该测试屏配置动作的最终结果 UPDATE。
    let trigger = format!(
        "CREATE TRIGGER fixture_config_result_failure BEFORE UPDATE ON operation_target_result FOR EACH ROW BEGIN IF OLD.resource_type='smart_screen' AND OLD.resource_key='{key}' AND OLD.result_state='pending' AND NEW.result_state<>'pending' AND EXISTS(SELECT 1 FROM operation_record WHERE id=NEW.operation_id AND domain_type='smart_screen' AND operation_type='app_config') THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='fixture configuration result failure'; END IF; END"
    );
    sqlx::raw_sql(&trigger).execute(ops).await?;
    let mut submitted = None;
    let failed: Result<(Value, Value, Value, (usize, usize)), Box<dyn std::error::Error>> = async {
        let task = tasks::submit(state, project, &checked.id, input.clone()).await?;
        submitted = Some(task.clone());
        wait(state, &task).await?;
        let results = task_data::read_results(state.local_store.pool(), project, &task).await?;
        let row = &results.targets[&asset.id];
        if state.task_repository.get(&task).await?.state != TaskState::FinalizingFailed
            || row.device != ResultState::Succeeded || row.business != ResultState::NotRequired
            || row.shared != ResultState::Pending || row.evidence["config"]["save"] != "saved"
            || row.evidence["config"]["restart"] != "not_required" || row.evidence["config"]["readback"] != "succeeded" {
            return Err("共享结果失败没有保留设备配置已保存且回读成功的事实".into());
        }
        let config = app_config::read(state, project, &[asset.id.clone()]).await?[0].config.clone().ok_or("共享失败后未读到实际配置")?;
        if config["customDeviceName"] != name { return Err("共享失败时设备实际名称没有完成修改".into()); }
        let shared: String = sqlx::query_scalar("SELECT result_state FROM operation_target_result WHERE operation_id=? AND resource_key=?")
            .bind(&task).bind(&key).fetch_one(ops).await?;
        if shared != "pending" { return Err("故障没有发生在最终共享结果保存阶段".into()); }
        let (plan, _) = task_data::read_plan(state.local_store.pool(), project, &task).await?;
        let fingerprint = configuration_fingerprint(&asset.fields.ip).await?;
        let action_logs = config_action_log_counts(state, &task).await?;
        if action_logs != (1, 0) { return Err("配置改名原任务的保存/重启次数不符合预期".into()); }
        Ok((json!({"config":config,"evidence":row.evidence,"observation":row.observation}), plan.detail, fingerprint, action_logs))
    }.await;
    let dropped = sqlx::raw_sql("DROP TRIGGER fixture_config_result_failure")
        .execute(ops)
        .await;
    // 无论断言是否通过，都先恢复共享写入并核实本次任务，解除占用后才能还原配置。
    dropped?;
    let Some(task) = submitted else {
        return failed.map(|_| ());
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        while state.job_supervisor.contains(&task).await {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    tasks::verify(state, project, &task).await?;
    let (before, plan_detail, fingerprint, action_logs) = failed?;
    if state.task_repository.get(&task).await?.state != TaskState::Succeeded {
        return Err("恢复共享写入后未成功补存".into());
    }
    let after = task_data::read_results(state.local_store.pool(), project, &task).await?;
    let result = &after.targets[&asset.id];
    let (plan, _) = task_data::read_plan(state.local_store.pool(), project, &task).await?;
    let config = app_config::read(state, project, &[asset.id.clone()]).await?[0]
        .config
        .clone()
        .ok_or("补存后未读到配置")?;
    if result.shared != ResultState::Succeeded
        || result.device != ResultState::Succeeded
        || serde_json::to_value(&result.observation)? != before["observation"]
        || result.evidence != before["evidence"]
        || plan.detail != plan_detail
        || config != before["config"]
        || configuration_fingerprint(&asset.fields.ip).await? != fingerprint
        || config_action_log_counts(state, &task).await? != action_logs
    {
        return Err("补存期间配置、应用进程、原请求或设备动作记录发生变化".into());
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operation_target_result WHERE operation_id=? AND result_state='succeeded'")
        .bind(&task).fetch_one(ops).await?;
    let audits: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE action='app_config' AND JSON_UNQUOTE(JSON_EXTRACT(changed_fields_json,'$.operationId'))=?")
        .bind(&task).fetch_one(ops).await?;
    if count != 1 || audits != 1 || state.task_repository.results_protected(&task).await? {
        return Err("补存没有收敛到一条结果和一条审计，或本机结果仍被保护".into());
    }
    std::fs::write(
        directory.join("shared-result-recovery.json"),
        serde_json::to_vec_pretty(&json!({
            "passed":true,"task":task,"deviceSavedBeforeFailure":true,"sharedRecordCount":count,"auditCount":audits,
            "sameRequest":true,"sameConfiguration":true,"samePidAndStorage":true,"saveActionCount":action_logs.0,"restartActionCount":action_logs.1
        }))?,
    )?;
    eprintln!(
        "配置已保存而共享结果失败：恢复后只补一条结果与审计，原请求、设备配置、进程和文件状态保持"
    );
    Ok(())
}

async fn wait(
    state: &inxaiot_desk_buddy_lib::formal::app_state::FormalAppState,
    task: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(150), async {
        loop {
            let row = state.task_repository.get(task).await.unwrap();
            if row.state.is_terminal()
                || row.state
                    == inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    Ok(())
}
#[tokio::test]
#[ignore = "142隔离库验证配置任务共享记录与接手；只修改测试说明首台4寸屏自定义名称并还原"]
async fn registered_configuration_takeover_shared_summary_and_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let cfg = project_test_config::database();
    if cfg.host != "192.168.3.142" {
        return Err("本测试只允许指定的142测试环境".into());
    }
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
    let business = format!("inxaiot_desk_buddy_cfgb_{suffix}");
    let shared = format!("inxaiot_desk_buddy_cfgw_{suffix}");
    sqlx::query(&format!(
        "CREATE DATABASE `{business}` CHARACTER SET utf8mb4"
    ))
    .execute(&admin)
    .await?;
    sqlx::query(&format!("CREATE DATABASE `{shared}` CHARACTER SET utf8mb4"))
        .execute(&admin)
        .await?;
    let dir = support::evidence_dir("configuration-shared")?;
    let state = support::state_at(&dir, true).await;
    let outcome:Result<(),Box<dyn std::error::Error>>=async {
        for table in ["smart_terminal_screen","t_project_building","op_device_service_area"] {sqlx::query(&format!("CREATE TABLE `{business}`.`{table}` LIKE inxvision_iot_dev_demo.`{table}`")).execute(&admin).await?;}
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'配置验收空间',3)")).execute(&admin).await?;
        let ops=sqlx::mysql::MySqlPoolOptions::new().max_connections(3).connect_with(options.clone().database(&shared)).await?;WorkbenchStore::new(ops.clone()).migrate().await?;
        let description=std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("test/测试数据说明.txt"))?;
        let api=description.lines().find_map(|line|line.trim().strip_prefix("平台API：")).ok_or("缺少测试API")?.trim();
        let start=description.find('{').ok_or("缺少测试登录")?;let end=start+description[start..].find('}').ok_or("登录格式错误")?+1;let login:Value=serde_json::from_str(&description[start..end])?;
        let adapter=Stage75Adapter::new(&state);
        let project=adapter.create_project(ProjectInput {name:"配置共享验收".into(),platform_url:format!("http://{api}"),db_host:cfg.host.clone(),db_port:cfg.port,db_user:cfg.username.clone(),db_tls_enabled:false,db_password:Some(cfg.password.clone()),business_db:business.clone(),workbench_db:shared.clone()}).await?.project.id;
        adapter.login_project(&project,PlatformLoginRequest {username:login["principal"].as_str().unwrap().into(),password:login["credentials"].as_str().unwrap().into(),session_uuid:login["sessionUUID"].as_str().unwrap().into(),image_code:login["imageCode"].as_str().unwrap().into()}).await?;
        let service=ScreenAssetsService::new(&state);let snapshot=service.snapshot(&project,true).await?;if !snapshot.platform_available{return Err("隔离测试项目不可用".into());}
        let repo=ScreenRepository::new(state.local_store.pool().clone());
        let local=repo.save_local(&project,&ScreenFields {name:"配置验收屏".into(),ip:support::live_screen("4")?.ip,size:"4".into(),space_id:Some("1000".into()),..Default::default()},None,None).await?;
        let preview=registration::preview(&state,&project,vec![local.clone()]).await?;
        if preview.items[0].state!="ready"{return Err(preview.items[0].reason.clone().into());}
        let registered=registration::submit(&state,&project,RegistrationSubmission {preview_id:preview.id,screen_ids:vec![local],mac_confirmations:Default::default(),space_confirmations:vec![]}).await?;wait(&state,&registered).await?;
        let snapshot=service.snapshot(&project,true).await?;let asset=snapshot.screens.first().ok_or("登记未完成")?.clone();if asset.source!="platform"{return Err("登记未完成".into());}
        let screen=asset.id.clone();let before=app_config::read(&state,&project,&[screen.clone()]).await?[0].config.clone().ok_or("配置未读取")?;
        std::fs::write(dir.join("original-config.json"),serde_json::to_vec(&before)?)?;
        let context=write_context::open(&state,&project).await?;
        let source=uuid::Uuid::now_v7().to_string();
        ScreenSharedResults::new(context.shared.clone()).start(&SharedScreenOperation {id:&source,business_project_id:&context.business,action:"app_config",name:"另一电脑配置修改".into(),operator:"验收A",instance_id:"config-client-a",targets:&[format!("{}:{screen}",context.business)],started_at:None}).await?;
        let held=HeldScreenLeases::acquire(&context,&source,"config-client-a",&[screen.clone()],false,false).await?;
        let input=ScreenOperationInput {action:"app_config".into(),target_ids:vec![screen.clone()],application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),reinstall:false,concurrency:1,expected_targets:Default::default(),retry_of_operation_id:None};
        let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"customDeviceName":"共享配置验收"},"clear":[]}))?;
        let checked=maintenance::preflight_with_config(&state,&project,input.clone(),Some(BTreeMap::from([(screen.clone(),patch.clone())]))).await?;
        if checked.items[0].state!="ready"{return Err("编辑检查阶段不应因另一台占用阻止读取".into());}
        let blocked=tasks::submit(&state,&project,&checked.id,input.clone()).await;
        let conflicts=match blocked {Err(inxaiot_desk_buddy_lib::core::error::AppError::ConfirmationRequired {code,details}) if code=="SCREEN_TAKEOVER_REQUIRED"=>serde_json::from_value::<Vec<takeover::TakeoverConflict>>(details["conflicts"].clone())?,_=>return Err("配置提交没有返回接手确认".into())};
        takeover::release(&state,&project,&conflicts,true).await?;
        if held.valid().await.is_ok(){return Err("接手后原电脑仍可写入".into());}drop(held);
        if HeldScreenLeases::acquire(&context,&source,"config-client-a",&[screen.clone()],false,true).await.is_ok(){return Err("已被释放的旧配置任务仍能重新取得占用继续写入".into());}
        service.snapshot(&project,true).await?;
        let checked=maintenance::preflight_with_config(&state,&project,input.clone(),Some(BTreeMap::from([(screen.clone(),patch)]))).await?;
        let task=tasks::submit(&state,&project,&checked.id,input.clone()).await?;
        wait(&state,&task).await?;
        let result=task_data::read_results(state.local_store.pool(),&project,&task).await?;
        let verification:Result<(),Box<dyn std::error::Error>>=async {
            let target=&result.targets[&screen];
            if target.device!=ResultState::Succeeded||target.shared!=ResultState::Succeeded||target.business!=ResultState::NotRequired{return Err(format!("共享配置任务失败：{}",target.message).into());}
            let row=sqlx::query("SELECT CAST(result_detail_json AS CHAR) AS detail FROM operation_target_result WHERE operation_id=?").bind(&task).fetch_one(&ops).await?;
            let text:String=row.try_get("detail")?;let detail:Value=serde_json::from_str(&text)?;
            if detail["configuration"]["save"]!="saved"||text.contains("http")||text.contains("authCode"){return Err("共享摘要不符合字段限制".into());}
            let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM audit_event WHERE action='app_config' AND changed_fields_json IS NOT NULL").fetch_one(&ops).await?;if count!=1{return Err("配置审计没有保存".into());}
            let after=service.snapshot(&project,true).await?;
            if serde_json::to_value(&asset)?!=serde_json::to_value(&after.screens[0])?{return Err("配置任务改写了平台屏资产".into());}
            Ok(())
        }.await;
        let recovery = if verification.is_ok() { shared_failure_recovers_without_repeating_device_actions(&state,&project,&asset,&input,&ops,&dir).await } else { Ok(()) };
        let restore:AppConfigPatch=serde_json::from_value(if before["customDeviceName"].is_null(){json!({"set":{},"clear":["customDeviceName"]})}else{json!({"set":{"customDeviceName":before["customDeviceName"]},"clear":[]})})?;
        let checked=maintenance::preflight_with_config(&state,&project,input.clone(),Some(BTreeMap::from([(screen.clone(),restore)]))).await?;
        let restored=tasks::submit(&state,&project,&checked.id,input).await?;wait(&state,&restored).await?;
        if app_config::read(&state,&project,&[screen]).await?[0].config.as_ref()!=Some(&before){return Err("共享配置验证后还原未通过".into());}
        let local_only=repo.save_local(&project,&ScreenFields {name:"混合批次未注册屏".into(),ip:support::live_screen("10")?.ip,size:"10".into(),..Default::default()},None,None).await?;
        adapter.logout_project(&project).await?;
        let registered_id=asset.id.clone();
        if app_config::read(&state,&project,&[registered_id.clone()]).await?[0].config.is_none(){return Err("平台退出登录不应阻止ADB只读配置".into());}
        let mixed=ScreenOperationInput {action:"app_config".into(),target_ids:vec![registered_id.clone(),local_only.clone()],application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),reinstall:false,concurrency:1,expected_targets:Default::default(),retry_of_operation_id:None};
        let unchanged:AppConfigPatch=serde_json::from_value(json!({"set":{"customDeviceName":"不应执行"},"clear":[]}))?;
        match maintenance::preflight_with_config(&state,&project,mixed,Some(BTreeMap::from([(registered_id,unchanged.clone()),(local_only,unchanged)]))).await {
            Err(error) if error.to_string().contains("整批未执行")=>{},
            _=>return Err("平台不可用时混合配置批次没有整体阻止".into()),
        }
        eprintln!("配置接手、旧占用拒绝、共享记录与审计、平台资产不变和配置还原已验证");
        ops.close().await;verification.and(recovery)
    }.await;
    support::close(state).await;
    // 名称由固定前缀和本次 UUID 生成，只删除本测试新建的两库。
    sqlx::query(&format!("DROP DATABASE `{business}`"))
        .execute(&admin)
        .await?;
    sqlx::query(&format!("DROP DATABASE `{shared}`"))
        .execute(&admin)
        .await?;
    admin.close().await;
    outcome
}
