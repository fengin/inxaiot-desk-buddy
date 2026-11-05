//! 首屏正式读取API实机验收：只读，单ID请求按前端并发1/2/3；独立ignored门禁。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::{model::ScreenFields, rules::{normalize_mac, valid_mac}, settings::ScreenSettingsRead},
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{device::{AdbDevice, AndroidTools}, screen_settings, sleep_policy},
        stage75_adapter::Stage75Adapter,
    },
};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::{BTreeMap, BTreeSet}, future::Future, path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicUsize, Ordering}}, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const GUARDS: &[(&str, &str, &str, &str)] = &[
    (
        "192.168.3.42",
        "10",
        "9c000c3c3643071205d",
        "62:91:4c:93:81:57",
    ),
    ("192.168.3.74", "4", "A47A4AK02040", "28:2d:06:15:59:ac"),
    ("192.168.3.82", "4", "A47A5CK00524", "40:fd:f3:01:ae:2c"),
];
const CODES: [&str; 4] = ["adb", "ntp", "time", "sleepOff"];
const KEYS: [(&str, &str); 8] = [
    ("global", "ntp_server"), ("global", "auto_time"), ("global", "auto_time_zone"),
    ("global", "stay_on_while_plugged_in"), ("system", "screen_off_timeout"),
    ("secure", "sleep_timeout"), ("secure", "screensaver_enabled"),
    ("secure", "screensaver_activate_on_sleep"),
];
fn require(value: bool, message: &str) -> TestResult<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
#[derive(Clone, Serialize, Deserialize)]
struct Target { ip: String, size: String, serial: String, wifi: String, wired: String }
fn targets_from_evidence(initial: &Value, final_a: &Value) -> TestResult<Vec<Target>> {
    require(initial["readOnly"] == true && initial["deviceWrites"] == 0, "初始身份不是只读基线")?;
    require(final_a["restored"] == true, "阶段A尚未全部恢复，未连接设备")?;
    let rows = initial["targets"].as_array().ok_or("初始三屏身份缺失")?;
    let restored = final_a["restoration"].as_array().ok_or("阶段A逐台恢复记录缺失")?;
    require(rows.len() == GUARDS.len() && restored.len() == GUARDS.len(), "必须恰为原三屏完整交接")?;
    let mut targets = Vec::new();
    for (ip, size, serial, wifi) in GUARDS {
        let rows = rows.iter().filter(|r| r["ip"] == *ip && r["size"] == *size).collect::<Vec<_>>();
        let end = restored.iter().filter(|r| r["ip"] == *ip).collect::<Vec<_>>();
        require(rows.len() == 1 && end.len() == 1, "固定目标缺失或重复")?;
        let row = rows[0]; let end = end[0];
        let get = |key: &str| -> TestResult<String> {
            require(row["commands"][key]["exit"] == 0, "初始身份命令未成功")?;
            Ok(row["commands"][key]["stdout"].as_str().ok_or("初始身份不完整")?.trim().into())
        };
        require(get("serial")? == *serial && normalize_mac(&get("wifiMac")?) == normalize_mac(wifi)
            && row["suppliedMac"].as_str().is_some_and(|m| normalize_mac(m) == normalize_mac(wifi)),
            "初始序列号/WiFi与固定三屏不一致")?;
        let wired = end["after"]["wiredMac"].as_str().filter(|m| valid_mac(m)).ok_or("阶段A当前有线身份缺失")?;
        require(end["restored"] == true && end["after"]["serial"] == *serial, "阶段A恢复身份不匹配")?;
        targets.push(Target { ip: (*ip).into(), size: (*size).into(), serial: (*serial).into(),
            wifi: normalize_mac(wifi), wired: normalize_mac(wired) });
    }
    Ok(targets)
}
fn evidence_path(root: &Path, env: &str, expected_name: &str) -> TestResult<PathBuf> {
    let path = PathBuf::from(std::env::var_os(env).ok_or("缺少明确阶段A证据路径")?).canonicalize()?;
    require(path.starts_with(root.join(".review-tools/combined-real-20261104").canonicalize()?)
        && path.file_name().is_some_and(|n| n == expected_name), "身份/恢复证据离开本轮范围")?;
    Ok(path)
}
fn audited_tools(root: &Path) -> TestResult<AndroidTools> {
    let expected = root.join(".review-tools/combined-real-20261104/adb-audit.exe").canonicalize()?;
    let explicit = PathBuf::from(std::env::var_os("INX_ADB_PATH").ok_or("须明确设置本轮透明审计工具")?).canonicalize()?;
    let tools = AndroidTools::discover()?;
    require(explicit == expected && tools.adb.canonicalize()? == expected, "实际工具不是已验证透明代理")?;
    Ok(tools)
}
fn audit_directory(root: &Path) -> TestResult<PathBuf> {
    let path = PathBuf::from(std::env::var_os("INX_COMBINED_ADB_AUDIT").ok_or("须使用全新只读审计目录")?);
    require(path.is_absolute() && path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("audit-settings-read-")),
        "读取审计目录须有专用前缀")?;
    let parent = path.parent().ok_or("审计父目录缺失")?.canonicalize()?;
    require(parent == root.join(".review-tools/combined-real-20261104").canonicalize()?, "读取审计目录不属于本轮")?;
    std::fs::create_dir_all(&path)?;
    require(std::fs::read_dir(&path)?.next().is_none(), "审计目录已使用，不把旧记录算本轮只读证据")?;
    Ok(path.canonicalize()?)
}
fn side_effect(args: &[&str]) -> bool {
    if args.len() < 2 { return true; }
    let mut command = &args[2..];
    if command.first() == Some(&"shell") { command = &command[1..]; }
    if command.starts_with(&["su", "0"]) { command = &command[2..]; }
    if matches!(command.first(), Some(&"reboot" | &"setprop" | &"input" | &"am" | &"monkey" | &"svc" | &"content" | &"cmd")) {
        return true;
    }
    if command.starts_with(&["settings", "put"]) || command.starts_with(&["settings", "delete"])
        || command.first() == Some(&"date") && command.iter().any(|a| a.starts_with('@'))
        || command.first() == Some(&"hwclock") && command.contains(&"-w")
        || command.first() == Some(&"logcat") && command.contains(&"-c")
        || command.first() == Some(&"pm") && !matches!(command.get(1), Some(&"path" | &"list" | &"dump")) {
        return true;
    }
    if command.starts_with(&["sh", "-c"]) {
        return !command.get(2).is_some_and(|s| regex::Regex::new(r"^test -c /dev/rtc\d+$").unwrap().is_match(s.trim_matches('\'')));
    }
    false
}
fn audit_zero_writes(path: &Path) -> TestResult<usize> {
    let mut count = 0;
    for entry in std::fs::read_dir(path)? {
        let value: Value = serde_json::from_slice(&std::fs::read(entry?.path())?)?;
        let args = value["args"].as_array().ok_or("审计参数缺失")?.iter()
            .map(|v| v.as_str().ok_or("审计参数不是文本")).collect::<Result<Vec<_>, _>>()?;
        require(!side_effect(&args), "只读API出现设备写入/启动/重放，保留原审计，不宣称通过")?;
        count += 1;
    }
    require(count > 0, "审计为空，不能证明真实读取或零写入")?;
    Ok(count)
}
async fn read(device: &AdbDevice, ip: &str, args: &[&str]) -> TestResult<String> {
    let value = device.shell(ip, args, CancellationToken::new()).await?;
    Ok(value.trim().into())
}
async fn identity(device: &AdbDevice, target: &Target) -> TestResult<()> {
    device.connect(&target.ip, CancellationToken::new()).await?;
    require(read(device, &target.ip, &["getprop", "ro.serialno"]).await? == target.serial, "物理序列号不同")?;
    require(normalize_mac(&read(device, &target.ip, &["cat", "/sys/class/net/wlan0/address"]).await?) == target.wifi,
        "用户WiFi身份不同")?;
    require(normalize_mac(&read(device, &target.ip, &["cat", "/sys/class/net/eth0/address"]).await?) == target.wired,
        "当前有线MAC与阶段A交接不同")?;
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    ip: String, boot_id: String, settings: BTreeMap<String, String>,
    persistent_adb: String, service_adb: String, time_zone: String,
    wakefulness: String, display: String,
}
impl Snapshot {
    fn critical_equal(&self, other: &Self) -> bool {
        self.ip == other.ip && self.boot_id == other.boot_id && self.settings == other.settings
            && self.persistent_adb == other.persistent_adb && self.service_adb == other.service_adb && self.time_zone == other.time_zone
    }
}
async fn snapshot(device: &AdbDevice, target: &Target) -> TestResult<Snapshot> {
    identity(device, target).await?;
    let mut settings = BTreeMap::new();
    for (ns, key) in KEYS { settings.insert(format!("{ns}.{key}"), read(device, &target.ip, &["settings", "get", ns, key]).await?); }
    let boot_id = read(device, &target.ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?;
    uuid::Uuid::parse_str(&boot_id)?;
    let power = sleep_policy::read(device, &target.ip, CancellationToken::new()).await?;
    Ok(Snapshot { ip: target.ip.clone(), boot_id, settings,
        persistent_adb: read(device, &target.ip, &["getprop", "persist.adb.tcp.port"]).await?,
        service_adb: read(device, &target.ip, &["getprop", "service.adb.tcp.port"]).await?,
        time_zone: read(device, &target.ip, &["getprop", "persist.sys.timezone"]).await?,
        wakefulness: power.wakefulness, display: power.display })
}
#[derive(Default)]
struct Calls { current: AtomicUsize, peak: AtomicUsize, total: AtomicUsize }
struct Active(Arc<Calls>);
impl Drop for Active { fn drop(&mut self) { self.0.current.fetch_sub(1, Ordering::SeqCst); } }
impl Calls {
    fn enter(self: &Arc<Self>) -> Active {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst); self.total.fetch_add(1, Ordering::SeqCst);
        Active(self.clone())
    }
}
fn validate_row(id: &str, rows: &[ScreenSettingsRead]) -> TestResult<()> {
    require(rows.len() == 1 && rows[0].screen_id == id && rows[0].error.is_none(), "单ID正式读取响应不完整或目标错配")?;
    let row = &rows[0];
    require(row.items.keys().map(String::as_str).collect::<BTreeSet<_>>() == CODES.into_iter().collect(), "未返回恰好四项状态")?;
    require(row.items["ntp"].state == "not_selected", "首屏默认NTP意外被选中")?;
    require(row.items.values().all(|v| ["ready", "skip", "blocked", "unknown", "not_selected"].contains(&v.state.as_str())),
        "读取返回未定义状态")?;
    require(row.pending_count == row.items.values().filter(|v| v.state == "ready").count()
        && row.blocked_count == row.items.values().filter(|v| ["blocked", "unknown"].contains(&v.state.as_str())).count(),
        "状态与汇总计数不一致")?;
    Ok(())
}
async fn batch<F, Fut>(ids: &[String], concurrency: usize, read_one: F) -> TestResult<Value>
where F: Fn(String) -> Fut, Fut: Future<Output = TestResult<Vec<ScreenSettingsRead>>> {
    require((1..=3).contains(&concurrency), "读取并发须为1/2/3")?;
    let counts = Arc::new(Calls::default());
    let work = |id: String| {
        let counts = counts.clone(); let read_one = &read_one;
        async move {
            let _active = counts.enter();
            let rows = read_one(id.clone()).await?; validate_row(&id, &rows)?;
            let row = &rows[0];
            Ok::<_, Box<dyn std::error::Error>>((id, json!({"screenId":row.screen_id,"readAt":row.read_at,
                "items":row.items.iter().map(|(key,value)|(key.clone(),value.state.clone())).collect::<BTreeMap<_,_>>(),
                "pendingCount":row.pending_count,"blockedCount":row.blocked_count,"defaultNtpSelected":false})))
        }
    };
    let rows = stream::iter(ids.iter().cloned().map(work)).buffer_unordered(concurrency).try_collect::<Vec<_>>().await?;
    require(counts.peak.load(Ordering::SeqCst) == concurrency && counts.total.load(Ordering::SeqCst) == ids.len(),
        "正式单ID调用峰值或总数与前端批次不一致")?;
    let rows = rows.into_iter().collect::<BTreeMap<_,_>>();
    Ok(json!({"requestedConcurrency":concurrency,"actualPeakReadCalls":counts.peak.load(Ordering::SeqCst),
        "actualReadCalls":counts.total.load(Ordering::SeqCst),"requestShape":"每次仅一个screenId，无后端concurrency参数",
        "rows":ids.iter().map(|id|rows.get(id).cloned().ok_or("目标读取结果丢失")).collect::<Result<Vec<_>,_>>()?}))
}
async fn task_count(state: &FormalAppState, project: &str) -> TestResult<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM local_task WHERE local_project_id=?").bind(project)
        .fetch_one(state.local_store.pool()).await?)
}
fn save(path: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(path.join("result.json"), serde_json::to_vec_pretty(report)?)?; Ok(())
}
#[tokio::test]
#[ignore = "仅A全部结束并显式交回后，READ_REAL=1、READ_HANDOFF=PHASE_A_FINISHED及原/最终身份门禁；仅三批只读，绝不设置/重启"]
async fn screen_settings_read_single_id_real_batches() -> TestResult<()> {
    require(std::env::var("INX_SCREEN_SETTINGS_READ_REAL").as_deref() == Ok("1")
        && std::env::var("INX_SCREEN_SETTINGS_READ_HANDOFF").as_deref() == Ok("PHASE_A_FINISHED"),
        "首屏只读尚未获A交接，未连接设备")?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().canonicalize()?;
    let initial = evidence_path(&root, "INX_SCREEN_SETTINGS_READ_INITIAL_BASELINE", "baseline.json")?;
    let final_a = evidence_path(&root, "INX_SCREEN_SETTINGS_READ_PHASE_A_RESULT", "result.json")?;
    let targets = targets_from_evidence(&serde_json::from_slice(&std::fs::read(&initial)?)?,
        &serde_json::from_slice(&std::fs::read(&final_a)?)?)?;
    let tools = audited_tools(&root)?; let audit = audit_directory(&root)?;
    let tool_sha = format!("{:x}", Sha256::digest(std::fs::read(&tools.adb)?));
    let device = AdbDevice::new(tools);
    let directory = root.join(".review-tools/screen-regression-20261104").join(format!("settings-read-real-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&directory)?;
    let state = support::state_at(&directory, false).await;
    let mut report = json!({"passed":false,"readOnly":true,"deviceWrites":null,"api":"screen_settings::read",
        "handoff":"PHASE_A_FINISHED","initialSource":initial,"phaseAFinalSource":final_a,"auditDirectory":audit,
        "auditedToolSha256":tool_sha,"isolatedDataDirectory":true,"platformConnectionsRequested":false,
        "batches":[],"before":[],"after":[],"naturalPowerChangesAllowed":true});
    let outcome: TestResult<()> = async {
        let project = Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let before = futures_util::future::try_join_all(targets.iter().map(|t| snapshot(&device, t))).await?;
        report["before"] = serde_json::to_value(&before)?; save(&directory, &report)?;
        let mut ids = Vec::new();
        for t in &targets {
            ids.push(repo.save_local(&project, &ScreenFields { name:"首屏只读验收".into(), ip:t.ip.clone(),
                size:t.size.clone(), mac:t.wired.clone(), ..Default::default() }, None, None).await?);
        }
        let count = task_count(&state, &project).await?;
        for concurrency in [1, 2, 3] {
            let started = std::time::Instant::now();
            let result = batch(&ids, concurrency, |id| {
                let state = state.clone(); let project = project.clone();
                async move { Ok(screen_settings::read(&state, &project, &[id]).await?) }
            }).await?;
            let after_count = task_count(&state, &project).await?;
            require(after_count == count, "首屏读取意外创建任务")?;
            let audit_count = audit_zero_writes(&audit)?;
            report["batches"].as_array_mut().unwrap().push(json!({"result":result,"elapsedMs":started.elapsed().as_millis(),
                "tasksBefore":count,"tasksAfter":after_count,"auditedCommandsSoFar":audit_count}));
            save(&directory, &report)?;
        }
        let after = futures_util::future::try_join_all(targets.iter().map(|t| snapshot(&device, t))).await?;
        report["after"] = serde_json::to_value(&after)?;
        let unchanged = before.iter().zip(&after).all(|(before,after)|before.critical_equal(after));
        report["criticalSettingsAndBootUnchanged"] = json!(unchanged);
        report["deviceWrites"] = json!(0); report["auditCommandCount"] = json!(audit_zero_writes(&audit)?);
        require(unchanged && task_count(&state, &project).await? == count, "只读前后关键原值/boot或任务数不同")?;
        report["passed"] = json!(true); Ok(())
    }.await;
    if let Err(error) = &outcome { report["error"] = json!(error.to_string()); }
    let saved = save(&directory, &report);
    support::close(state).await;
    saved?; outcome?;
    println!("SCREEN_SETTINGS_READ_ONLY_PASS {}", directory.display());
    Ok(())
}

#[cfg(test)]
mod guards {
    use super::*;
    #[test]
    #[ignore = "仅离线读取已结束的三屏A证据并校验交接合同，不调用任何ADB或正式读取API"]
    fn completed_a_handoff_contract_offline() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let initial: Value = serde_json::from_slice(&std::fs::read(root.join(
            ".review-tools/combined-real-20261104/baseline-1793765480305/baseline.json")).unwrap()).unwrap();
        let final_a: Value = serde_json::from_slice(&std::fs::read(root.join(
            ".review-tools/combined-real-20261104/evidence/combined-ntp-01a1a5c4-92ae-7b03-b72d-202651175739/result.json")).unwrap()).unwrap();
        let targets = targets_from_evidence(&initial, &final_a).unwrap();
        assert_eq!(targets.len(), 3);
        for target in &targets {
            let restored = final_a["restoration"].as_array().unwrap().iter().find(|row| row["ip"] == target.ip).unwrap();
            assert_eq!(target.wired, normalize_mac(restored["after"]["wiredMac"].as_str().unwrap()));
        }
        println!("OFFLINE_HANDOFF_CONTRACT_OK targets=3 initial_serial_wifi_matched=true wired_from_finished_a_result=true");
    }
    fn row(id: &str) -> ScreenSettingsRead {
        serde_json::from_value(json!({"screenId":id,"readAt":"fixture",
            "items":CODES.into_iter().map(|key|(key,json!({"state":if key=="ntp"{"not_selected"}else{"skip"},
                "current":null,"target":null,"currentDisplay":"","targetDisplay":"","reason":"","rebootRequired":false}))).collect::<BTreeMap<_,_>>(),
            "pendingCount":0,"blockedCount":0})).unwrap()
    }
    #[tokio::test]
    async fn single_id_batches_measure_one_two_three_without_device_calls() {
        let ids = vec!["one".into(), "two".into(), "three".into()];
        for concurrency in [1, 2, 3] {
            let value = batch(&ids, concurrency, |id| async move {
                tokio::time::sleep(Duration::from_millis(10)).await; Ok(vec![row(&id)])
            }).await.unwrap();
            assert_eq!(value["actualPeakReadCalls"], concurrency);
            assert_eq!(value["actualReadCalls"], 3);
            assert_eq!(value["rows"][2]["screenId"], "three");
        }
    }
    #[test]
    fn responses_require_same_id_four_states_and_default_ntp_not_selected() {
        assert!(validate_row("one", &[row("one")]).is_ok());
        assert!(validate_row("one", &[row("other")]).is_err());
        let mut missing = row("one"); missing.items.remove("time");
        assert!(validate_row("one", &[missing]).is_err());
        let mut selected = row("one"); selected.items.get_mut("ntp").unwrap().state = "ready".into();
        assert!(validate_row("one", &[selected]).is_err());
    }
    #[test]
    fn exact_three_initial_identities_and_final_restored_wired_gate() {
        let initial = json!({"readOnly":true,"deviceWrites":0,"targets":GUARDS.iter().map(|(ip,size,serial,wifi)|
            json!({"ip":ip,"size":size,"suppliedMac":wifi,"commands":{"serial":{"exit":0,"stdout":serial},
                "wifiMac":{"exit":0,"stdout":wifi}}})).collect::<Vec<_>>()});
        let end = json!({"restored":true,"restoration":GUARDS.iter().enumerate().map(|(i,(ip,_,serial,_))|
            json!({"ip":ip,"restored":true,"after":{"serial":serial,"wiredMac":format!("02:33:44:55:66:{:02x}",i)}})).collect::<Vec<_>>()});
        assert_eq!(targets_from_evidence(&initial,&end).unwrap().len(),3);
        let mut bad=end.clone(); bad["restoration"][0]["after"]["serial"]=json!("other");
        assert!(targets_from_evidence(&initial,&bad).is_err());
        let mut missing=end.clone(); missing["restoration"].as_array_mut().unwrap().pop();
        assert!(targets_from_evidence(&initial,&missing).is_err());
        let mut unfinished=end; unfinished["restored"]=json!(false);
        assert!(targets_from_evidence(&initial,&unfinished).is_err());
    }
    #[test]
    fn audit_rejects_configuration_power_restart_and_provider_mutations() {
        for args in [vec!["-s","fixture:5555","reboot"],vec!["-s","fixture:5555","shell","am","start"],
            vec!["-s","fixture:5555","shell","settings","put","global","auto_time","1"],
            vec!["-s","fixture:5555","shell","content","call"],vec!["-s","fixture:5555","shell","input","keyevent","224"]] {
            assert!(side_effect(&args));
        }
        assert!(!side_effect(&["-s","fixture:5555","shell","settings","get","global","auto_time"]));
        assert!(!side_effect(&["-s","fixture:5555","shell","sh","-c","'test -c /dev/rtc0'"]));
    }
}
