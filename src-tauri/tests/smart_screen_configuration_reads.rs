#[path="common/screen_test_support.rs"] mod support;
#[path="common/screen_configuration_read_support.rs"] mod measured;
use inxaiot_desk_buddy_lib::{application::ports::project_management::ProjectManagementPort,core::error::{AppError,AppResult},
    domain::smart_screen::{app_config::AppConfigPatch,model::ScreenFields,operation::ScreenOperationInput},
    infrastructure::{local_sqlite::screen_repository::{ScreenRepository,now},smart_screen::{app_config,maintenance,task_data,
        device::{AdbDevice,AndroidTools,DeviceCommandPort,ProcessOutput}},stage75_adapter::Stage75Adapter}};
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{json,Value};
use std::{collections::BTreeMap,path::Path,sync::{Arc,Mutex,atomic::{AtomicUsize,Ordering}},time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Clone,Default)]
struct MockCommands {failed_ip:Option<String>,snapshot_mode:&'static str,calls:Arc<Mutex<BTreeMap<String,Vec<String>>>>}
fn config(ip:&str)->Value {
    let env=json!({"otaUrl":"https://example.test","wsUrl":null,"h5Url":null,"h5ReadyCheckEnabled":true,"otaWsUrl":null,"otaH5Url":null,
        "effectiveWsUrl":null,"effectiveH5Url":null,"wsSource":"none","h5Source":"none"});
    json!({"customDeviceName":ip,"environments":{"current":"prod","test":env,"pre":env,"prod":env}})
}
fn packet(value:Value)->String {format!("Result: Bundle[{{result={}}}]",STANDARD.encode(serde_json::to_vec(&value).unwrap()))}
fn capabilities()->Value {json!({"status":"ok","data":{"protocolVersion":1,"packageName":"chat.xiaoxin.app","ready":true,"supportedFields":["customDeviceName"]}})}
fn home_response()->Value {json!({"status":"ok","data":{"protocolVersion":1,"packageName":"chat.xiaoxin.app","deviceOwner":false,"resolvedHome":null}})}
fn kiosk_response()->Value {json!({"status":"ok","data":{"protocolVersion":1,"packageName":"chat.xiaoxin.app","deviceOwner":false,"configured":true,
    "enabled":false,"active":false,"activityReady":true,"lockTaskMode":"none","storageConfirmed":true}})}
fn snapshot(ip:&str,mode:&str)->String {
    if mode.is_empty() || mode=="old" || mode=="old-full" || (mode=="mixed" && ip=="192.0.2.3") {return packet(json!({"status":"failed","code":"UNSUPPORTED"}))}
    if mode=="forbidden" {return packet(json!({"status":"failed","code":"FORBIDDEN"}))}
    if mode=="malformed" {return "Result: Bundle[{result=%%%}]".into()}
    let mut response=json!({"status":"ok","data":{"snapshotVersion":1,"packageName":"chat.xiaoxin.app","capabilities":capabilities(),
        "config":{"status":"ok","data":config(ip)},
        "home":home_response(),"kiosk":kiosk_response()}});
    response["data"]["config"]["data"]["authCode"]=json!("must-not-project");
    response["data"]["capabilities"]["data"]["privateCapability"]=json!("must-not-project");
    match mode {
        "large"=>response["data"]["extra"]=json!("x".repeat(300*1024)),
        "home-failed"=>response["data"]["home"]=json!({"status":"failed","code":"UNSUPPORTED"}),
        "home-incomplete"=>{response["data"]["home"]["data"].as_object_mut().unwrap().remove("resolvedHome");},
        "config-failed"=>response["data"]["config"]=json!({"status":"failed","code":"STORAGE_ERROR"}),
        "capabilities-failed"=>response["data"]["capabilities"]=json!({"status":"failed","code":"NOT_READY"}),
        "kiosk-incomplete"=>{response["data"]["kiosk"]["data"].as_object_mut().unwrap().remove("configured");},
        "kiosk-active-missing"=>{response["data"]["kiosk"]["data"].as_object_mut().unwrap().remove("active");},
        "kiosk-enabled-missing"=>{response["data"]["kiosk"]["data"].as_object_mut().unwrap().remove("enabled");},
        "kiosk-missing"=>{response["data"].as_object_mut().unwrap().remove("kiosk");},
        _=>{},
    }
    packet(response)
}
impl DeviceCommandPort for MockCommands {
    async fn execute(&self,program:&Path,args:Vec<String>,_:Duration,_:CancellationToken)->AppResult<ProcessOutput> {
        let ip=args.windows(2).find(|p|p[0]=="-s" || p[0]=="connect").map(|p|p[1].split(':').next().unwrap().to_string())
            .or_else(||args.last().filter(|v|v.parse::<std::net::Ipv4Addr>().is_ok()).cloned()).expect("only explicit mock targets");
        let number=ip.rsplit('.').next().unwrap().parse::<u8>().unwrap();
        let method=args.windows(2).find(|p|p[0]=="--method").map(|p|p[1].clone());
        let label=method.clone().unwrap_or_else(||args.last().cloned().unwrap_or_default());
        self.calls.lock().unwrap().entry(ip.clone()).or_default().push(label);
        tokio::time::sleep(Duration::from_millis(2+(6-number%6) as u64)).await;
        let stdout=if program.file_name().unwrap().to_string_lossy().to_ascii_lowercase().starts_with("ping") {
            format!("Reply from {ip}: bytes=32 time<1ms TTL=64")
        } else if args.first().is_some_and(|v|v=="connect") {"connected".into()}
        else if args.last().is_some_and(|v|v=="get-state") {
            if self.failed_ip.as_deref()==Some(&ip) {return Err(AppError::Conflict("模拟目标断线".into()))} "device".into()
        } else if let Some(method)=method {
            if method=="readManagementSnapshot" && self.snapshot_mode=="transport" {return Err(AppError::Conflict("模拟聚合连接失败".into()))}
            if method=="readManagementSnapshot" && self.snapshot_mode=="failed-exit" {return Ok(ProcessOutput {success:false,stdout:packet(json!({"status":"failed","code":"UNSUPPORTED"})),stderr:String::new()})}
            match method.as_str() {
                "readManagementSnapshot"=>snapshot(&ip,self.snapshot_mode),
                "getCapabilities"=>packet(capabilities()),
                "readConfig"=>packet(json!({"status":"ok","data":config(&ip)})),
                "readHome" if self.snapshot_mode=="old-full"=>packet(home_response()),
                "readKiosk" if self.snapshot_mode=="old-full"=>packet(kiosk_response()),
                "readHome"|"readKiosk"=>packet(json!({"status":"failed","code":"UNSUPPORTED"})),
                _=>panic!("只读预检不得调用{method}"),
            }
        } else if args.iter().any(|v|v.contains("/sys/class/net/eth0")) {format!("02:11:22:33:44:{number:02X}")}
        else if args.iter().any(|v|v=="getprop") {match args.last().unwrap().as_str() {
            "ro.build.version.sdk"=>"27","ro.build.version.release"=>"8.1.0","ro.product.cpu.abilist"=>"arm64-v8a",
            "persist.adb.tcp.port"=>"5555","persist.sys.timezone"=>"Asia/Shanghai",_=>"test-firmware"}.into()}
        else if args.iter().any(|v|v=="pm") {"package:/mock/xiaoxin.apk".into()}
        else if args.ends_with(&["dumpsys".into(),"package".into(),"chat.xiaoxin.app".into()]) {"versionName=2.1.2\nversionCode=20034 minSdk=24".into()}
        else if args.iter().any(|v|v.contains("__INX_XIAOXIN_PIDOF_EXIT__")) {"42\n__INX_XIAOXIN_PIDOF_EXIT__=0\n".into()}
        else if args.iter().any(|v|v=="pidof") {"42".into()}
        else if args.iter().any(|v|v=="device_policy") {
            if self.snapshot_mode=="owner-failed" {return Err(AppError::Conflict("模拟系统Owner读取拒绝".into()))}
            "Current Device Policy Manager state:\nEnabled Device Admins (User 0):\n".into()
        }
        else if args.ends_with(&["dumpsys".into(),"activity".into(),"activities".into()]) {include_str!("fixtures/home_runtime/android10_before_activation.txt").into()}
        else if args.last().is_some_and(|v|v=="id") {"uid=0(root) gid=0(root)".into()}
        else if args.last().is_some_and(|v|v=="/data/system/users/0/package-restrictions.xml") {"<package-restrictions><persistent-preferred-activities /></package-restrictions>".into()}
        else if args.iter().any(|v|v=="resolve-activity") {"No activity found".into()}
        else if args.iter().any(|v|v=="df") {"/dev/block/data 100000 1000 99000 1% /data".into()}
        else if args.iter().any(|v|v=="date") {now()}
        else if args.windows(3).any(|v|v==["settings","get","global"]) {"1".into()}
        else {panic!("未声明的模拟命令，禁止调用真实进程：{args:?}")};
        Ok(ProcessOutput {success:true,stdout,stderr:String::new()})
    }
}
fn input(ids:Vec<String>,action:&str,concurrency:u32)->ScreenOperationInput {
    ScreenOperationInput {action:action.into(),target_ids:ids,application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),
        reinstall:false,concurrency,expected_targets:Default::default(),retry_of_operation_id:None}
}
fn patches(ids:&[String])->BTreeMap<String,AppConfigPatch> {
    ids.iter().enumerate().map(|(index,id)|(id.clone(),AppConfigPatch {set:BTreeMap::from([("customDeviceName".into(),json!(format!("target-{index}")))]),..Default::default()})).collect()
}
async fn assets(state:&inxaiot_desk_buddy_lib::formal::app_state::FormalAppState)->(String,Vec<String>) {
    let project=Stage75Adapter::new(state).create_project(support::local_input()).await.unwrap().project.id;
    let repo=ScreenRepository::new(state.local_store.pool().clone());let mut ids=Vec::new();
    for i in 1..=6 {ids.push(repo.save_local(&project,&ScreenFields {name:format!("screen{i}"),ip:format!("192.0.2.{i}"),
        mac:format!("0211223344{i:02X}"),size:"4".into(),..Default::default()},None,None).await.unwrap());}
    ids.reverse();(project,ids)
}
fn device(mock:&MockCommands,counts:&Arc<measured::Counts>)->AdbDevice<measured::Counted<MockCommands>> {
    AdbDevice::with_commands(AndroidTools {adb:"mock-never-executed".into()},measured::Counted {inner:mock.clone(),counts:counts.clone()})
}
async fn isolated_measurement()->tokio::sync::MutexGuard<'static,()> {
    static LOCK:std::sync::OnceLock<tokio::sync::Mutex<()>>=std::sync::OnceLock::new();
    LOCK.get_or_init(||tokio::sync::Mutex::new(())).lock().await
}

