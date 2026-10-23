//! 第一阶段只读性能/身份回归：串行与并行调用相同读取、预检核心，绝不提交设备写任务。
#[path="common/screen_test_support.rs"] mod support;
#[path="common/screen_configuration_read_support.rs"] mod measured;
use inxaiot_desk_buddy_lib::{application::ports::project_management::ProjectManagementPort,core::error::{AppError,AppResult},
    domain::smart_screen::{app_config::{AppConfigPatch,AppConfigRead},model::ScreenFields,operation::ScreenOperationInput,rules::{normalize_mac,valid_mac}},
    formal::app_state::FormalAppState,infrastructure::{local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{app_config,maintenance,device::{AdbDevice,AndroidTools,DeviceCommandPort,NativeDeviceCommands,ProcessOutput}},stage75_adapter::Stage75Adapter}};
use futures_util::{StreamExt,stream};
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use std::{collections::BTreeMap,path::{Path,PathBuf},sync::Arc,time::{Duration,Instant}};
use tokio_util::sync::CancellationToken;
type TestResult<T>=Result<T,Box<dyn std::error::Error>>;
const TARGETS:[(&str,&str,&str);3]=[("192.168.3.42","9c000c3c3643071205d","10"),("192.168.3.74","A47A4AK02040","4"),("192.168.3.82","A47A5CK00524","4")];
fn require(ok:bool,message:&str)->TestResult<()> {if ok {Ok(())} else {Err(message.to_string().into())}}
fn sha(value:&Value)->String {format!("{:x}",Sha256::digest(serde_json::to_vec(value).unwrap()))}

