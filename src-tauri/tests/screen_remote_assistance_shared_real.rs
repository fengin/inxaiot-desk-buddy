//! 142随机隔离库上的辅助操作共享互斥回归；设备端口全部模拟，绝不执行ADB或真实输入。
#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::{
        project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
    },
    core::error::AppResult,
    domain::{
        common::project::{PlatformLoginRequest, ProjectInput},
        smart_screen::model::{ResultState, ScreenTargetResult},
    },
    formal::workbench_store::WorkbenchStore,
    infrastructure::{
        smart_screen::{
            assets_service::ScreenAssetsService,
            leases::HeldScreenLeases,
            remote_screen::{
                self, ControlAction, ControlFuture, ControlInput, ControlPort, ControlSendContext,
                DeviceFacts, RawCapture, RemoteScreenSessions, ScreenCapturePort,
            },
            shared_results::{ScreenSharedResults, SharedScreenOperation},
            write_context,
        },
        stage75_adapter::Stage75Adapter,
        task_runtime::retry_pending_local_finalization,
    },
};
use serde_json::{Value, json};
use sqlx::{ConnectOptions, MySqlPool, Row};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const IP: &str = "192.0.2.42";
const SERIAL: &str = "shared-fixture-serial";
const MAC: &str = "02:11:22:33:44:55";
const SECRET_MARKER: &str = "SharedRemoteSecret_%s'\"$&42";
const RELEASE_TRIGGER: &str = "fixture_remote_release_failure";
const PREPARE_TRIGGER: &str = "fixture_remote_prepare_lease";
const LINK_TRIGGER: &str = "fixture_remote_link_failure";

fn require(ok: bool, message: &'static str) -> TestResult<()> {
    if ok { Ok(()) } else { Err(message.into()) }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
fn persist(directory: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}
fn valid_schema(schema: &str) -> bool {
    regex::Regex::new(r"^inxaiot_desk_buddy_remote_[bw]_[a-f0-9]{32}$")
        .unwrap()
        .is_match(schema)
}
async fn create_schema(admin: &MySqlPool, schema: &str, marker: &str) -> TestResult<()> {
    require(valid_schema(schema), "隔离库名不符合本轮范围")?;
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
        .execute(admin)
        .await?;
    sqlx::query(&format!("CREATE TABLE `{schema}`.__remote_assistance_scope(id INT PRIMARY KEY, marker VARCHAR(32) NOT NULL)")).execute(admin).await?;
    sqlx::query(&format!(
        "INSERT INTO `{schema}`.__remote_assistance_scope VALUES(1,?)"
    ))
    .bind(marker)
    .execute(admin)
    .await?;
    Ok(())
}
async fn drop_schema(admin: &MySqlPool, schema: &str, marker: &str) -> TestResult<()> {
    require(valid_schema(schema), "不允许清理范围外数据库")?;
    let actual: String = sqlx::query_scalar(&format!(
        "SELECT marker FROM `{schema}`.__remote_assistance_scope WHERE id=1"
    ))
    .fetch_one(admin)
    .await?;
    require(actual == marker, "隔离库marker不匹配，未清理")?;
    sqlx::query(&format!("DROP DATABASE `{schema}`"))
        .execute(admin)
        .await?;
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME=?")
            .bind(schema)
            .fetch_one(admin)
            .await?;
    require(remaining == 0, "隔离库删除未确认")
}

fn png() -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        out.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
    let mut compressed =
        flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    compressed.write_all(&[0, 0, 0, 0, 255]).unwrap();
    chunk(&mut out, b"IDAT", &compressed.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}
struct MemoryDevice {
    sends: AtomicUsize,
}
impl ControlPort for MemoryDevice {
    fn probe<'a>(&'a self, ip: &'a str, _: CancellationToken) -> ControlFuture<'a, DeviceFacts> {
        Box::pin(async move {
            assert_eq!(ip, IP);
            Ok(DeviceFacts {
                android_sdk: 29,
                awake: Some(true),
                serial: Some(SERIAL.into()),
                mac: Some(MAC.into()),
                foreground_package: "chat.xiaoxin.app".into(),
                width: 1,
                height: 1,
                orientation: 0,
            })
        })
    }
    fn send<'a>(
        &'a self,
        ip: &'a str,
        action: &'a ControlAction,
        context: ControlSendContext,
        _: CancellationToken,
    ) -> ControlFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(ip, IP);
            assert_eq!(context.android_sdk, 29);
            assert!(context.deadline > tokio::time::Instant::now());
            assert!(matches!(action,ControlAction::Text{text} if text==SECRET_MARKER));
            self.sends.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
}
impl ScreenCapturePort for MemoryDevice {
    async fn capture(&self, ip: &str, _: CancellationToken) -> AppResult<Vec<u8>> {
        assert_eq!(ip, IP);
        Ok(png())
    }
    async fn capture_verified(&self, ip: &str, cancel: CancellationToken) -> AppResult<RawCapture> {
        Ok(RawCapture {
            png: self.capture(ip, cancel).await?,
            serial: Some(SERIAL.into()),
        })
    }
}