#[tokio::test]
async fn read_batches_keep_ids_and_single_screen_failure_while_global_invokes_share_three_slots() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mock=MockCommands {failed_ip:Some("192.0.2.3".into()),..Default::default()};let counts=Arc::new(measured::Counts::default());
    let (a,b)=tokio::join!(app_config::read_with_device_factory(&state,&project,&ids,||Ok(device(&mock,&counts))),
        app_config::read_with_device_factory(&state,&project,&ids[..2],||Ok(device(&mock,&counts))));
    let a=a.unwrap();let b=b.unwrap();assert_eq!(a.iter().map(|r|&r.screen_id).collect::<Vec<_>>(),ids.iter().collect::<Vec<_>>());
    assert_eq!(b.iter().map(|r|&r.screen_id).collect::<Vec<_>>(),ids[..2].iter().collect::<Vec<_>>());
    assert_eq!(a.iter().filter(|r|r.config.is_none()).count(),1);
    for (i,row) in a.iter().enumerate() {if let Some(config)=&row.config {assert_eq!(config["customDeviceName"],format!("192.0.2.{}",6-i));}}
    assert!(counts.snapshot().max_concurrent_commands<=3 && counts.snapshot().max_concurrent_commands>1);
    support::close(state).await;
}

#[tokio::test]
async fn preflight_parallel_and_serial_have_same_per_screen_sequence_and_ordered_plan_targets() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mut logs=Vec::new();
    for concurrency in [1,3] {
        let mock=MockCommands {failed_ip:Some("192.0.2.3".into()),..Default::default()};let counts=Arc::new(measured::Counts::default());
        let preview=maintenance::preflight_with_device_factory(&state,&project,input(ids.clone(),"app_config",concurrency),Some(patches(&ids)),None,
            ||Ok(device(&mock,&counts))).await.unwrap();
        assert_eq!(preview.items.iter().map(|v|&v.screen_id).collect::<Vec<_>>(),ids.iter().collect::<Vec<_>>());
        assert_eq!(preview.items.iter().filter(|v|v.state=="blocked").count(),1);
        let (plan,_)=task_data::read_preflight_plan(state.local_store.pool(),&project,&preview.id).await.unwrap();
        assert_eq!(plan.targets.iter().map(|v|&v.id).collect::<Vec<_>>(),preview.items.iter().filter(|v|v.state=="ready").map(|v|&v.screen_id).collect::<Vec<_>>());
        assert_eq!(plan.detail["observations"].as_object().unwrap().len(),6);
        assert_eq!(counts.snapshot().max_concurrent_commands,concurrency as usize);
        logs.push(mock.calls.lock().unwrap().clone());
    }
    assert_eq!(logs[0],logs[1]);
    support::close(state).await;
}