#[derive(Clone)] struct ReadOnlyCommands;
fn allowed_query(program:&Path,args:&[String])->bool {
    let target=|value:&str|TARGETS.iter().any(|(ip,_,_)|*ip==value);
    let words=args.iter().map(String::as_str).collect::<Vec<_>>();
    if program.file_name().is_some_and(|name|["ping","ping.exe"].contains(&name.to_string_lossy().to_ascii_lowercase().as_str())) {
        return words.len()==5 && target(words[4]) && [vec!["-n","1","-w","2000"],vec!["-c","1","-W","2000"],vec!["-c","1","-W","2"]].iter().any(|prefix|words[..4]==prefix[..]);
    }
    if let ["connect",address]=words.as_slice() {return address.strip_suffix(":5555").is_some_and(target)}
    if words.len()<3 || words[0]!="-s" || !words[1].strip_suffix(":5555").is_some_and(target) {return false}
    if words[2..]==["get-state"] {return true}
    if words[2]!="shell" {return false}
    match &words[3..] {
        ["getprop",key]=>["ro.serialno","ro.build.version.release","ro.product.model","ro.build.fingerprint","ro.build.version.sdk",
            "ro.product.cpu.abilist","persist.adb.tcp.port","persist.sys.timezone"].contains(key),
        ["pm","path","chat.xiaoxin.app"]|["dumpsys","package","chat.xiaoxin.app"]|["pidof","chat.xiaoxin.app"]
            |["dumpsys","device_policy"]|["dumpsys","activity","activities"]|["dumpsys","activity"]|["dumpsys","power"]
            |["df","-k","/data"]|["date","-u","+%Y-%m-%dT%H:%M:%SZ"]|["id"]|["su","0","id"]
            |["cat","/data/system/users/0/package-restrictions.xml"]|["su","0","cat","/data/system/users/0/package-restrictions.xml"]=>true,
        ["settings","get","global",key]=>["auto_time","auto_time_zone"].contains(key),
        ["content","call","--uri",uri,"--method",method]=>match *uri {
            "content://chat.xiaoxin.app.workbench.config"=>["getCapabilities","readConfig","readKiosk","readManagementSnapshot"].contains(method),
            "content://chat.xiaoxin.app.workbench.home"=>*method=="readHome",
            _=>false,
        },
        ["cmd","package","resolve-activity","--brief","--user","0","-a","android.intent.action.MAIN","-c","android.intent.category.HOME","-c","android.intent.category.DEFAULT"]=>true,
        [script]=>*script=="if [ ! -d /sys/class/net/eth0 ] || [ -d /sys/class/net/eth0/wireless ] || [ -e /sys/class/net/eth0/phy80211 ]; then exit 2; fi; read t < /sys/class/net/eth0/type; if [ x$t != x1 ]; then exit 2; fi; cat /sys/class/net/eth0/address"
            || *script==r#"for p in /sys/class/net/eth*; do if [ -r "$p/address" ] && [ ! -d "$p/wireless" ] && [ ! -e "$p/phy80211" ]; then read t < "$p/type"; if [ x$t = x1 ]; then echo "${p##*/}=$(cat "$p/address")"; fi; fi; done"#,
        _=>false,
    }
}
impl DeviceCommandPort for ReadOnlyCommands {
    async fn execute(&self,program:&Path,args:Vec<String>,limit:Duration,cancel:CancellationToken)->AppResult<ProcessOutput> {
        if !allowed_query(program,&args) {
            return Err(AppError::InvalidConfig("只读性能回归禁止设备修改或界面动作".into()));
        }
        NativeDeviceCommands.execute(program,args,limit,cancel).await
    }
}
fn device(tools:&AndroidTools,counts:&Arc<measured::Counts>)->AdbDevice<measured::Counted<ReadOnlyCommands>> {
    AdbDevice::with_commands(tools.clone(),measured::Counted {inner:ReadOnlyCommands,counts:counts.clone()})
}
fn input(ids:&[String],concurrency:u32)->ScreenOperationInput {
    ScreenOperationInput {action:"app_config".into(),target_ids:ids.to_vec(),application_id:None,apk:None,app_version:String::new(),abi:"universal".into(),
        reinstall:false,concurrency,expected_targets:Default::default(),retry_of_operation_id:None}
}
fn summarize(row:&AppConfigRead)->Value {
    json!({"screenId":row.screen_id,"readSucceeded":row.config.is_some(),"versionName":row.capabilities.as_ref().map(|v|v["appVersionName"].clone()),
        "versionCode":row.capabilities.as_ref().map(|v|v["appVersionCode"].clone()),"configSha256":row.config.as_ref().map(sha),
        "homeSupported":row.home.as_ref().map(|v|v.supported),"kioskSupported":row.kiosk.as_ref().map(|v|v.supported),"ownerState":row.owner.as_ref().map(|v|&v.state)})
}
async fn reads(state:&FormalAppState,project:&str,ids:&[String],tools:&AndroidTools,parallel:bool)->TestResult<(Vec<AppConfigRead>,Value)> {
    let counts=Arc::new(measured::Counts::default());let started=Instant::now();
    let operation=|id:String| {let counts=counts.clone();async move {
        let tick=Instant::now();let result=app_config::read_with_device_factory(state,project,&[id.clone()],||Ok(device(tools,&counts))).await;
        (id,tick.elapsed().as_millis(),result)
    }};
    let mut pending=stream::iter(ids.iter().cloned().map(operation)).buffer_unordered(if parallel {3} else {1});
    let mut returned=BTreeMap::new();let mut timings=BTreeMap::new();
    while let Some((id,elapsed,result))=pending.next().await {
        let mut rows=result?;require(rows.len()==1 && rows[0].screen_id==id,"单ID读取发生目标错配")?;
        returned.insert(id.clone(),rows.remove(0));timings.insert(id,elapsed);
    }
    let rows=ids.iter().map(|id|returned.remove(id).expect("one result per id")).collect::<Vec<_>>();
    let report=json!({"mode":if parallel {"concurrent-single-id-read"} else {"serial-single-id-read"},"elapsedMs":started.elapsed().as_millis(),
        "commands":counts.snapshot(),"perScreenElapsedMs":timings,"rows":rows.iter().map(summarize).collect::<Vec<_>>()});
    Ok((rows,report))
}
async fn run(state:&FormalAppState,group:&str,directory:&Path,report:&mut Value)->TestResult<()> {
    let descriptions=support::live_screens()?;
    let targets=TARGETS.iter().filter(|(_,_,size)|*size==group).collect::<Vec<_>>();
    require(!targets.is_empty() && targets.iter().all(|(ip,_,size)|descriptions.iter().any(|d|d.ip==*ip && d.size==*size)),"当前测试说明缺少明确目标，未连接设备")?;
    let tools=AndroidTools::discover()?;let prep_counts=Arc::new(measured::Counts::default());let adb=device(&tools,&prep_counts);
    let project=Stage75Adapter::new(state).create_project(support::local_input()).await?.project.id;
    let repo=ScreenRepository::new(state.local_store.pool().clone());let mut ids=Vec::new();let mut identities=Vec::new();
    for (ip,serial,size) in targets {
        adb.connect(ip,CancellationToken::new()).await?;
        let actual=adb.shell(ip,&["getprop","ro.serialno"],CancellationToken::new()).await?;
        require(actual.trim()==*serial,"硬件序列号不匹配，未继续")?;
        let id=repo.save_local(&project,&ScreenFields {name:format!("{size}寸配置并发只读回归"),ip:(*ip).into(),size:(*size).into(),..Default::default()},None,None).await?;
        let asset=repo.asset(&project,&id).await?;let observed=adb.inspect(&asset,"inspect",CancellationToken::new()).await?;
        let mac=observed.observed_mac.as_deref().filter(|v|valid_mac(v)).ok_or("未取得有效有线身份")?;
        let mut fields=asset.fields.clone();fields.mac=normalize_mac(mac);repo.save_local(&project,&fields,Some(&id),Some(asset.revision)).await?;
        identities.push(json!({"id":id,"ip":ip,"size":size,"serialSha256":sha(&json!(serial)),"macSource":observed.mac_source,
            "versionName":observed.observed_app_version,"versionCode":observed.app_version_code}));ids.push(id);
    }
    report["projectId"]=json!(project);report["identities"]=json!(identities);report["measurements"]=json!([]);
    std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(report)?)?;
    let (baseline,serial)=reads(state,&project,&ids,&tools,false).await?;
    report["measurements"].as_array_mut().unwrap().push(serial);
    require(baseline.iter().all(|r|r.config.is_some()),"串行读取存在失败目标，未进入后续预检")?;
    let (parallel,parallel_report)=reads(state,&project,&ids,&tools,true).await?;
    report["measurements"].as_array_mut().unwrap().push(parallel_report);
    require(baseline.iter().zip(&parallel).all(|(a,b)|a.screen_id==b.screen_id && a.config==b.config),"并行读取值或ID与串行不一致")?;
    let patches=baseline.iter().map(|row| {
        let name=&row.config.as_ref().unwrap()["customDeviceName"];
        let patch=if name.is_null() {AppConfigPatch {clear:vec!["customDeviceName".into()],..Default::default()}}
            else {AppConfigPatch {set:BTreeMap::from([("customDeviceName".into(),name.clone())]),..Default::default()}};
        (row.screen_id.clone(),patch)
    }).collect::<BTreeMap<_,_>>();
    for concurrency in [1,3] {
        let counts=Arc::new(measured::Counts::default());let tick=Instant::now();
        let preview=maintenance::preflight_with_device_factory(state,&project,input(&ids,concurrency),Some(patches.clone()),None,||Ok(device(&tools,&counts))).await?;
        report["measurements"].as_array_mut().unwrap().push(json!({"mode":format!("preflight-concurrency-{concurrency}"),"elapsedMs":tick.elapsed().as_millis(),
            "commands":counts.snapshot(),"items":preview.items.iter().map(|item|json!({"screenId":item.screen_id,"state":item.state,"reason":item.reason})).collect::<Vec<_>>()}));
        require(preview.items.iter().map(|item|&item.screen_id).collect::<Vec<_>>()==ids.iter().collect::<Vec<_>>(),"预检返回顺序与目标不一致")?;
        require(preview.items.iter().all(|item|item.state=="ready"),"预检有未通过目标，详情保留在result.json")?;
        require(counts.snapshot().max_concurrent_commands<=3,"命令并发超过三")?;
    }
    let (after,last)=reads(state,&project,&ids,&tools,true).await?;
    report["measurements"].as_array_mut().unwrap().push(last);
    require(baseline.iter().zip(&after).all(|(a,b)|a.config==b.config),"只读/预检阶段改变了配置")?;
    require(state.task_repository.list_recent(&project,100).await?.iter().all(|task|task.operation_type==maintenance::PREVIEW),"只读性能回归意外创建执行任务")?;
    report["passed"]=json!(true);report["deviceWrites"]=json!(0);report["platformConnections"]=json!(0);Ok(())
}