#[tokio::test]
#[ignore = "仅142随机双库与内存设备端口；验证注册屏共享互斥和释放失败恢复，不执行ADB"]
async fn registered_assistance_lease_conflict_and_release_recovery_are_isolated() -> TestResult<()>
{
    require(
        std::env::var("INX_REMOTE_SCREEN_SHARED_REAL").as_deref() == Ok("YES"),
        "缺少共享租约隔离测试门禁",
    )?;
    let cfg = project_test_config::database();
    require(
        cfg.host == "192.168.3.142" && cfg.port == 3306,
        "只允许142测试数据库",
    )?;
    let token = uuid::Uuid::now_v7().simple().to_string();
    let business = format!("inxaiot_desk_buddy_remote_b_{token}");
    let shared = format!("inxaiot_desk_buddy_remote_w_{token}");
    let directory = root()
        .join(".review-tools/remote-screen-20261026")
        .join(format!("shared-contract-{token}"));
    std::fs::create_dir_all(&directory)?;
    let mut report = json!({"passed":false,"marker":token,"businessSchema":business,"sharedSchema":shared,"createdSchemas":[],"schemasRemoved":false,
        "devicePort":"memory-fixture","deviceIp":IP,"adbInvoked":false,"screenshotsWritten":false,"productionDataUsed":false});
    persist(&directory, &report)?;
    println!(
        "辅助共享互斥隔离证据：{}",
        directory.join("result.json").display()
    );
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
    create_schema(&admin, &business, &token).await?;
    report["createdSchemas"]
        .as_array_mut()
        .unwrap()
        .push(json!(business));
    persist(&directory, &report)?;
    create_schema(&admin, &shared, &token).await?;
    report["createdSchemas"]
        .as_array_mut()
        .unwrap()
        .push(json!(shared));
    persist(&directory, &report)?;
    let state = support::state_at(&directory.join("app-data"), false).await;
    let sessions = RemoteScreenSessions::default();
    let result=async {
        for table in ["smart_terminal_screen","t_project_building","op_device_service_area"] {
            sqlx::query(&format!("CREATE TABLE `{business}`.`{table}` LIKE inxvision_iot_dev_demo.`{table}`")).execute(&admin).await?;
        }
        sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'辅助互斥隔离空间',3)")).execute(&admin).await?;
        sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,app_version,version,status,delete_flag) VALUES(7001,'辅助互斥隔离屏',?,?,'10-inch',1000,'isolated','2.1.3','keep-h5',0,0)"))
            .bind(IP).bind(MAC).execute(&admin).await?;
        let ops=sqlx::mysql::MySqlPoolOptions::new().max_connections(4).connect_with(options.clone().database(&shared)).await?;
        WorkbenchStore::new(ops.clone()).migrate().await?;
        let description=std::fs::read_to_string(root().join("test/测试数据说明.txt"))?;
        let endpoint=description.lines().find_map(|line|line.trim().strip_prefix("平台API：")).ok_or("未提供测试API")?.trim();
        let endpoint=reqwest::Url::parse(&format!("http://{endpoint}"))?;
        require(endpoint.host_str()==Some("192.168.3.142")&&endpoint.username().is_empty()&&endpoint.password().is_none(),"只允许142测试API")?;
        let start=description.find('{').ok_or("未提供测试登录配置")?;let end=start+description[start..].find('}').ok_or("测试登录配置不完整")?+1;
        let login:Value=serde_json::from_str(&description[start..end])?;
        let value=|key:&str|->TestResult<String>{login[key].as_str().map(str::to_owned).ok_or("测试登录字段缺失".into())};
        let adapter=Stage75Adapter::new(&state);
        let project=adapter.create_project(ProjectInput{name:"辅助操作共享互斥隔离验收".into(),platform_url:endpoint.to_string(),db_host:cfg.host.clone(),db_port:cfg.port,
            db_user:cfg.username.clone(),db_tls_enabled:false,db_password:Some(cfg.password.clone()),business_db:business.clone(),workbench_db:shared.clone()}).await?.project.id;
        adapter.login_project(&project,PlatformLoginRequest{username:value("principal")?,password:value("credentials")?,session_uuid:value("sessionUUID")?,image_code:value("imageCode")?}).await?;
        let snapshot=ScreenAssetsService::new(&state).snapshot(&project,true).await?;
        require(snapshot.platform_available&&snapshot.screens.len()==1,"隔离平台屏未正确读取")?;
        let asset=snapshot.screens.into_iter().next().unwrap();require(asset.id=="7001"&&asset.fields.ip==IP&&asset.source=="platform","隔离资产范围不符")?;
        let baseline:Value=sqlx::query_scalar::<_,String>(&format!("SELECT CAST(JSON_OBJECT('name',name,'ip',ip,'mac',mac,'app_version',app_version,'version',version,'status',status) AS CHAR) FROM `{business}`.smart_terminal_screen WHERE id=7001"))
            .fetch_one(&admin).await.and_then(|value|serde_json::from_str(&value).map_err(|e|sqlx::Error::Decode(Box::new(e))))?;
        let io=Arc::new(MemoryDevice{sends:AtomicUsize::new(0)});
        // 等效故障注入：共享端已存在本operation租约，但本机prepare尚未交付Held句柄。
        // 仅在本轮随机库触发；不模拟真实网络断连，不触发任何设备端口send。
        sqlx::raw_sql(&format!("CREATE TRIGGER `{shared}`.`{PREPARE_TRIGGER}` AFTER INSERT ON `{shared}`.operation_record FOR EACH ROW BEGIN IF NEW.domain_type='smart_screen' AND NEW.operation_type='remote_assistance' THEN INSERT INTO `{shared}`.resource_lease(resource_type,resource_key,domain_type,operation_id,owner_instance_id,owner_user,lease_token,fencing_token,lease_state,acquired_at,heartbeat_at,expires_at) VALUES('smart_screen',CONCAT(NEW.business_project_id,':7001'),'smart_screen',NEW.id,NEW.instance_id,NEW.operator_name,UUID(),1,'active',UTC_TIMESTAMP(6),UTC_TIMESTAMP(6),DATE_ADD(UTC_TIMESTAMP(6),INTERVAL 90 SECOND)); END IF; END"))
            .execute(&admin).await?;
        sqlx::raw_sql(&format!("CREATE TRIGGER {LINK_TRIGGER} BEFORE UPDATE OF remote_operation_record_id ON local_task WHEN NEW.operation_type='remote_assistance' BEGIN SELECT RAISE(FAIL,'isolated link fixture'); END;"))
            .execute(state.local_store.pool()).await?;
        let preparing=remote_screen::open(&state,&sessions,"shared-fixture",&project,&asset.id).await?;
        let prepare_port:Arc<dyn ControlPort>=io.clone();
        require(remote_screen::begin_control_with(&state,&sessions,"shared-fixture",&project,&preparing.session_id,prepare_port).await.is_err(),"prepare故障没有被拒绝")?;
        let prepared_tasks:Vec<String>=sqlx::query_scalar("SELECT id FROM local_task WHERE local_project_id=? AND operation_type='remote_assistance'")
            .bind(&project).fetch_all(state.local_store.pool()).await?;
        require(prepared_tasks.len()==1,"prepare故障产生了非预期任务数量")?;
        let prepared_task=&prepared_tasks[0];
        report["prepareTaskId"]=json!(prepared_task);persist(&directory,&report)?;
        tokio::time::timeout(Duration::from_secs(12),async {while state.job_supervisor.contains(prepared_task).await{tokio::time::sleep(Duration::from_millis(10)).await;}}).await?;
        let prepared_lease=sqlx::query("SELECT lease_state,owner_instance_id,resource_key FROM resource_lease WHERE operation_id=?")
            .bind(prepared_task).fetch_one(&ops).await?;
        require(prepared_lease.try_get::<String,_>("lease_state")?=="released"&&prepared_lease.try_get::<String,_>("resource_key")?=="777001:7001","未交付Held句柄的本操作租约没有被精确释放")?;
        require(io.sends.load(Ordering::SeqCst)==0&&state.task_repository.results_protected(prepared_task).await?,"prepare故障发出了输入或丢失恢复保护")?;
        let prepared_summary:String=sqlx::query_scalar("SELECT CAST(result_detail_json AS CHAR) FROM operation_target_result WHERE operation_id=?")
            .bind(prepared_task).fetch_one(&ops).await?;
        sqlx::raw_sql(&format!("DROP TRIGGER `{shared}`.`{PREPARE_TRIGGER}`")).execute(&admin).await?;
        sqlx::query(&format!("DROP TRIGGER {LINK_TRIGGER}")).execute(state.local_store.pool()).await?;
        retry_pending_local_finalization(&state,prepared_task,false).await?;
        let prepared_recovered:String=sqlx::query_scalar("SELECT CAST(result_detail_json AS CHAR) FROM operation_target_result WHERE operation_id=?")
            .bind(prepared_task).fetch_one(&ops).await?;
        require(prepared_summary==prepared_recovered&&!state.task_repository.results_protected(prepared_task).await?&&io.sends.load(Ordering::SeqCst)==0,"prepare恢复改写摘要、未解除保护或重放了输入")?;
        report["preparePartialLeaseFixture"]=json!({"kind":"已存在本operation租约但未交付句柄的等效故障注入","realNetworkInterrupted":false,"ownLeaseReleased":true,"recoveredWithoutInput":true,"sendCount":0});
        persist(&directory,&report)?;
        remote_screen::close(&sessions,"shared-fixture",&project,&preparing.session_id)?;
        let session=remote_screen::open(&state,&sessions,"shared-fixture",&project,&asset.id).await?;
        let port:Arc<dyn ControlPort>=io.clone();
        let control=remote_screen::begin_control_with(&state,&sessions,"shared-fixture",&project,&session.session_id,port).await?;
        report["taskId"]=json!(control.task_id);report["projectId"]=json!(project);persist(&directory,&report)?;
        let context=write_context::open(&state,&project).await?;
        let expected_key=format!("{}:{}",context.business,asset.id);
        let lease=sqlx::query("SELECT resource_type,resource_key,operation_id,owner_instance_id,lease_token,fencing_token,lease_state FROM resource_lease WHERE operation_id=?")
            .bind(&control.task_id).fetch_one(&ops).await?;
        let first_fence:u64=lease.try_get("fencing_token")?;
        let plan:Value=serde_json::from_str(&sqlx::query_scalar::<_,String>("SELECT plan_json FROM local_screen_task_data WHERE local_task_id=?")
            .bind(&control.task_id).fetch_one(state.local_store.pool()).await?)?;
        let operator:String=sqlx::query_scalar("SELECT operator_name FROM operation_record WHERE id=?").bind(&control.task_id).fetch_one(&ops).await?;
        require(lease.try_get::<String,_>("resource_type")?=="smart_screen"&&lease.try_get::<String,_>("resource_key")?==expected_key
            &&lease.try_get::<String,_>("operation_id")?==control.task_id&&lease.try_get::<String,_>("owner_instance_id")?==plan["instanceId"].as_str().unwrap_or("")
            &&lease.try_get::<String,_>("lease_state")?=="active"&&first_fence>0&&uuid::Uuid::parse_str(&lease.try_get::<String,_>("lease_token")?).is_ok(),"辅助操作共享租约字段不正确")?;
        require(plan["operator"].as_str()==Some(operator.as_str()),"本机计划没有保留真实操作人")?;
        report["sharedLeaseFieldsVerified"]=json!(true);
        let competing=uuid::Uuid::now_v7().to_string();let keys=vec![expected_key.clone()];
        let repository=ScreenSharedResults::new(ops.clone());
        repository.start(&SharedScreenOperation{id:&competing,business_project_id:&context.business,action:"install",name:"隔离竞争请求",operator:"fixture",instance_id:"other-fixture-instance",targets:&keys,started_at:None}).await?;
        match HeldScreenLeases::acquire(&context,&competing,"other-fixture-instance",std::slice::from_ref(&asset.id),false,false).await {
            Ok(held)=>{held.release().await?;return Err("已有安装路径未被辅助操作共享租约阻止".into());},
            Err(_)=>report["existingLeasePathBlocked"]=json!(true),
        }
        let frame=remote_screen::capture_with(&state,&sessions,"shared-fixture",&project,&session.session_id,&*io).await?;
        let receipt=remote_screen::act(&state,&sessions,"shared-fixture",&project,&session.session_id,
            ControlInput{frame_id:frame.frame_id,action:ControlAction::Text{text:SECRET_MARKER.into()}}).await?;
        require(receipt.status=="sent"&&io.sends.load(Ordering::SeqCst)==1,"内存输入端口没有收到一次输入")?;
        require(uuid::Uuid::parse_str(&control.task_id).is_ok(),"任务标识不安全")?;
        sqlx::raw_sql(&format!("CREATE TRIGGER `{shared}`.`{RELEASE_TRIGGER}` BEFORE UPDATE ON `{shared}`.resource_lease FOR EACH ROW BEGIN IF NEW.operation_id='{}' AND NEW.lease_state='released' THEN SIGNAL SQLSTATE '45000' SET MESSAGE_TEXT='isolated release fixture'; END IF; END",control.task_id)).execute(&admin).await?;
        require(remote_screen::end_control(&sessions,"shared-fixture",&project,&session.session_id).await.is_err(),"释放失败被错误报告为全部完成")?;
        tokio::time::timeout(Duration::from_secs(8),async {while state.job_supervisor.contains(&control.task_id).await{tokio::time::sleep(Duration::from_millis(10)).await;}}).await?;
        require(state.task_repository.results_protected(&control.task_id).await?,"共享释放未确认却丢失本机保护")?;
        let saved_summary:String=sqlx::query_scalar("SELECT CAST(result_detail_json AS CHAR) FROM operation_target_result WHERE operation_id=?")
            .bind(&control.task_id).fetch_one(&ops).await?;
        let active:i64=sqlx::query_scalar("SELECT count(*) FROM resource_lease WHERE operation_id=? AND lease_state='active'").bind(&control.task_id).fetch_one(&ops).await?;
        require(active==1&&!saved_summary.contains(SECRET_MARKER),"故障窗口租约或摘要保留错误")?;
        report["releaseFailureProtected"]=json!(true);persist(&directory,&report)?;
        sqlx::raw_sql(&format!("DROP TRIGGER `{shared}`.`{RELEASE_TRIGGER}`")).execute(&admin).await?;
        retry_pending_local_finalization(&state,&control.task_id,false).await?;
        let recovered:String=sqlx::query_scalar("SELECT CAST(result_detail_json AS CHAR) FROM operation_target_result WHERE operation_id=?")
            .bind(&control.task_id).fetch_one(&ops).await?;
        require(saved_summary==recovered,"补存重写了已经确认的共享摘要")?;
        require(!state.task_repository.results_protected(&control.task_id).await?&&io.sends.load(Ordering::SeqCst)==1,"恢复没有解除保护或重复发送了输入")?;
        let fresh=HeldScreenLeases::acquire(&context,&competing,"other-fixture-instance",std::slice::from_ref(&asset.id),false,false).await?;
        require(fresh.grants.len()==1&&fresh.grants[0].fencing_token>first_fence,"释放后未恢复正常互斥计数")?;
        let old_task=state.task_repository.get(&control.task_id).await?;
        remote_screen::assistance::recover(&state,&old_task,false).await?;
        fresh.valid().await?;
        require(io.sends.load(Ordering::SeqCst)==1,"重复核实重放了输入")?;
        report["recoveryPreservesOtherOperationLease"]=json!(true);
        fresh.release().await?;
        let competing_targets:Vec<(String,String)>=sqlx::query_as("SELECT resource_key,result_state FROM operation_target_result WHERE operation_id=? AND resource_type='smart_screen'")
            .bind(&competing).fetch_all(&ops).await?;
        require(competing_targets==vec![(expected_key.clone(),"pending".into())],"竞争请求的共享目标标识或初始状态不匹配")?;
        repository.save_target(&competing,&context.business,&ScreenTargetResult{format_version:1,screen_id:expected_key.clone(),device:ResultState::Succeeded,
            business:ResultState::NotRequired,shared:ResultState::Pending,message:"隔离竞争请求已确认可重新取得并释放租约".into(),..Default::default()},"succeeded").await?;
        let competing_saved:Vec<(String,String)>=sqlx::query_as("SELECT resource_key,result_state FROM operation_target_result WHERE operation_id=? AND resource_type='smart_screen'")
            .bind(&competing).fetch_all(&ops).await?;
        require(competing_saved==vec![(expected_key.clone(),"succeeded".into())],"竞争请求共享摘要没有保存到原目标")?;
        repository.finish(&competing,&context.business).await?;
        let after:String=sqlx::query_scalar(&format!("SELECT CAST(JSON_OBJECT('name',name,'ip',ip,'mac',mac,'app_version',app_version,'version',version,'status',status) AS CHAR) FROM `{business}`.smart_terminal_screen WHERE id=7001")).fetch_one(&admin).await?;
        require(serde_json::from_str::<Value>(&after)?==baseline,"辅助操作修改了隔离平台资产")?;
        for entry in walkdir::WalkDir::new(&state.paths.data_dir).follow_links(false) {
            let entry=entry?;if entry.file_type().is_file(){let bytes=std::fs::read(entry.path())?;require(!bytes.windows(SECRET_MARKER.len()).any(|part|part==SECRET_MARKER.as_bytes()),"输入正文进入本机持久文件")?;}
        }
        report["recoveryDidNotReplayInput"]=json!(true);report["originalSharedSummaryPreserved"]=json!(true);report["platformFixtureUnchanged"]=json!(true);
        report["inputSendCount"]=json!(io.sends.load(Ordering::SeqCst));
        remote_screen::close(&sessions,"shared-fixture",&project,&session.session_id)?;ops.close().await;
        Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    sessions.close_all();
    support::close(state).await;
    if result.is_ok() {
        drop_schema(&admin, &business, &token).await?;
        drop_schema(&admin, &shared, &token).await?;
        report["schemasRemoved"] = json!(true);
        report["passed"] = json!(true);
    }
    persist(&directory, &report)?;
    admin.close().await;
    result
}