#[tokio::test]
async fn other_maintenance_checks_remain_serial_even_when_input_requests_three() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mock=MockCommands::default();let counts=Arc::new(measured::Counts::default());
    let preview=maintenance::preflight_with_device_factory(&state,&project,input(ids.clone(),"restart",3),None,None,||Ok(device(&mock,&counts))).await.unwrap();
    assert!(preview.items.iter().all(|v|v.state=="ready"));assert_eq!(counts.snapshot().max_concurrent_commands,1);
    support::close(state).await;
}

#[tokio::test]
async fn complete_batch_size_and_missing_id_checks_run_before_adapter_creation() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let repo=ScreenRepository::new(state.local_store.pool().clone());let mut asset=repo.asset(&project,&ids[0]).await.unwrap();asset.fields.size="10".into();
    repo.save_local(&project,&asset.fields,Some(&asset.id),Some(asset.revision)).await.unwrap();
    let created=AtomicUsize::new(0);let mock=MockCommands::default();let counts=Arc::new(measured::Counts::default());
    assert!(app_config::read_with_device_factory(&state,&project,&ids,||{created.fetch_add(1,Ordering::SeqCst);Ok(device(&mock,&counts))}).await.is_err());
    assert!(maintenance::preflight_with_device_factory(&state,&project,input(ids.clone(),"app_config",3),Some(patches(&ids)),None,
        ||{created.fetch_add(1,Ordering::SeqCst);Ok(device(&mock,&counts))}).await.is_err());
    assert!(app_config::read_with_device_factory(&state,&project,&["missing".into()],||{created.fetch_add(1,Ordering::SeqCst);Ok(device(&mock,&counts))}).await.is_err());
    assert_eq!(created.load(Ordering::SeqCst),0);assert_eq!(counts.snapshot().total,0);
    support::close(state).await;
}