#[tokio::test]
#[ignore="仅INX_CONFIG_READ_REAL=YES与GROUP=4|10时，固定三屏白名单、稳定序列号、新建本机隔离库；绝不提交写任务"]
async fn configuration_read_preflight_real()->TestResult<()> {
    require(std::env::var("INX_CONFIG_READ_REAL").as_deref()==Ok("YES"),"未授权实机只读回归")?;
    let group=std::env::var("INX_CONFIG_READ_GROUP").unwrap_or_default();require(["4","10"].contains(&group.as_str()),"明确选择4或10寸组")?;
    let root=PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().canonicalize()?;
    let base=root.join(".review-tools/config-read-concurrency-20261023");std::fs::create_dir_all(&base)?;
    require(base.canonicalize()?.starts_with(&root),"证据目录不能离开仓库")?;
    let directory=base.join(uuid::Uuid::now_v7().to_string());std::fs::create_dir(&directory)?;
    println!("只读性能隔离目录：{}",directory.display());
    let state=support::state_at(&directory,false).await;let mut report=json!({"group":group,"passed":false,"deviceWrites":0});
    let outcome=run(&state,&group,&directory,&mut report).await;
    if let Err(error)=&outcome {report["error"]=json!(error.to_string());}
    std::fs::write(directory.join("result.json"),serde_json::to_vec_pretty(&report)?)?;
    support::close(state).await;outcome
}

