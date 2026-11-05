//! 设备检查菜单合并的真实只读回归：原inspect与ping任务，各三屏并发3一次。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::{ResultState, ScreenFields},
            operation::ScreenOperationInput,
            rules::normalize_mac,
        },
    },
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools},
            task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const GUARDS: &[(&str, &str, &str, &str, &str)] = &[
    (
        "192.168.3.42",
        "10",
        "9c000c3c3643071205d",
        "62:91:4c:93:81:57",
        "26:da:35:7d:85:b5",
    ),
    (
        "192.168.3.74",
        "4",
        "A47A4AK02040",
        "28:2d:06:15:59:ac",
        "ee:b4:a7:44:9c:af",
    ),
    (
        "192.168.3.82",
        "4",
        "A47A5CK00524",
        "40:fd:f3:01:ae:2c",
        "66:b3:35:a7:d8:67",
    ),
];
const KEYS: &[(&str, &str)] = &[
    ("global", "ntp_server"),
    ("global", "auto_time"),
    ("global", "auto_time_zone"),
    ("global", "stay_on_while_plugged_in"),
    ("system", "screen_off_timeout"),
    ("secure", "sleep_timeout"),
    ("secure", "screensaver_enabled"),
    ("secure", "screensaver_activate_on_sleep"),
    ("secure", "screensaver_activate_on_dock"),
    ("system", "screen_brightness"),
    ("system", "screen_brightness_mode"),
];
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    ip: String,
    serial: String,
    wifi_mac: String,
    wired_mac: String,
    boot_id: String,
    settings: BTreeMap<String, String>,
    properties: BTreeMap<String, String>,
    app_pids: Vec<u32>,
}
fn require(value: bool, message: &str) -> TestResult<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn save(directory: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}
async fn read(device: &AdbDevice, ip: &str, args: &[&str]) -> TestResult<String> {
    let mut all = vec!["shell"];
    all.extend_from_slice(args);
    let out = device
        .adb(ip, &all, Duration::from_secs(15), CancellationToken::new())
        .await
        .map_err(|error| format!("{ip} 只读命令 {} 失败：{error}", args.join(" ")))?;
    require(
        out.success && out.stderr.is_empty(),
        "限定只读命令未完成；不写设置、不启动或重启来修复",
    )?;
    Ok(out.stdout.trim().into())
}
async fn snapshot(
    device: &AdbDevice,
    guard: &(&str, &str, &str, &str, &str),
) -> TestResult<Snapshot> {
    let (ip, _, serial, wifi, wired) = *guard;
    device
        .connect(ip, CancellationToken::new())
        .await
        .map_err(|error| format!("{ip} 固定地址ADB连接确认失败：{error}"))?;
    let actual_serial = read(device, ip, &["getprop", "ro.serialno"]).await?;
    let actual_wifi =
        normalize_mac(&read(device, ip, &["cat", "/sys/class/net/wlan0/address"]).await?);
    let actual_wired =
        normalize_mac(&read(device, ip, &["cat", "/sys/class/net/eth0/address"]).await?);
    require(
        actual_serial == serial
            && actual_wifi == normalize_mac(wifi)
            && actual_wired == normalize_mac(wired),
        "指定IP的真实serial/双MAC与授权白名单不符",
    )?;
    let boot_id = read(device, ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?;
    uuid::Uuid::parse_str(&boot_id)?;
    let mut settings = BTreeMap::new();
    for (ns, key) in KEYS {
        settings.insert(
            format!("{ns}.{key}"),
            read(device, ip, &["settings", "get", ns, key]).await?,
        );
    }
    let mut properties = BTreeMap::new();
    for key in [
        "persist.adb.tcp.port",
        "service.adb.tcp.port",
        "persist.sys.timezone",
    ] {
        properties.insert(key.into(), read(device, ip, &["getprop", key]).await?);
    }
    let out = device
        .adb(
            ip,
            &["shell", "pidof", "chat.xiaoxin.app"],
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await?;
    require(
        out.stderr.is_empty() && (out.success || out.stdout.trim().is_empty()),
        "原APP运行状态未可靠取得",
    )?;
    let mut app_pids = out
        .stdout
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    app_pids.sort_unstable();
    app_pids.dedup();
    Ok(Snapshot {
        ip: ip.into(),
        serial: actual_serial,
        wifi_mac: actual_wifi,
        wired_mac: actual_wired,
        boot_id,
        settings,
        properties,
        app_pids,
    })
}
fn input(action: &str, ids: &[String]) -> ScreenOperationInput {
    ScreenOperationInput {
        action: action.into(),
        target_ids: ids.to_vec(),
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 3,
        retry_of_operation_id: None,
        expected_targets: BTreeMap::new(),
    }
}

#[tokio::test]
#[ignore = "需INX_SCREEN_INSPECTION_REAL=1；固定42/74/82真实SDK，隔离库inspect与ping正式队列各一次，只读不启动/设置/重启"]
async fn full_inspect_and_network_only_ping_preserve_three_screens() -> TestResult<()> {
    require(
        std::env::var("INX_SCREEN_INSPECTION_REAL").as_deref() == Ok("1"),
        "必须显式允许本轮只读实机检查",
    )?;
    let sdk =
        std::path::PathBuf::from("D:/env/AndroidSdk/platform-tools/adb.exe").canonicalize()?;
    let tools = AndroidTools::discover()?;
    require(
        tools.adb.canonicalize()? == sdk,
        "必须使用明确的真实SDK工具，不能fixture或代理",
    )?;
    let live = support::live_screens()?;
    require(
        live.len() == GUARDS.len()
            && GUARDS.iter().all(|(ip, size, _, wifi, _)| {
                live.iter()
                    .filter(|s| {
                        s.ip == *ip
                            && s.size == *size
                            && normalize_mac(&s.mac) == normalize_mac(wifi)
                    })
                    .count()
                    == 1
            }),
        "当前说明必须恰为授权三屏，不扫描或回退其它地址",
    )?;
    let directory = support::evidence_dir("inspection-modes-readonly")?;
    eprintln!("只读检查两模式实机证据：{}", directory.display());
    let state = support::state_at(&directory, true).await;
    let device = AdbDevice::new(tools);
    let mut report = json!({"passed":false,"readOnly":true,"nativeSdk":sdk,"platformConnectionsRequested":false,"productionInstanceOpened":false,"deviceStartsRequested":0,"deviceSettingsWritesRequested":0,"deviceRebootsRequested":0,"concurrency":3,"runs":[]});
    let outcome:TestResult<()>=async {
        let before=futures_util::future::try_join_all(GUARDS.iter().map(|g|snapshot(&device,g))).await?;
        report["before"]=serde_json::to_value(&before)?;save(&directory,&report)?;
        let project=Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let repository=ScreenRepository::new(state.local_store.pool().clone());let mut ids=Vec::new();
        for (ip,size,_,_,wired) in GUARDS {
            ids.push(repository.save_local(&project,&ScreenFields{name:format!("只读检查{ip}"),ip:(*ip).into(),mac:normalize_mac(wired),size:(*size).into(),..Default::default()},None,None).await?);
        }
        let initial=repository.snapshot(&project).await?;
        require(initial.screens.iter().all(|s|s.source=="local"),"本轮只允许本机未注册目标")?;
        report["projectId"]=json!(project);report["targetIds"]=json!(ids);save(&directory,&report)?;
        let mut inspect_ids=BTreeMap::new();
        for action in ["inspect","ping"] {
            let request=input(action,&ids);let preview=tasks::preflight(&state,&project,request.clone()).await?;
            require(preview.items.len()==3&&preview.items.iter().all(|i|i.state=="ready")&&preview.items.iter().map(|i|&i.screen_id).collect::<Vec<_>>()==ids.iter().collect::<Vec<_>>(),"正式预检目标/ID或可执行状态不符")?;
            let (plan,_)=task_data::read_preflight_plan(state.local_store.pool(),&project,&preview.id).await?;
            require(plan.input.action==action&&plan.input.concurrency==3&&plan.targets.len()==3&&plan.targets.iter().all(|t|t.source=="local"),"预检计划未冻结所需模式/并发/本机目标")?;
            let task=tasks::submit(&state,&project,&preview.id,request).await?;
            report["runs"].as_array_mut().unwrap().push(json!({"action":action,"taskId":task,"preflight":preview}));save(&directory,&report)?;
            let final_state=tokio::time::timeout(Duration::from_secs(180),async {loop {
                let record=state.task_repository.get(&task).await?;
                if record.state.is_terminal()||record.state==TaskState::FinalizingFailed {return Ok::<_,inxaiot_desk_buddy_lib::core::error::AppError>(record.state);}
                tokio::time::sleep(Duration::from_millis(250)).await;
            }}).await??;
            let results=task_data::read_results(state.local_store.pool(),&project,&task).await?;
            let entry=report["runs"].as_array_mut().unwrap().last_mut().unwrap();entry["taskState"]=json!(final_state.as_str());entry["results"]=serde_json::to_value(&results)?;save(&directory,&report)?;
            require(final_state==TaskState::Succeeded&&results.finished&&results.targets.len()==3,"正式只读任务未完整成功；不重发或用写操作修复")?;
            require(state.task_repository.get(&task).await?.remote_operation_record_id.is_none(),"本机检查不应创建远端操作记录")?;
            for (index,id) in ids.iter().enumerate() {
                let target=results.targets.get(id).ok_or("结果目标ID缺失")?;
                let observed=target.observation.as_ref().ok_or("检查没有真实观测")?;
                require(target.device==ResultState::Succeeded&&target.business==ResultState::NotRequired&&target.shared==ResultState::NotRequired,"本机检查不得写业务状态/版本或共享库")?;
                require(observed.screen_id==*id&&observed.task_id.as_deref()==Some(task.as_str())&&observed.operation_type==action&&observed.observed_ip==GUARDS[index].0&&!observed.id.is_empty()&&matches!(observed.ping.as_deref(),Some("online"|"offline")),"Ping事实/观测/任务ID不一致")?;
                if action=="inspect" {
                    require(observed.adb_available&&observed.errors.is_empty()&&observed.observed_mac.as_deref().is_some_and(|m|normalize_mac(m)==normalize_mac(GUARDS[index].4))&&observed.sdk==Some(if GUARDS[index].1=="4"{27}else{29})&&!observed.abis.is_empty()&&observed.android.is_some()&&observed.device_model.is_some()&&observed.firmware.is_some()&&observed.app_installed==Some(true)&&observed.observed_app_version.is_some()&&observed.app_version_code.is_some()&&observed.free_space_mb.is_some()&&observed.device_time.is_some()&&observed.computer_time.is_some()&&observed.timezone.is_some()&&observed.automatic_time.is_some()&&observed.automatic_timezone.is_some(),"完整inspect缺少ADB/硬件/APP/时间信息或有读取错误")?;
                    inspect_ids.insert(id.clone(),observed.id.clone());
                } else {
                    require(!observed.adb_available&&observed.android.is_none()&&observed.observed_mac.is_none()&&observed.observed_app_version.is_none()&&observed.device_time.is_none()&&observed.app_running.is_none(),"网络模式不应伪造新的ADB/系统/APP/时间检查数据")?;
                }
            }
            let current=repository.snapshot(&project).await?;
            for old in &initial.screens {
                let current_asset=current.screens.iter().find(|s|s.id==old.id).ok_or("本机资产ID变化")?;
                require(current_asset.fields==old.fields&&current_asset.platform_status==old.platform_status&&current_asset.app_version==old.app_version,"只读任务改变资产配置/业务状态/业务版本")?;
                if action=="ping" {require(current.observations[&old.id].iter().any(|o|Some(&o.id)==inspect_ids.get(&old.id)&&o.operation_type=="inspect"&&o.adb_available),"仅网络检查覆盖了原完整检查历史")?;}
            }
            let after=futures_util::future::try_join_all(GUARDS.iter().map(|g|snapshot(&device,g))).await?;
            report["runs"].as_array_mut().unwrap().last_mut().unwrap()["afterSnapshot"]=serde_json::to_value(&after)?;
            let unchanged=before==after;report["runs"].as_array_mut().unwrap().last_mut().unwrap()["configurationIdentityBootAndAppPidUnchanged"]=json!(unchanged);save(&directory,&report)?;
            require(unchanged,"检查前后设备身份/配置/boot/APP进程发生变化，仅保留事实、不写回猜测修复")?;
            eprintln!("模式{action}三屏并发3正式任务成功，配置/boot/APP进程不变");
        }
        report["passed"]=json!(true);Ok(())
    }.await;
    if let Err(error) = &outcome {
        report["error"] = json!(error.to_string());
    }
    let saved = save(&directory, &report);
    support::close(state).await;
    saved?;
    outcome
}