#[tokio::test]
async fn reading_and_configuration_preflight_share_one_global_limit() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mock=MockCommands::default();let counts=Arc::new(measured::Counts::default());
    let (read,checked)=tokio::join!(app_config::read_with_device_factory(&state,&project,&ids,||Ok(device(&mock,&counts))),
        maintenance::preflight_with_device_factory(&state,&project,input(ids.clone(),"app_config",3),Some(patches(&ids)),None,||Ok(device(&mock,&counts))));
    assert_eq!(read.unwrap().len(),6);assert!(checked.unwrap().items.iter().all(|item|item.state=="ready"));
    assert_eq!(counts.snapshot().max_concurrent_commands,3);
    support::close(state).await;
}

#[tokio::test]
async fn fatal_setup_failure_creates_neither_preview_nor_device_commands() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let error=maintenance::preflight_with_device_factory(&state,&project,input(ids.clone(),"app_config",3),Some(patches(&ids)),None,
        ||->AppResult<AdbDevice<MockCommands>> {Err(AppError::Conflict("模拟基础设施不可用".into()))}).await.unwrap_err();
    assert!(error.to_string().contains("模拟基础设施"));
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM local_screen_task_data").fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(count,0);assert!(state.task_repository.list_recent(&project,100).await.unwrap().is_empty());
    support::close(state).await;
}

#[tokio::test]
async fn snapshot_and_explicit_legacy_fallback_return_the_same_public_values_and_keep_system_checks() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mut values=Vec::new();
    for mode in ["new","old-full"] {
        let mock=MockCommands {snapshot_mode:mode,..Default::default()};let counts=Arc::new(measured::Counts::default());
        let rows=app_config::read_with_device_factory(&state,&project,&ids[..1],||Ok(device(&mock,&counts))).await.unwrap();
        let row=&rows[0];assert!(row.config.is_some());assert!(row.home.as_ref().unwrap().supported);assert!(row.kiosk.as_ref().unwrap().supported);
        assert_eq!(row.owner.as_ref().unwrap().state,"none");
        let mut value=serde_json::to_value(row).unwrap();value.as_object_mut().unwrap().remove("readAt");
        assert!(!value.to_string().contains("must-not-project"));values.push(value);
        let methods=counts.snapshot().provider_methods;
        assert_eq!(methods.get("readManagementSnapshot"),Some(&1));
        assert_eq!(methods.len(),if mode=="new" {1} else {5});
        if mode=="old-full" {for method in ["getCapabilities","readConfig","readHome","readKiosk"] {assert_eq!(methods.get(method),Some(&1));}}
        let calls=mock.calls.lock().unwrap();let calls=&calls["192.0.2.6"];
        for expected in ["id","/data/system/users/0/package-restrictions.xml","android.intent.category.DEFAULT","activities","device_policy"] {
            assert!(calls.iter().any(|call|call==expected),"missing independent system read: {expected}");
        }
    }
    assert_eq!(values[0],values[1]);support::close(state).await;
}