#[test]
fn real_wrapper_only_allows_declared_queries_on_the_three_targets() {
    let command=|tail:&[&str]|[vec!["-s","192.168.3.74:5555","shell"],tail.to_vec()].concat().into_iter().map(String::from).collect::<Vec<_>>();
    assert!(allowed_query(Path::new("adb.exe"),&command(&["getprop","ro.serialno"])));
    assert!(allowed_query(Path::new("adb.exe"),&command(&["content","call","--uri","content://chat.xiaoxin.app.workbench.config","--method","readConfig"])));
    assert!(allowed_query(Path::new("adb.exe"),&command(&["content","call","--uri","content://chat.xiaoxin.app.workbench.config","--method","readManagementSnapshot"])));
    for tail in [vec!["dpm","set-device-owner","component"],vec!["am","start","-n","component"],vec!["pm","clear","chat.xiaoxin.app"],
        vec!["settings","delete","global","key"],vec!["rm","file"],vec!["mv","a","b"],vec!["sh","-c","reboot"],
        vec!["content","call","--uri","content://chat.xiaoxin.app.workbench.home","--method","prepareHomeActivation"]] {
        assert!(!allowed_query(Path::new("adb.exe"),&command(&tail)));
    }
    let mut other=command(&["id"]);other[1]="192.0.2.1:5555".into();assert!(!allowed_query(Path::new("adb.exe"),&other));
}