#[tokio::test]
async fn forbidden_broken_oversized_or_failed_transport_never_triggers_legacy_reads() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    for mode in ["forbidden","malformed","large","transport","failed-exit"] {
        let mock=MockCommands {snapshot_mode:mode,..Default::default()};let counts=Arc::new(measured::Counts::default());
        let row=app_config::read_with_device_factory(&state,&project,&ids[..1],||Ok(device(&mock,&counts))).await.unwrap().remove(0);
        assert!(row.config.is_none() && row.home.is_none() && row.kiosk.is_none() && row.owner.is_none());
        assert_eq!(counts.snapshot().provider_methods,BTreeMap::from([("readManagementSnapshot".into(),1)]));
    }
    support::close(state).await;
}

#[tokio::test]
async fn snapshot_segment_failures_remain_independent_and_never_invent_false_values() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    for mode in ["home-failed","home-incomplete","config-failed","capabilities-failed","kiosk-incomplete","kiosk-missing","kiosk-active-missing","kiosk-enabled-missing","owner-failed"] {
        let mock=MockCommands {snapshot_mode:mode,..Default::default()};let counts=Arc::new(measured::Counts::default());
        let row=app_config::read_with_device_factory(&state,&project,&ids[..1],||Ok(device(&mock,&counts))).await.unwrap().remove(0);
        assert_eq!(row.config.is_none(),["config-failed","capabilities-failed"].contains(&mode));
        assert_eq!(row.capabilities.is_none(),mode=="capabilities-failed");
        let invalid_home=mode.starts_with("home-");let home=row.home.unwrap();assert_eq!(home.supported,!invalid_home);
        if invalid_home {assert_eq!(home.enabled,None);assert_eq!(home.active,None);}
        let kiosk=row.kiosk.unwrap();let invalid_kiosk=mode.starts_with("kiosk-");
        assert_eq!(kiosk.supported,!invalid_kiosk);
        if invalid_kiosk {assert_eq!(kiosk.configured,None);assert_eq!(kiosk.enabled,None);assert_eq!(kiosk.active,None);}
        assert_eq!(row.owner.unwrap().state,if mode=="owner-failed" {"unknown"} else {"none"});
        assert_eq!(counts.snapshot().provider_methods,BTreeMap::from([("readManagementSnapshot".into(),1)]));
    }
    support::close(state).await;
}

#[tokio::test]
async fn mixed_new_and_old_screens_keep_order_and_only_the_old_screen_falls_back() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mock=MockCommands {snapshot_mode:"mixed",..Default::default()};let counts=Arc::new(measured::Counts::default());
    let rows=app_config::read_with_device_factory(&state,&project,&ids,||Ok(device(&mock,&counts))).await.unwrap();
    assert_eq!(rows.iter().map(|row|&row.screen_id).collect::<Vec<_>>(),ids.iter().collect::<Vec<_>>());assert!(rows.iter().all(|row|row.config.is_some()));
    for (ip,methods) in counts.snapshot().provider_methods_by_ip {
        assert_eq!(methods.get("readManagementSnapshot"),Some(&1));assert_eq!(methods.len(),if ip=="192.0.2.3" {5} else {1});
    }
    assert!(counts.snapshot().max_concurrent_commands<=3);support::close(state).await;
}

#[tokio::test]
async fn write_side_read_device_and_preflight_keep_the_original_protocol_on_new_apps() {
    let _measurement=isolated_measurement().await;
    let dir=tempfile::tempdir().unwrap();let state=support::state_at(dir.path(),false).await;let(project,ids)=assets(&state).await;
    let mock=MockCommands {snapshot_mode:"new",..Default::default()};let counts=Arc::new(measured::Counts::default());
    app_config::read_device(&device(&mock,&counts),"192.0.2.6",None).await.unwrap();
    let preview=maintenance::preflight_with_device_factory(&state,&project,input(ids[..1].to_vec(),"app_config",3),Some(patches(&ids[..1])),None,
        ||Ok(device(&mock,&counts))).await.unwrap();assert_eq!(preview.items[0].state,"ready");
    let methods=counts.snapshot().provider_methods;assert!(!methods.contains_key("readManagementSnapshot"));
    assert_eq!(methods.get("getCapabilities"),Some(&2));assert_eq!(methods.get("readConfig"),Some(&2));
    support::close(state).await;
}
