#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::{ScreenAsset, ScreenFields},
            ntp::NtpPatch,
            operation::ScreenOperationInput,
            rules::{normalize_mac, valid_mac},
            settings::ScreenSettingsPatch,
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools},
            maintenance, screen_settings, sleep_policy, task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, time::Duration};
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

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Backup {
    ip: String,
    size: String,
    serial: String,
    wifi_mac: String,
    wired_mac: String,
    boot_id: String,
    rtc: String,
    root_mode: String,
    settings: BTreeMap<String, String>,
    persistent_adb: String,
    service_adb: String,
    time_zone: String,
    system_seconds: i64,
    rtc_seconds: i64,
    app_pids: String,
    wakefulness: String,
    display: String,
}
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct SegmentOptions {
    start_mask: u8,
    end_mask: u8,
    include_clear: bool,
    target_ips: Vec<String>,
}
fn segment_options(
    phase: &str,
    start: Option<&str>,
    end: Option<&str>,
    clear: Option<&str>,
    targets: Option<&str>,
) -> TestResult<SegmentOptions> {
    let (minimum, maximum) = match phase {
        "basic" => (1, 7),
        "ntp" => (8, 15),
        _ => return Err("必须明确basic或ntp阶段".into()),
    };
    let start_mask = start.map(str::parse::<u8>).transpose()?.unwrap_or(minimum);
    let end_mask = end.map(str::parse::<u8>).transpose()?.unwrap_or(maximum);
    require(
        start_mask >= minimum && end_mask <= maximum && start_mask <= end_mask,
        "分段mask越界或起点晚于终点",
    )?;
    let include_clear = match clear {
        None => phase == "ntp",
        Some("0") => false,
        Some("1") if phase == "ntp" => true,
        _ => return Err("INCLUDE_CLEAR仅NTP可显式1，否则用0".into()),
    };
    let target_ips = if let Some(value) = targets {
        let values = value.split(',').map(str::trim).collect::<Vec<_>>();
        require(
            !values.is_empty()
                && values
                    .iter()
                    .all(|v| !v.is_empty() && GUARDS.iter().any(|g| g.0 == *v))
                && values
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    == values.len(),
            "目标只能是不重复的原三屏白名单子集",
        )?;
        GUARDS
            .iter()
            .filter(|g| values.contains(&g.0))
            .map(|g| g.0.to_string())
            .collect()
    } else {
        GUARDS.iter().map(|g| g.0.to_string()).collect()
    };
    Ok(SegmentOptions {
        start_mask,
        end_mask,
        include_clear,
        target_ips,
    })
}
fn segment_from_env(phase: &str) -> TestResult<SegmentOptions> {
    let start = std::env::var("INX_SCREEN_SETTINGS_REAL_START_MASK").ok();
    let end = std::env::var("INX_SCREEN_SETTINGS_REAL_END_MASK").ok();
    let clear = std::env::var("INX_SCREEN_SETTINGS_REAL_INCLUDE_CLEAR").ok();
    let targets = std::env::var("INX_SCREEN_SETTINGS_REAL_TARGETS").ok();
    segment_options(
        phase,
        start.as_deref(),
        end.as_deref(),
        clear.as_deref(),
        targets.as_deref(),
    )
}
fn approved_verification_label(
    directory_name: &str,
    label: Option<&str>,
) -> TestResult<&'static str> {
    let expected = match directory_name {
        "combined-basic-01a1a4d1-2fc2-7200-b85d-c07f98baa500" => "execute-07-three",
        "combined-ntp-01a1a506-bd9e-7360-8b9c-ebe4ac9393ad" => "execute-10-three",
        _ => return Err("只允许原basic07或NTP10两个精确证据目录".into()),
    };
    require(
        label.is_none_or(|label| label == expected),
        "只读核实标签与固定目录不匹配",
    )?;
    Ok(expected)
}
fn audit_has_device_side_effect(args: &[&str]) -> bool {
    if args.len() < 2 {
        return true;
    }
    let mut command = &args[2..];
    if command.first() == Some(&"shell") {
        command = &command[1..];
    }
    if command.starts_with(&["su", "0"]) {
        command = &command[2..];
    }
    if matches!(
        command.first(),
        Some(&"reboot" | &"setprop" | &"input" | &"am" | &"monkey" | &"svc" | &"content")
    ) {
        return true;
    }
    if command.starts_with(&["settings", "put"]) || command.starts_with(&["settings", "delete"]) {
        return true;
    }
    if command.first() == Some(&"date") && command.iter().any(|arg| arg.starts_with('@')) {
        return true;
    }
    if command.first() == Some(&"hwclock") && command.contains(&"-w") {
        return true;
    }
    if command.first() == Some(&"pm")
        && !matches!(command.get(1), Some(&"path" | &"list" | &"dump"))
    {
        return true;
    }
    if command.first() == Some(&"cmd") {
        return true;
    }
    if command.first() == Some(&"logcat") && command.contains(&"-c") {
        return true;
    }
    if command.starts_with(&["sh", "-c"]) {
        return !command.get(2).is_some_and(|script| {
            Regex::new(r"^test -c /dev/rtc\d+$")
                .unwrap()
                .is_match(script.trim_matches('\''))
        });
    }
    false
}
fn reboot_budget(phase: &str, segment: &SegmentOptions) -> BTreeMap<String, usize> {
    segment
        .target_ips
        .iter()
        .map(|ip| {
            let selected = (segment.start_mask..=segment.end_mask)
                .filter(|mask| phase == "ntp" || ip == "192.168.3.42" && mask & 1 != 0)
                .count();
            (
                ip.clone(),
                selected + if segment.include_clear { 2 } else { 0 },
            )
        })
        .collect()
}
fn baseline_matches(current: &Backup, formal: &Value, initial_power: &str) -> bool {
    let field = |prefix: &str| {
        initial_power
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix(prefix))
    };
    formal["ip"] == current.ip
        && formal["size"] == current.size
        && formal["serial"] == current.serial
        && formal["wifiMac"] == current.wifi_mac
        && formal["wiredMac"] == current.wired_mac
        && formal["settings"] == serde_json::to_value(&current.settings).unwrap_or(Value::Null)
        && formal["persistentAdb"] == current.persistent_adb
        && formal["serviceAdb"] == current.service_adb
        && formal["timeZone"] == current.time_zone
        && field("mWakefulness=") == Some(current.wakefulness.as_str())
        && field("Display Power: state=") == Some(current.display.as_str())
}
fn bind_initial_baseline(backups: &[Backup], report: &mut Value) -> TestResult<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".review-tools/combined-real-20261104");
    let sample = root.join("baseline-1793765480305/baseline.json");
    let formal_path =
        root.join("evidence/combined-basic-01a1a4d1-2fc2-7200-b85d-c07f98baa500/result.json");
    let sample_bytes = std::fs::read(&sample)?;
    let formal_bytes = std::fs::read(&formal_path)?;
    let initial: Value = serde_json::from_slice(&formal_bytes)?;
    let mut comparisons = Vec::new();
    for current in backups {
        let original = initial["actualTargets"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["ip"] == current.ip))
            .ok_or("最初基线缺少目标")?;
        let power = initial["cases"][0]["before"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["ip"] == current.ip))
            .and_then(|row| row["power"].as_str())
            .ok_or("最初睡醒基线缺失")?;
        let matched = baseline_matches(current, original, power);
        comparisons.push(json!({"ip":current.ip,"matchesInitial":matched}));
        require(
            matched,
            "新分段与最初身份/11键/端口/时区/睡醒不一致，未开始写入，不把临时值当恢复目标",
        )?;
    }
    report["initialBaselineReference"] = json!(sample);
    report["initialBaselineSha256"] = json!(format!("{:x}", Sha256::digest(sample_bytes)));
    report["formalInitialBaselineSource"] = json!(formal_path);
    report["formalInitialBaselineSha256"] = json!(format!("{:x}", Sha256::digest(formal_bytes)));
    report["initialBaselineComparison"] = json!(comparisons);
    report["baselineExcludedVolatileFields"] = json!([
        "历史时钟秒数",
        "旧%s读数",
        "已授权改变的bootId与APP进程编号"
    ]);
    Ok(())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClearOutcome {
    Succeeded,
    ExpectedUnsyncedDefault,
}
fn clear_outcome(target: &Value, mask: u8) -> TestResult<ClearOutcome> {
    let settings = &target["evidence"]["screenSettings"];
    let ntp = &settings["items"]["ntp"];
    require(
        ntp["evidence"]["targetServer"] == ""
            && ntp["evidence"]["saveConfirmed"] == true
            && ntp["evidence"]["activationConfirmed"] == true
            && settings["reboot"]["bootCompleted"] == true
            && settings["reboot"]["beforeBootId"]
                .as_str()
                .is_some_and(|before| {
                    settings["reboot"]["afterBootId"]
                        .as_str()
                        .is_some_and(|after| {
                            before != after
                                && uuid::Uuid::parse_str(before).is_ok()
                                && uuid::Uuid::parse_str(after).is_ok()
                        })
                }),
        "清空原NTP必须先确认保存及新启动生效",
    )?;
    let later_ok = |field: &str| {
        matches!(
            settings["items"][field]["state"].as_str(),
            Some("succeeded" | "skipped")
        )
    };
    if target["device"] == "succeeded"
        && ntp["state"] == "succeeded"
        && ntp["evidence"]["syncConfirmed"] == true
        && (mask & 2 == 0 || later_ok("time"))
        && (mask & 4 == 0 || later_ok("sleepOff"))
    {
        return Ok(ClearOutcome::Succeeded);
    }
    require(
        target["device"] == "failed"
            && ntp["state"] == "failed"
            && ntp["evidence"]["syncConfirmed"] == false
            && (mask & 2 == 0 || settings["items"]["time"]["state"] == "not_started")
            && (mask & 4 == 0 || settings["items"]["sleepOff"]["state"] == "not_started"),
        "只能接受默认源未同步且后续项目未执行；其他失败、未知或后续错误不能宽容通过",
    )?;
    Ok(ClearOutcome::ExpectedUnsyncedDefault)
}
fn require_audited_tool(actual: &Path, requested: Option<&Path>) -> TestResult<()> {
    let requested = requested.ok_or("必须显式指定透明审计ADB工具")?;
    require(
        actual.canonicalize()? == requested.canonicalize()?
            && requested.file_name().and_then(|value| value.to_str()) == Some("adb-audit.exe"),
        "实际工具未走本轮ADB透明审计",
    )
}
fn audited_tools() -> TestResult<AndroidTools> {
    let tools = AndroidTools::discover()?;
    let requested = std::env::var_os("INX_ADB_PATH").map(std::path::PathBuf::from);
    require_audited_tool(&tools.adb, requested.as_deref())?;
    audit_offset()?;
    Ok(tools)
}
fn audit_coverage_confirmed(had_unknown: bool, before: usize, after: usize) -> bool {
    after > 0 && after >= before && (!had_unknown || after > before)
}
fn require(value: bool, message: &str) -> TestResult<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn save(dir: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(dir.join("result.json"), serde_json::to_vec_pretty(report)?)?;
    Ok(())
}
fn store_case(dir: &Path, report: &mut Value, case: &Value) -> TestResult<()> {
    let cases = report["cases"].as_array_mut().ok_or("案例列表缺失")?;
    if let Some(previous) = cases
        .iter_mut()
        .find(|previous| previous["label"] == case["label"])
    {
        *previous = case.clone();
    } else {
        cases.push(case.clone());
    }
    save(dir, report)
}
async fn capture_task(
    state: &FormalAppState,
    project: &str,
    task: &str,
    dir: &Path,
    report: &mut Value,
    case: &mut Value,
) -> TestResult<()> {
    let record = state.task_repository.get(task).await?;
    case["taskState"] = json!(record.state.as_str());
    case["taskTerminalObserved"] =
        json!(record.state.is_terminal() || record.state == TaskState::FinalizingFailed);
    store_case(dir, report, case)?;
    match task_data::read_results(state.local_store.pool(), project, task).await {
        Ok(results) => {
            case["results"] = serde_json::to_value(results)?;
            case["resultsReadError"] = Value::Null;
        }
        Err(error) => {
            case["resultsReadError"] = json!(error.to_string());
            store_case(dir, report, case)?;
            return Err(error.into());
        }
    }
    case["taskSnapshotAt"] = json!(stamp());
    store_case(dir, report, case)
}
fn before_reboot_notice(
    dir: &Path,
    report: &mut Value,
    kind: &str,
    label: &str,
    ips: &[String],
) -> TestResult<()> {
    let notice = json!({"kind":kind,"case":label,"targets":ips,"maximumRebootsPerTarget":1,"announcedAt":stamp(),"networkHelp":"启动后如未自动联网，可由现场人员手动连接；记录人工协助，不声称自动恢复"});
    report["nextRebootNotice"] = notice.clone();
    save(dir, report)?;
    std::fs::write(
        dir.join("next-reboot.json"),
        serde_json::to_vec_pretty(&notice)?,
    )?;
    eprintln!(
        "[QA_REBOOT_PLAN] {kind} {label}：{}，每台至多一次；本地next-reboot.json已落盘",
        ips.join("、")
    );
    Ok(())
}
fn needs_boot_probe(target: &Value) -> bool {
    matches!(target["device"].as_str(), Some("pending" | "unknown"))
        && target["evidence"]["phase"] == "screen_settings_reboot_requested"
        && target["evidence"]["screenSettings"]["reboot"]["requested"] == true
        && target["evidence"]["screenSettings"]["reboot"]["bootCompleted"] != true
        && target["evidence"]["screenSettings"]["reboot"]["afterBootId"].is_null()
        && target["evidence"]["screenSettings"]["stoppedAt"].is_null()
        && target["evidence"]["screenSettings"]["cancelled"] != true
}
fn get_state_allows_boot(success: bool, stdout: &str, stderr: &str) -> bool {
    success && stdout.trim() == "device" && stderr.trim().is_empty()
}
async fn boot_probe(device: &AdbDevice, ip: &str) -> Value {
    let mut result = json!({"ip":ip,"at":stamp(),"bootId":null});
    match device
        .adb(
            ip,
            &["get-state"],
            Duration::from_secs(2),
            CancellationToken::new(),
        )
        .await
    {
        Ok(value) => {
            let ready = get_state_allows_boot(value.success, &value.stdout, &value.stderr);
            result["getState"] =
                json!({"success":value.success,"stdout":value.stdout,"stderr":value.stderr});
            if ready {
                match device
                    .adb(
                        ip,
                        &["shell", "cat", "/proc/sys/kernel/random/boot_id"],
                        Duration::from_secs(2),
                        CancellationToken::new(),
                    )
                    .await
                {
                    Ok(value) => {
                        if value.success
                            && value.stderr.is_empty()
                            && uuid::Uuid::parse_str(value.stdout.trim()).is_ok()
                        {
                            result["bootId"] = json!(value.stdout.trim());
                        }
                        result["bootRead"] = json!({"success":value.success,"stderr":value.stderr});
                    }
                    Err(error) => result["bootRead"] = json!({"error":error.to_string()}),
                }
            }
        }
        Err(error) => result["getState"] = json!({"error":error.to_string()}),
    }
    result
}
fn stamp() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}
fn audit_offset() -> TestResult<usize> {
    let directory =
        std::env::var_os("INX_COMBINED_ADB_AUDIT").ok_or("本轮必须开启透明ADB命令审计")?;
    std::fs::create_dir_all(&directory)?;
    Ok(std::fs::read_dir(directory)?.count())
}
async fn command(device: &AdbDevice, ip: &str, args: &[&str]) -> TestResult<String> {
    let output = device
        .adb(ip, args, Duration::from_secs(20), CancellationToken::new())
        .await?;
    require(
        output.success && output.stderr.trim().is_empty(),
        &format!("设备命令未完成：{ip} {args:?} {}", output.stderr),
    )?;
    Ok(output.stdout.trim().into())
}
async fn read(device: &AdbDevice, ip: &str, args: &[&str]) -> TestResult<String> {
    let mut all = vec!["shell"];
    all.extend_from_slice(args);
    command(device, ip, &all).await
}
async fn root_read(device: &AdbDevice, before: &Backup, args: &[&str]) -> TestResult<String> {
    let mut all = vec!["shell"];
    if before.root_mode == "su0" {
        all.extend(["su", "0"]);
    }
    all.extend_from_slice(args);
    command(device, &before.ip, &all).await
}
async fn identity(device: &AdbDevice, before: &Backup) -> TestResult<()> {
    require(
        read(device, &before.ip, &["getprop", "ro.serialno"]).await? == before.serial,
        "序列号变化，禁止写入",
    )?;
    require(
        valid_mac(&read(device, &before.ip, &["cat", "/sys/class/net/eth0/address"]).await?),
        "有线MAC无法可靠读取，禁止QA准备或恢复",
    )?;
    require(
        normalize_mac(&read(device, &before.ip, &["cat", "/sys/class/net/wlan0/address"]).await?)
            == before.wifi_mac,
        "WiFi身份变化，禁止写入",
    )
}
async fn strict_identity(device: &AdbDevice, before: &Backup) -> TestResult<()> {
    identity(device, before).await?;
    require(
        normalize_mac(&read(device, &before.ip, &["cat", "/sys/class/net/eth0/address"]).await?)
            == before.wired_mac,
        "正常测试期间有线身份变化，停止后续测试；仅允许双重身份确认后收尾恢复",
    )
}
async fn settings(device: &AdbDevice, ip: &str) -> TestResult<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for (ns, key) in KEYS {
        values.insert(
            format!("{ns}.{key}"),
            read(device, ip, &["settings", "get", ns, key]).await?,
        );
    }
    Ok(values)
}
async fn system_sample(device: &AdbDevice, ip: &str) -> TestResult<(i64, i64)> {
    // 与正式clock::system_seconds一致；已测4寸固件date -u +%s有8小时时区偏差。
    let calendar = read(device, ip, &["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"]).await?;
    let checked_at = stamp();
    Ok((
        time::OffsetDateTime::parse(
            calendar.trim(),
            &time::format_description::well_known::Rfc3339,
        )?
        .unix_timestamp(),
        checked_at,
    ))
}
async fn observe(device: &AdbDevice, before: &Backup) -> TestResult<Value> {
    observe_inner(device, before, false).await
}
async fn observe_inner(device: &AdbDevice, before: &Backup, restoring: bool) -> TestResult<Value> {
    if restoring {
        identity(device, before).await?;
    } else {
        strict_identity(device, before).await?;
    }
    let rtc = read(
        device,
        &before.ip,
        &["cat", &format!("/sys/class/rtc/{}/since_epoch", before.rtc)],
    )
    .await?
    .parse::<i64>()?;
    let rtc_checked_at = stamp();
    let (system, system_checked_at) = system_sample(device, &before.ip).await?;
    let wired_mac =
        normalize_mac(&read(device, &before.ip, &["cat", "/sys/class/net/eth0/address"]).await?);
    Ok(
        json!({"ip":before.ip,"serial":before.serial,"wiredMac":wired_mac,"baselineWiredMac":before.wired_mac,"wiredMacUnchanged":wired_mac==before.wired_mac,
        "bootId":read(device,&before.ip,&["cat","/proc/sys/kernel/random/boot_id"]).await?,
        "bootCompleted":read(device,&before.ip,&["getprop","sys.boot_completed"]).await?,
        "settings":settings(device,&before.ip).await?,"persistentAdb":read(device,&before.ip,&["getprop","persist.adb.tcp.port"]).await?,
        "serviceAdb":read(device,&before.ip,&["getprop","service.adb.tcp.port"]).await?,
        "timeZone":read(device,&before.ip,&["getprop","persist.sys.timezone"]).await?,
        "systemSeconds":system,"rtcSeconds":rtc,"systemCheckedAt":system_checked_at,"rtcCheckedAt":rtc_checked_at,
        "systemOffsetSeconds":system-system_checked_at,"rtcOffsetSeconds":rtc-rtc_checked_at,
        "power":read(device,&before.ip,&["dumpsys","power"]).await?,
        "ntpStatus":read(device,&before.ip,&["dumpsys","network_time_update_service"]).await?,
        "ntpEvents":read(device,&before.ip,&["logcat","-b","events","-d","-s","ntp_success:I","ntp_failure:I","*:S"]).await?}),
    )
}
async fn write(
    device: &AdbDevice,
    before: &Backup,
    args: &[&str],
    dir: &Path,
    report: &mut Value,
    reason: &str,
) -> TestResult<()> {
    if report["restoring"] == true {
        identity(device, before).await?;
    } else {
        strict_identity(device, before).await?;
    }
    let entry = json!({"ip":before.ip,"serial":before.serial,"command":args,"reason":reason,"requestedAt":stamp()});
    report["qaWrites"]
        .as_array_mut()
        .ok_or("缺少写入证据列表")?
        .push(entry);
    save(dir, report)?; // 任何准备及收尾写入都先落盘。
    let response = root_read(device, before, args).await;
    let last = report["qaWrites"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    last["response"] = json!(response.as_ref().ok());
    last["error"] = json!(response.as_ref().err().map(ToString::to_string));
    save(dir, report)?;
    response?;
    Ok(())
}
async fn put(
    device: &AdbDevice,
    before: &Backup,
    ns: &str,
    key: &str,
    value: &str,
    dir: &Path,
    report: &mut Value,
    reason: &str,
) -> TestResult<()> {
    let args = if value == "null" {
        vec!["settings", "delete", ns, key]
    } else {
        vec!["settings", "put", ns, key, value]
    };
    write(device, before, &args, dir, report, reason).await?;
    require(
        read(device, &before.ip, &["settings", "get", ns, key]).await? == value,
        "准备或恢复设置回读不同",
    )
}
async fn backup(device: &AdbDevice, target_ips: &[String]) -> TestResult<Vec<Backup>> {
    let descriptions = support::live_screens()?;
    let mut result = Vec::new();
    for (ip, size, serial, wifi) in GUARDS
        .iter()
        .filter(|g| target_ips.iter().any(|ip| ip.as_str() == g.0))
    {
        require(
            descriptions
                .iter()
                .filter(|s| {
                    s.ip == *ip && s.size == *size && normalize_mac(&s.mac) == normalize_mac(wifi)
                })
                .count()
                == 1,
            "测试说明与本次三屏白名单不符",
        )?;
        device.connect(ip, CancellationToken::new()).await?;
        require(
            read(device, ip, &["getprop", "ro.serialno"]).await? == *serial,
            "本次序列号不符",
        )?;
        require(
            normalize_mac(&read(device, ip, &["cat", "/sys/class/net/wlan0/address"]).await?)
                == normalize_mac(wifi),
            "测试说明的WiFiMAC不符",
        )?;
        let wired = read(device, ip, &["cat", "/sys/class/net/eth0/address"]).await?;
        require(valid_mac(&wired), "没有可靠有线身份")?;
        let uid = read(device, ip, &["id"]).await?;
        let root_mode = if uid.contains("uid=0(") {
            "shell"
        } else {
            require(
                read(device, ip, &["su", "0", "id"])
                    .await?
                    .contains("uid=0("),
                "现有会话没有已验证管理员权限",
            )?;
            "su0"
        };
        let mut candidates = Vec::new();
        for rtc in read(device, ip, &["ls", "/sys/class/rtc"])
            .await?
            .split_whitespace()
        {
            if read(device, ip, &["cat", &format!("/sys/class/rtc/{rtc}/name")]).await?
                == if *size == "4" {
                    "rk808-rtc"
                } else {
                    "rtc-pcf85063"
                }
            {
                candidates.push(rtc.to_string());
            }
        }
        require(candidates.len() == 1, "没有唯一验证RTC")?;
        let rtc = candidates.remove(0);
        require(
            read(
                device,
                ip,
                &["cat", &format!("/sys/class/rtc/{rtc}/hctosys")],
            )
            .await?
                == "1",
            "所选RTC不是已确认启动时钟",
        )?;
        let power = sleep_policy::read(device, ip, CancellationToken::new()).await?;
        result.push(Backup {
            ip: (*ip).into(),
            size: (*size).into(),
            serial: (*serial).into(),
            wifi_mac: normalize_mac(wifi),
            wired_mac: normalize_mac(&wired),
            boot_id: read(device, ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?,
            root_mode: root_mode.into(),
            settings: settings(device, ip).await?,
            persistent_adb: read(device, ip, &["getprop", "persist.adb.tcp.port"]).await?,
            service_adb: read(device, ip, &["getprop", "service.adb.tcp.port"]).await?,
            time_zone: read(device, ip, &["getprop", "persist.sys.timezone"]).await?,
            system_seconds: system_sample(device, ip).await?.0,
            rtc_seconds: read(
                device,
                ip,
                &["cat", &format!("/sys/class/rtc/{rtc}/since_epoch")],
            )
            .await?
            .parse()?,
            rtc,
            app_pids: read(device, ip, &["pidof", "chat.xiaoxin.app"]).await?,
            wakefulness: power.wakefulness,
            display: power.display,
        });
    }
    Ok(result)
}
fn patch(mask: u8, server: &str) -> ScreenSettingsPatch {
    ScreenSettingsPatch {
        adb: mask & 1 != 0,
        time: mask & 2 != 0,
        sleep_off: mask & 4 != 0,
        ntp: (mask & 8 != 0).then(|| NtpPatch {
            server: server.into(),
        }),
    }
}
fn input(screens: &[ScreenAsset], concurrency: u32) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "screen_settings".into(),
        target_ids: screens.iter().map(|s| s.id.clone()).collect(),
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: String::new(),
        reinstall: false,
        concurrency,
        expected_targets: BTreeMap::new(),
        retry_of_operation_id: None,
    }
}
async fn formal(
    state: &FormalAppState,
    device: &AdbDevice,
    project: &str,
    screens: &[ScreenAsset],
    backups: &[Backup],
    mask: u8,
    server: &str,
    concurrency: u32,
    label: &str,
    submit: bool,
    allow_ntp_failure: bool,
    dir: &Path,
    report: &mut Value,
) -> TestResult<Value> {
    let request = input(screens, concurrency);
    let patches = screens
        .iter()
        .map(|s| (s.id.clone(), patch(mask, server)))
        .collect();
    let mut case = json!({"label":label,"selectionMask":mask,"patch":patch(mask,server),"concurrency":concurrency,"before":[],"after":[],"afterErrors":[],"submitted":false});
    store_case(dir, report, &case)?;
    for s in screens {
        let b = backups
            .iter()
            .find(|b| b.ip == s.fields.ip)
            .ok_or("目标不在白名单")?;
        case["before"]
            .as_array_mut()
            .unwrap()
            .push(observe(device, b).await?);
        store_case(dir, report, &case)?;
    }
    let preview = screen_settings::preflight(state, project, request.clone(), patches).await?;
    case["preflight"] = serde_json::to_value(&preview)?;
    store_case(dir, report, &case)?;
    require(preview.items.len() == screens.len(), "正式预检目标数不一致")?;
    for (s, old) in screens.iter().zip(case["before"].as_array().unwrap()) {
        let b = backups.iter().find(|b| b.ip == s.fields.ip).unwrap();
        let current = observe(device, b).await?;
        for key in [
            "settings",
            "persistentAdb",
            "serviceAdb",
            "timeZone",
            "bootId",
        ] {
            require(current[key] == old[key], "正式预检发生设备配置或启动变化")?;
        }
    }
    case["preflightReadOnlyVerified"] = json!(true);
    let blocked = preview
        .items
        .iter()
        .filter(|r| r.state == "blocked")
        .collect::<Vec<_>>();
    require(
        blocked.is_empty(),
        &format!("正式预检被阻断：{}", serde_json::to_string(&blocked)?),
    )?;
    let ready = preview
        .items
        .iter()
        .filter(|r| r.state == "ready")
        .map(|r| r.screen_id.clone())
        .collect::<Vec<_>>();
    if ready.is_empty() {
        case["allSkipNoTask"] = json!(true);
        require(
            task_data::read_preflight_plan(state.local_store.pool(), project, &preview.id)
                .await
                .is_err(),
            "全skip仍存在可执行预览",
        )?;
    } else if submit {
        let reboot_ips = preview
            .items
            .iter()
            .filter(|item| {
                item.state == "ready"
                    && item.settings.as_ref().is_some_and(|settings| {
                        settings.items.values().any(|item| item.reboot_required)
                    })
            })
            .filter_map(|item| {
                screens
                    .iter()
                    .find(|screen| screen.id == item.screen_id)
                    .map(|screen| screen.fields.ip.clone())
            })
            .collect::<Vec<_>>();
        case["plannedRebootTargets"] = json!(reboot_ips);
        if !reboot_ips.is_empty() {
            before_reboot_notice(dir, report, "product", label, &reboot_ips)?;
        }
        case["productAuditStartOffset"] = json!(audit_offset()?);
        case["submitted"] = json!(true);
        store_case(dir, report, &case)?;
        let mut request = request;
        request.target_ids = ready;
        let task = maintenance::submit(state, project, &preview.id, request).await?;
        case["taskId"] = json!(task);
        store_case(dir, report, &case)?;
        let mut boot_history: BTreeMap<String, Vec<Value>> = case["before"]
            .as_array()
            .unwrap()
            .iter()
            .map(|before| {
                (
                    before["ip"].as_str().unwrap().to_string(),
                    vec![json!({"bootId":before["bootId"],"observedAt":stamp()})],
                )
            })
            .collect();
        let waves = screens.len().div_ceil(concurrency.max(1) as usize);
        let mut next_probe = tokio::time::Instant::now();
        case["bootProbeIntervalSeconds"] = json!(5);
        case["bootProbeHistory"] = json!([]);
        let waited = tokio::time::timeout(Duration::from_secs(120 + 360 * waves as u64), async {
            loop {
                let value = state.task_repository.get(&task).await?;
                if value.state.is_terminal() || value.state == TaskState::FinalizingFailed {
                    return Ok::<_, Box<dyn std::error::Error>>(value.state);
                }
                if tokio::time::Instant::now() >= next_probe {
                    capture_task(state, project, &task, dir, report, &mut case).await?;
                    let active = screens
                        .iter()
                        .filter(|screen| needs_boot_probe(&case["results"]["targets"][&screen.id]))
                        .collect::<Vec<_>>();
                    let samples = futures_util::future::join_all(
                        active
                            .iter()
                            .map(|screen| boot_probe(device, &screen.fields.ip)),
                    )
                    .await;
                    for sample in samples {
                        if let Some(boot) = sample["bootId"].as_str() {
                            let history = boot_history
                                .get_mut(sample["ip"].as_str().unwrap())
                                .unwrap();
                            if history.last().unwrap()["bootId"] != boot {
                                history.push(json!({"bootId":boot,"observedAt":stamp()}));
                            }
                        }
                        case["bootProbeHistory"]
                            .as_array_mut()
                            .unwrap()
                            .push(sample);
                    }
                    case["observedBootHistory"] = serde_json::to_value(&boot_history)?;
                    store_case(dir, report, &case)?;
                    next_probe = tokio::time::Instant::now() + Duration::from_secs(5);
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        })
        .await;
        let final_state = match waited {
            Ok(Ok(state)) => state,
            Ok(Err(error)) => {
                let _ = capture_task(state, project, &task, dir, report, &mut case).await;
                return Err(error);
            }
            Err(error) => {
                let _ = capture_task(state, project, &task, dir, report, &mut case).await;
                return Err(error.into());
            }
        };
        case["taskState"] = json!(final_state.as_str());
        case["productAuditEndOffset"] = json!(audit_offset()?);
        case["taskTerminalObserved"] = json!(true);
        store_case(dir, report, &case)?;
        let results = task_data::read_results(state.local_store.pool(), project, &task).await?;
        case["results"] = serde_json::to_value(&results)?;
        case["taskTerminalObserved"] = json!(true);
        case["observedBootHistory"] = serde_json::to_value(&boot_history)?;
        store_case(dir, report, &case)?;
        // 标记只能由restore中的严格当前原源只读证明解除；正式任务的save/activation不能代替fresh/PID证据。
        for s in screens {
            let b = backups.iter().find(|b| b.ip == s.fields.ip).unwrap();
            match observe(device, b).await {
                Ok(after) => {
                    let history = boot_history.get_mut(&b.ip).unwrap();
                    if history.last().unwrap()["bootId"] != after["bootId"] {
                        history.push(json!({"bootId":after["bootId"],"observedAt":stamp()}));
                    }
                    case["after"].as_array_mut().unwrap().push(after);
                }
                Err(error) => case["afterErrors"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"ip":b.ip,"error":error.to_string()})),
            }
            case["observedBootHistory"] = serde_json::to_value(&boot_history)?;
            store_case(dir, report, &case)?;
        }
        case["observedBootHistory"] = serde_json::to_value(&boot_history)?;
        store_case(dir, report, &case)?;
        require(
            case["afterErrors"].as_array().unwrap().is_empty(),
            "任务真实终态已保存，但部分设备最终只读快照未取得，停止新case并恢复",
        )?;
        require(
            boot_history.values().all(|samples| samples.len() <= 2),
            "同一正式任务观察到多次启动编号变化",
        )?;
        require(
            final_state == TaskState::Succeeded || allow_ntp_failure,
            &format!("正式队列未成功：{label} {}", case["results"]),
        )?;
        if allow_ntp_failure {
            require(
                results.targets.len() == screens.len(),
                "清除结果缺少目标，不把缺失当作环境未同步",
            )?;
            let mut limited = Vec::new();
            for screen in screens {
                let target = results
                    .targets
                    .get(&screen.id)
                    .ok_or("清除结果缺少所选屏")?;
                if clear_outcome(&serde_json::to_value(target)?, mask)?
                    == ClearOutcome::ExpectedUnsyncedDefault
                {
                    limited.push(screen.fields.ip.clone());
                }
            }
            let expected_state = if limited.is_empty() {
                TaskState::Succeeded
            } else if limited.len() == screens.len() {
                TaskState::Failed
            } else {
                TaskState::PartiallySucceeded
            };
            require(
                final_state == expected_state,
                "清除父任务状态与逐屏已确认事实不一致",
            )?;
            case["expectedFailure"] = json!(!limited.is_empty());
            case["defaultSourceUnsynchronizedTargets"] = json!(limited);
            case["allSelectedGoalsSucceeded"] = json!(limited.is_empty());
            if !limited.is_empty() {
                case["expectedFailureReason"] = json!(
                    "默认源未同步；只确认保存/新启动激活，后续时间和睡眠未执行，不计为授时全通过"
                );
                if !report["environmentLimitations"].is_array() {
                    report["environmentLimitations"] = json!([]);
                }
                report["environmentLimitations"].as_array_mut().unwrap().push(json!({"case":label,"targets":limited,"uncovered":"默认源实际授时及其后续设置"}));
            }
        }
    }
    store_case(dir, report, &case)?;
    eprintln!(
        "实机组合设置完成：{label}，mask={mask}，submitted={}",
        case["submitted"]
    );
    Ok(case)
}
async fn prepare(
    device: &AdbDevice,
    backups: &[Backup],
    screens: &[ScreenAsset],
    mask: u8,
    dir: &Path,
    report: &mut Value,
) -> TestResult<()> {
    for s in screens {
        let b = backups.iter().find(|b| b.ip == s.fields.ip).unwrap();
        if mask & 1 != 0 && b.size == "10" {
            write(
                device,
                b,
                &["setprop", "persist.adb.tcp.port", "0"],
                dir,
                report,
                "制造ADB持久端口差异，当前运行端口保持5555",
            )
            .await?;
        }
        if mask & 8 != 0 {
            report["ntpRestoreRebootRequired"][&b.ip] = json!(true);
            report["ntpRestoreReferenceBoot"][&b.ip] =
                json!(read(device, &b.ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?);
            save(dir, report)?;
            put(
                device,
                b,
                "global",
                "ntp_server",
                "null",
                dir,
                report,
                "制造自定义NTP地址差异",
            )
            .await?;
        }
        if mask & 2 != 0 {
            put(
                device,
                b,
                "global",
                "auto_time",
                "0",
                dir,
                report,
                "隔离自动校正，准备小幅45秒双时钟差异",
            )
            .await?;
            write(
                device,
                b,
                &["date", "-u", &format!("@{}", stamp() - 45)],
                dir,
                report,
                "只漂移45秒用于正式校时验收",
            )
            .await?;
            write(
                device,
                b,
                &["hwclock", "-w", "-u", "-f", &format!("/dev/{}", b.rtc)],
                dir,
                report,
                "所选已验证RTC同步45秒偏差",
            )
            .await?;
        }
        if mask & 4 != 0 {
            put(
                device,
                b,
                "global",
                "stay_on_while_plugged_in",
                "0",
                dir,
                report,
                "制造睡眠策略未关闭差异",
            )
            .await?;
        }
    }
    Ok(())
}
fn original_source_confirmed(
    reference_boot: &str,
    boot_before: &str,
    boot_after: &str,
    completed: &str,
    original: &str,
    current: &Value,
    latest: &Value,
) -> bool {
    !reference_boot.is_empty()
        && uuid::Uuid::parse_str(reference_boot).is_ok()
        && uuid::Uuid::parse_str(boot_after).is_ok()
        && boot_before == boot_after
        && boot_after != reference_boot
        && completed == "1"
        && current["bootId"] == boot_after
        && current["server"] == original
        && current["autoTime"] == true
        && current["activationConfirmed"] == true
        && current["syncConfirmed"] == true
        && current["status"]["systemServerPid"]
            .as_u64()
            .is_some_and(|pid| {
                pid > 0 && current["sourceEvent"]["systemServerPid"].as_u64() == Some(pid)
            })
        && current["sourceEvent"]["address"].as_str() == Some(format!("/{original}").as_str())
        && latest["systemServerPid"] == current["status"]["systemServerPid"]
        && latest["address"].as_str() == Some(format!("/{original}").as_str())
}
fn latest_ntp_activity(log: &str, pid: u64) -> Option<Value> {
    let expression =
        Regex::new(r"^\s*[IWE]/ntp_(success|failure)\s*\(\s*(\d+)\s*\):\s*\[([^,\]]+)[,\]]")
            .ok()?;
    log.lines().filter_map(|line|{
        let capture=expression.captures(line.trim())?;
        (capture[2].parse::<u64>().ok()?==pid).then(||json!({"kind":capture[1].to_string(),"address":capture[3].trim(),"systemServerPid":pid}))
    }).last()
}
fn known_other_source(probe: &Value, original: &str) -> bool {
    probe["snapshotReliable"] == true
        && probe["latestActivity"]["address"]
            .as_str()
            .is_some_and(|address| {
                address
                    .rsplit_once('/')
                    .and_then(|(_, ip)| ip.parse::<std::net::IpAddr>().ok())
                    .is_some()
                    && address != format!("/{original}")
            })
}
#[derive(Debug, PartialEq, Eq)]
enum RestoreDecision {
    ConfirmedReadOnly,
    KnownReloadNeeded,
    Unconfirmed,
}
fn restore_decision(probe: &Value, recorded_other: bool, original: &str) -> RestoreDecision {
    if probe["snapshotReliable"] != true {
        return RestoreDecision::Unconfirmed;
    }
    if probe["confirmed"] == true {
        RestoreDecision::ConfirmedReadOnly
    } else if known_other_source(probe, original) || recorded_other {
        RestoreDecision::KnownReloadNeeded
    } else {
        RestoreDecision::Unconfirmed
    }
}
fn recorded_other_activation(report: &Value, ip: &str, current_boot: &str, original: &str) -> bool {
    let Some(id) = report["assetIds"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["ip"] == ip))
        .and_then(|row| row["id"].as_str())
    else {
        return false;
    };
    report["cases"].as_array().is_some_and(|cases| {
        cases.iter().any(|case| {
            let evidence = &case["results"]["targets"][id]["evidence"]["screenSettings"];
            let ntp = &evidence["items"]["ntp"]["evidence"];
            evidence["reboot"]["afterBootId"] == current_boot
                && evidence["reboot"]["bootCompleted"] == true
                && evidence["reboot"]["requested"] == true
                && uuid::Uuid::parse_str(current_boot).is_ok()
                && evidence["reboot"]["beforeBootId"]
                    .as_str()
                    .is_some_and(|before| {
                        before != current_boot && uuid::Uuid::parse_str(before).is_ok()
                    })
                && ntp["saveConfirmed"] == true
                && ntp["activationConfirmed"] == true
                && ntp["targetServer"]
                    .as_str()
                    .is_some_and(|server| server != original)
        })
    })
}
async fn original_source_probe(
    state: &FormalAppState,
    device: &AdbDevice,
    before: &Backup,
    reference_boot: &str,
    report: &Value,
) -> TestResult<Value> {
    identity(device, before).await?;
    let project = report["projectId"].as_str().ok_or("恢复源证据缺项目")?;
    let id = report["assetIds"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["ip"] == before.ip))
        .and_then(|row| row["id"].as_str())
        .ok_or("恢复源证据缺资产")?;
    let mut asset = ScreenRepository::new(state.local_store.pool().clone())
        .asset(project, id)
        .await?;
    // 仅QA只读临时副本使用当前有线MAC；不修改数据库或放宽产品写入身份门禁。
    asset.fields.mac =
        normalize_mac(&read(device, &before.ip, &["cat", "/sys/class/net/eth0/address"]).await?);
    let boot_before = read(
        device,
        &before.ip,
        &["cat", "/proc/sys/kernel/random/boot_id"],
    )
    .await?;
    let observed = device
        .inspect(&asset, "inspect", CancellationToken::new())
        .await?;
    let row =
        screen_settings::inspect(state, project, device, &asset, &observed, &patch(0, "")).await?;
    let current = row.items.get("ntp").ok_or("只读NTP项缺失")?.current.clone();
    let pid = current["status"]["systemServerPid"].as_u64();
    let activity = read(
        device,
        &before.ip,
        &[
            "logcat",
            "-b",
            "events",
            "-d",
            "-v",
            "brief",
            "-s",
            "ntp_success:I",
            "ntp_failure:I",
            "*:S",
        ],
    )
    .await?;
    let latest = pid
        .and_then(|pid| latest_ntp_activity(&activity, pid))
        .unwrap_or(Value::Null);
    let completed = read(device, &before.ip, &["getprop", "sys.boot_completed"]).await?;
    let current_pid = read(device, &before.ip, &["pidof", "system_server"])
        .await?
        .parse::<u64>()
        .ok();
    let boot_after = read(
        device,
        &before.ip,
        &["cat", "/proc/sys/kernel/random/boot_id"],
    )
    .await?;
    let original = before
        .settings
        .get("global.ntp_server")
        .ok_or("原NTP地址缺失")?;
    let reliable = boot_before == boot_after
        && current["bootId"] == boot_after
        && completed == "1"
        && pid.is_some_and(|pid| pid > 0 && current_pid == Some(pid));
    Ok(
        json!({"readOnly":true,"checkedAt":stamp(),"referenceBootId":reference_boot,"bootBefore":boot_before,"bootId":boot_after,"bootCompleted":completed,"current":current,"latestActivity":latest,"snapshotReliable":reliable,
        "currentSystemServerPid":current_pid,"confirmed":reliable&&original_source_confirmed(reference_boot,&boot_before,&boot_after,&completed,original,&current,&latest)}),
    )
}
async fn restore(
    state: &FormalAppState,
    device: &AdbDevice,
    before: &Backup,
    dir: &Path,
    report: &mut Value,
) -> TestResult<Value> {
    let current_ntp = read(
        device,
        &before.ip,
        &["settings", "get", "global", "ntp_server"],
    )
    .await
    .ok();
    let mut must_activate_original_ntp = report["ntpRestoreRebootRequired"][&before.ip] == true
        || current_ntp
            .as_ref()
            .is_some_and(|v| Some(v) != before.settings.get("global.ntp_server"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    loop {
        let _ = device.connect(&before.ip, CancellationToken::new()).await;
        if identity(device, before).await.is_ok()
            && read(device, &before.ip, &["getprop", "sys.boot_completed"])
                .await
                .ok()
                .as_deref()
                == Some("1")
        {
            break;
        }
        require(
            tokio::time::Instant::now() < deadline,
            "设备重连超时，原配置尚未恢复",
        )?;
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
    // 对已经回到原保存值的晚恢复设备，先复用正式只读判定，避免marker本身引发额外重启。
    if must_activate_original_ntp {
        let original = before
            .settings
            .get("global.ntp_server")
            .ok_or("原NTP地址缺失")?;
        let saved = read(
            device,
            &before.ip,
            &["settings", "get", "global", "ntp_server"],
        )
        .await?;
        let auto = read(
            device,
            &before.ip,
            &["settings", "get", "global", "auto_time"],
        )
        .await?;
        if saved == *original && auto == "1" {
            let reference = report["ntpRestoreReferenceBoot"][&before.ip]
                .as_str()
                .unwrap_or(&before.boot_id)
                .to_string();
            match original_source_probe(state, device, before, &reference, report).await {
                Ok(probe) => {
                    report["ntpRestoreBeforeAnyWriteProof"][&before.ip] = probe.clone();
                    if restore_decision(&probe, false, original)
                        == RestoreDecision::ConfirmedReadOnly
                    {
                        must_activate_original_ntp = false;
                        report["ntpRestoreRebootRequired"][&before.ip] = json!(false);
                        report["ntpRestorationDecision"][&before.ip] =
                            json!("写入收尾前已只读确认晚恢复新启动的原源；不追加QA重启");
                    }
                    save(dir, report)?;
                }
                Err(error) => {
                    report["ntpRestoreBeforeAnyWriteReadError"][&before.ip] =
                        json!(error.to_string());
                    save(dir, report)?;
                }
            }
        }
    }
    // 时钟恢复使用准确当前UTC及唯一启动RTC，不能把历史快照的秒数回写。
    put(
        device,
        before,
        "global",
        "auto_time",
        "0",
        dir,
        report,
        "恢复准确当前时钟前暂时停止自动校时",
    )
    .await?;
    write(
        device,
        before,
        &["date", "-u", &format!("@{}", stamp())],
        dir,
        report,
        "QA收尾恢复准确当前电脑UTC",
    )
    .await?;
    write(
        device,
        before,
        &["hwclock", "-w", "-u", "-f", &format!("/dev/{}", before.rtc)],
        dir,
        report,
        "QA收尾恢复已验证启动RTC准确当前UTC",
    )
    .await?;
    for (ns, key) in KEYS {
        let wanted = before
            .settings
            .get(&format!("{ns}.{key}"))
            .ok_or("原值备份缺失")?;
        let current = read(device, &before.ip, &["settings", "get", ns, key]).await?;
        if current != *wanted {
            if *key == "ntp_server" {
                must_activate_original_ntp = true;
            }
            require(
                ["ntp_server", "auto_time", "stay_on_while_plugged_in"].contains(key),
                "未修改的基线键出现外部变化，不覆盖",
            )?;
            put(
                device,
                before,
                ns,
                key,
                wanted,
                dir,
                report,
                "恢复本轮实际修改过的原系统设置",
            )
            .await?;
        }
    }
    if read(device, &before.ip, &["getprop", "persist.adb.tcp.port"]).await?
        != before.persistent_adb
    {
        write(
            device,
            before,
            &["setprop", "persist.adb.tcp.port", &before.persistent_adb],
            dir,
            report,
            "恢复本轮原ADB持久端口值",
        )
        .await?;
    }
    if must_activate_original_ntp {
        let reference = report["ntpRestoreReferenceBoot"][&before.ip]
            .as_str()
            .unwrap_or(&before.boot_id)
            .to_string();
        let original = before
            .settings
            .get("global.ntp_server")
            .ok_or("原NTP地址缺失")?
            .clone();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
        let restart_needed = loop {
            match original_source_probe(state, device, before, &reference, report).await {
                Ok(probe) => {
                    if !report["ntpRestoreReadOnlyProofs"][&before.ip].is_array() {
                        report["ntpRestoreReadOnlyProofs"][&before.ip] = json!([]);
                    }
                    report["ntpRestoreReadOnlyProofs"][&before.ip]
                        .as_array_mut()
                        .unwrap()
                        .push(probe.clone());
                    save(dir, report)?;
                    let decision = restore_decision(
                        &probe,
                        probe["bootId"].as_str().is_some_and(|boot| {
                            recorded_other_activation(report, &before.ip, boot, &original)
                        }),
                        &original,
                    );
                    if decision == RestoreDecision::ConfirmedReadOnly {
                        report["ntpRestoreRebootRequired"][&before.ip] = json!(false);
                        report["ntpRestorationDecision"][&before.ip] = json!(
                            "当前新启动/当前系统PID的原源新鲜成功证据已确认，仅只读收尾，不追加QA重启"
                        );
                        save(dir, report)?;
                        break false;
                    }
                    if decision == RestoreDecision::KnownReloadNeeded {
                        report["ntpRestorationDecision"][&before.ip] = json!(
                            "原值已保存，现有证据确认运行源仍是其它目标，需一次独立QA恢复重启"
                        );
                        save(dir, report)?;
                        break true;
                    }
                }
                Err(error) => {
                    report["ntpRestoreLastReadError"][&before.ip] =
                        json!({"at":stamp(),"error":error.to_string()});
                    save(dir, report)?;
                }
            }
            require(
                tokio::time::Instant::now() < deadline,
                "原源只读证据仍不完整，保留恢复责任，不用额外重启猜测恢复",
            )?;
            tokio::time::sleep(Duration::from_secs(5)).await;
        };
        if restart_needed {
            let old_boot = read(
                device,
                &before.ip,
                &["cat", "/proc/sys/kernel/random/boot_id"],
            )
            .await?;
            identity(device, before).await?;
            before_reboot_notice(
                dir,
                report,
                "qa_restore",
                "恢复已证实仍未生效的原NTP运行源",
                &[before.ip.clone()],
            )?;
            report["qaWrites"].as_array_mut().unwrap().push(json!({"ip":before.ip,"serial":before.serial,"command":["adb","reboot"],"reason":"恢复已保存的原NTP运行源，属于QA收尾而非产品重放","beforeBootId":old_boot,"requestedAt":stamp()}));
            save(dir, report)?;
            let response = device
                .adb(
                    &before.ip,
                    &["reboot"],
                    Duration::from_secs(20),
                    CancellationToken::new(),
                )
                .await;
            let last = report["qaWrites"]
                .as_array_mut()
                .unwrap()
                .last_mut()
                .unwrap();
            last["responseSuccess"] = json!(response.as_ref().ok().map(|v| v.success));
            last["responseError"] = json!(response.as_ref().err().map(ToString::to_string));
            save(dir, report)?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
            loop {
                let _ = device.connect(&before.ip, CancellationToken::new()).await;
                if identity(device, before).await.is_ok()
                    && read(device, &before.ip, &["getprop", "sys.boot_completed"])
                        .await
                        .ok()
                        .as_deref()
                        == Some("1")
                    && read(
                        device,
                        &before.ip,
                        &["cat", "/proc/sys/kernel/random/boot_id"],
                    )
                    .await
                    .ok()
                    .is_some_and(|v| v != old_boot)
                {
                    break;
                }
                require(
                    tokio::time::Instant::now() < deadline,
                    "QA恢复重启后未重新就绪，原NTP运行源责任仍未解除",
                )?;
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
            loop {
                let probe = original_source_probe(state, device, before, &old_boot, report).await?;
                if !report["ntpRestoreReadOnlyProofs"][&before.ip].is_array() {
                    report["ntpRestoreReadOnlyProofs"][&before.ip] = json!([]);
                }
                report["ntpRestoreReadOnlyProofs"][&before.ip]
                    .as_array_mut()
                    .unwrap()
                    .push(probe.clone());
                save(dir, report)?;
                if probe["confirmed"] == true {
                    break;
                }
                require(
                    tokio::time::Instant::now() < deadline,
                    "原NTP运行源新启动授时尚未确认，不将仅地址恢复记为完整恢复",
                )?;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            report["ntpRestoreRebootRequired"][&before.ip] = json!(false);
            save(dir, report)?;
        }
    }
    let power = sleep_policy::read(device, &before.ip, CancellationToken::new()).await?;
    if before.wakefulness == "Asleep" && power.wakefulness != "Asleep" {
        let timeout = before
            .settings
            .get("system.screen_off_timeout")
            .ok_or("原息屏时间缺失")?
            .parse::<u64>()?;
        require(timeout <= 120_000, "原为休眠但超时很长，禁止猜测或强制睡眠")?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout + 20_000);
        loop {
            let power = sleep_policy::read(device, &before.ip, CancellationToken::new()).await?;
            if power.wakefulness == "Asleep" && power.display == "OFF" {
                break;
            }
            require(
                tokio::time::Instant::now() < deadline,
                "恢复原接电/超时后自然休眠尚未恢复",
            )?;
            tokio::time::sleep(Duration::from_secs(3)).await;
        }
    } else if before.wakefulness == "Awake" && power.wakefulness != "Awake" {
        write(
            device,
            before,
            &["input", "keyevent", "224"],
            dir,
            report,
            "QA恢复原唤醒状态，仅一次非切换唤醒",
        )
        .await?;
    }
    let power = sleep_policy::read(device, &before.ip, CancellationToken::new()).await?;
    require(
        power.wakefulness == before.wakefulness && power.display == before.display,
        "睡眠或唤醒运行态未回到本轮基线",
    )?;
    require(
        !read(device, &before.ip, &["pidof", "chat.xiaoxin.app"])
            .await?
            .is_empty(),
        "原小新运行进程未恢复，不主动重启应用",
    )?;
    let mut after = observe_inner(device, before, true).await?;
    after["powerStateRestored"] = json!(true);
    after["appRunningConfirmed"] = json!(true);
    require(
        after["settings"] == serde_json::to_value(&before.settings)?,
        "系统配置未完全恢复基线",
    )?;
    require(
        after["persistentAdb"] == before.persistent_adb
            && after["serviceAdb"] == before.service_adb
            && after["timeZone"] == before.time_zone,
        "ADB或时区未恢复基线",
    )?;
    require(
        after["systemOffsetSeconds"]
            .as_i64()
            .unwrap_or(i64::MAX)
            .abs()
            <= 15
            && after["rtcOffsetSeconds"].as_i64().unwrap_or(i64::MAX).abs() <= 15,
        "收尾系统或RTC未恢复准确当前时间",
    )?;
    Ok(after)
}

#[tokio::test]
#[ignore = "需INX_SCREEN_SETTINGS_REAL=1且phase=basic或ntp；仅当前三屏白名单，隔离队列真实写入并恢复基线"]
async fn authorized_three_screen_formal_settings_matrix_and_restore() -> TestResult<()> {
    require(
        std::env::var("INX_SCREEN_SETTINGS_REAL").as_deref() == Ok("1"),
        "缺少实机显式门禁",
    )?;
    let phase = std::env::var("INX_SCREEN_SETTINGS_REAL_PHASE")?;
    require(
        ["basic", "ntp"].contains(&phase.as_str()),
        "必须明确basic或ntp阶段",
    )?;
    let segment = segment_from_env(&phase)?;
    if phase == "ntp" {
        require(
            std::env::var("INX_SCREEN_SETTINGS_NTP_CONFIRMED").as_deref() == Ok("192.168.3.142"),
            "须先确认原142授时源才允许NTP实机段",
        )?;
    }
    let directory = support::evidence_dir(&format!("combined-{phase}"))?;
    eprintln!("阶段A实机证据：{}", directory.display());
    let state = support::state_at(&directory, true).await;
    let tools = audited_tools()?;
    let device = AdbDevice::new(tools);
    let mut report = json!({"phase":phase,"segment":segment,"passed":false,"restored":false,"productionInstanceOpened":false,"platformConnectionsRequested":false,"applicationDataStore":"本轮全新SQLite与MemorySecretStore","qaWrites":[],"cases":[],"restoration":[],"ntpRestoreRebootRequired":{}});
    report["plannedMaximumProductReboots"] = serde_json::to_value(reboot_budget(&phase, &segment))?;
    report["bootProbeIntervalSeconds"] = json!(5);
    report["systemTimeReadFormat"] =
        json!("date -u +%Y-%m-%dT%H:%M:%SZ；系统/RTC分别即时采电脑UTC");
    eprintln!(
        "[QA_SEGMENT_PLAN] {phase} mask{}..{} clear={} targets={}，计划上限={}",
        segment.start_mask,
        segment.end_mask,
        segment.include_clear,
        segment.target_ips.join("、"),
        report["plannedMaximumProductReboots"]
    );
    let mut backups = Vec::new();
    let mut mutation_possible = false;
    let outcome: TestResult<()> = async {
        backups = backup(&device,&segment.target_ips).await?;
        report["actualTargets"] = serde_json::to_value(&backups)?;
        bind_initial_baseline(&backups,&mut report)?;
        report["backupCompleteBeforeWrite"] = json!(true);
        save(&directory, &report)?;
        let project = Stage75Adapter::new(&state)
            .create_project(support::local_input())
            .await?
            .project
            .id;
        report["projectId"] = json!(project);
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let mut screens = Vec::new();
        for b in &backups {
            let id = repo
                .save_local(
                    &project,
                    &ScreenFields {
                        name: format!("阶段A{}寸{}", b.size, b.ip),
                        ip: b.ip.clone(),
                        mac: b.wired_mac.clone(),
                        size: b.size.clone(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await?;
            let asset = repo.asset(&project, &id).await?;
            let observed = device
                .inspect(&asset, "inspect", CancellationToken::new())
                .await?;
            require(
                observed.adb_available
                    && observed.observed_mac.as_deref().map(normalize_mac)
                        == Some(b.wired_mac.clone()),
                "正式有线身份检查失败",
            )?;
            screens.push(asset);
        }
        report["assetIds"] = serde_json::to_value(&screens)?;
        save(&directory, &report)?;
        if phase == "basic" {
            let skip_matrix=std::env::var("INX_SCREEN_SETTINGS_REAL_SKIP_MATRIX").as_deref()==Ok("1");
            report["startMask"]=json!(segment.start_mask);report["endMask"]=json!(segment.end_mask);report["matrixSkippedByExplicitQaRequest"]=json!(skip_matrix);
            for mask in if skip_matrix {0..0} else {0..16} {
                formal(
                    &state,
                    &device,
                    &project,
                    &screens,
                    &backups,
                    mask,
                    "192.168.3.142",
                    u32::from(mask % 3 + 1),
                    &format!("matrix-{mask:02}"),
                    false,
                    false,
                    &directory,
                    &mut report,
                )
                .await?;
            }
            for mask in segment.start_mask..=segment.end_mask {
                mutation_possible = true;
                prepare(&device, &backups, &screens, mask, &directory, &mut report).await?;
                formal(
                    &state,
                    &device,
                    &project,
                    &screens,
                    &backups,
                    mask,
                    "192.168.3.142",
                    u32::from((mask - 1) % 3 + 1),
                    &format!("execute-{mask:02}-three"),
                    true,
                    false,
                    &directory,
                    &mut report,
                )
                .await?;
            }
            let repeated=formal(
                &state,
                &device,
                &project,
                &screens,
                &backups,
                segment.end_mask,
                "192.168.3.142",
                3,
                "default-all-skip-after-success",
                false,
                false,
                &directory,
                &mut report,
            )
            .await?;
            require(repeated["allSkipNoTask"]==true,"同值目标未全部跳过，不自动追加执行或重启")?;
        } else {
            require(
                std::env::var("INX_SCREEN_SETTINGS_NTP_CONFIRMED").as_deref()
                    == Ok("192.168.3.142"),
                "须先确认142当前UDP授时服务有效",
            )?;
            require(backups.iter().all(|b|b.settings.get("global.ntp_server").map(String::as_str)==Some("192.168.3.142")
                && b.settings.get("global.auto_time").map(String::as_str)==Some("1")),
                "本轮NTP恢复验证只适用于已确认原142地址且自动时间开启的三屏，其他原值须另定观察依据")?;
            report["startMask"]=json!(segment.start_mask);report["endMask"]=json!(segment.end_mask);
            for mask in segment.start_mask..=segment.end_mask {
                mutation_possible = true;
                prepare(&device, &backups, &screens, mask, &directory, &mut report).await?;
                formal(
                    &state,
                    &device,
                    &project,
                    &screens,
                    &backups,
                    mask,
                    "192.168.3.142",
                    u32::from((mask - 1) % 3 + 1),
                    &format!("execute-{mask:02}-three"),
                    true,
                    false,
                    &directory,
                    &mut report,
                )
                .await?;
            }
            let repeated=formal(
                &state,
                &device,
                &project,
                &screens,
                &backups,
                segment.end_mask,
                "192.168.3.142",
                2,
                "same-segment-target-skip",
                false,
                false,
                &directory,
                &mut report,
            )
            .await?;
            require(repeated["allSkipNoTask"]==true,"同值NTP目标未全部跳过，不自动追加执行或重启")?;
            if segment.include_clear {
            for screen in &screens {
                report["ntpRestoreRebootRequired"][&screen.fields.ip] = json!(true);
                report["ntpRestoreReferenceBoot"][&screen.fields.ip]=json!(read(&device,&screen.fields.ip,&["cat","/proc/sys/kernel/random/boot_id"]).await?);
            }
            save(&directory, &report)?;
            prepare(&device,&backups,&screens,6,&directory,&mut report).await?;
            formal(
                &state,
                &device,
                &project,
                &screens,
                &backups,
                14,
                "",
                1,
                "ntp-clear-with-time-sleep-selected",
                true,
                true,
                &directory,
                &mut report,
            )
            .await?;
            formal(
                &state,
                &device,
                &project,
                &screens,
                &backups,
                8,
                "192.168.3.142",
                1,
                "ntp-set-back-internal-selected",
                true,
                false,
                &directory,
                &mut report,
            )
            .await?;
            } else {report["clearAndSetBackDeferred"]=json!(true);}
        }
        Ok(())
    }
    .await;
    report["verificationPassed"] = json!(outcome.is_ok());
    report["error"] = json!(outcome.as_ref().err().map(ToString::to_string));
    let outcome_save_error = save(&directory, &report)
        .err()
        .map(|error| error.to_string());
    let stopped = state.task_queue.shutdown(Duration::from_secs(30)).await;
    report["queueStopOutcomes"] = json!(
        stopped
            .iter()
            .map(|(id, outcome)| json!({"taskId":id,"outcome":format!("{outcome:?}")}))
            .collect::<Vec<_>>()
    );
    report["isolatedQueueStoppedBeforeRestore"] = json!(true);
    report["restoring"] = json!(true);
    let mut restore_errors = Vec::new();
    if let Some(error) = outcome_save_error {
        restore_errors.push(format!("测试结果保存失败：{error}"));
    }
    if mutation_possible {
        for before in &backups {
            match restore(&state, &device, before, &directory, &mut report).await {
                Ok(after) => report["restoration"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"ip":before.ip,"restored":true,"after":after})),
                Err(error) => {
                    restore_errors.push(format!("{}:{error}", before.ip));
                    report["restoration"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"ip":before.ip,"restored":false,"error":error.to_string()}));
                }
            }
            if let Err(error) = save(&directory, &report) {
                restore_errors.push(format!("恢复证据保存失败：{error}"));
            }
        }
    }
    report["restored"] = json!(restore_errors.is_empty());
    report["restoreErrors"] = json!(restore_errors);
    report["passed"] = json!(outcome.is_ok() && restore_errors.is_empty());
    let final_save = save(&directory, &report);
    support::close(state).await;
    final_save?;
    require(
        restore_errors.is_empty(),
        "收尾尚未全部恢复，必须先处理原证据中的设备",
    )?;
    outcome
}

#[tokio::test]
#[ignore = "需INX_SCREEN_SETTINGS_REAL=1及原basic07或NTP10精确路径/标签；仅核当前状态，禁止设置/启动/重启/自动续跑"]
async fn read_only_verify_original_unknown_without_replaying() -> TestResult<()> {
    require(
        std::env::var("INX_SCREEN_SETTINGS_REAL").as_deref() == Ok("1"),
        "缺少显式实机门禁",
    )?;
    let directory = std::path::PathBuf::from(
        std::env::var_os("INX_SCREEN_SETTINGS_VERIFY_SOURCE").ok_or("必须明确原隔离证据目录")?,
    )
    .canonicalize()?;
    let evidence_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".review-tools/combined-real-20261104/evidence")
        .canonicalize()?;
    let name = directory
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("证据目录名无效")?;
    let requested_label = std::env::var("INX_SCREEN_SETTINGS_VERIFY_CASE_LABEL").ok();
    let label = approved_verification_label(name, requested_label.as_deref())?;
    require(
        directory.parent() == Some(evidence_root.as_path()),
        "只允许本轮evidence内两个固定目录，禁止生产或其他目录",
    )?;
    let original = std::fs::read(directory.join("result.json"))?;
    let report: Value = serde_json::from_slice(&original)?;
    let project = report["projectId"].as_str().ok_or("原项目缺失")?;
    let case = report["cases"]
        .as_array()
        .ok_or("原案例缺失")?
        .iter()
        .find(|v| v["label"] == label)
        .ok_or("固定标签的原案例缺失")?;
    let task = case["taskId"].as_str().ok_or("原任务缺失")?;
    let state = support::state_at(&directory, false).await;
    let outcome:TestResult<Value>=async {
        let device=AdbDevice::new(audited_tools()?);
        for (ip,_,serial,wifi) in GUARDS {
            require(read(&device,ip,&["getprop","ro.serialno"]).await?==*serial,"核实前物理序列号不符")?;
            require(normalize_mac(&read(&device,ip,&["cat","/sys/class/net/wlan0/address"]).await?)==normalize_mac(wifi),"核实前用户WiFiMAC不符")?;
        }
        let before=task_data::read_results(state.local_store.pool(),project,task).await?;
        let old_state=state.task_repository.get(task).await?.state;
        let offset=audit_offset()?;
        tasks::verify(&state,project,task).await?;
        let after=task_data::read_results(state.local_store.pool(),project,task).await?;
        let final_state=state.task_repository.get(task).await?.state;
        let protected=state.task_repository.results_protected(task).await?;
        for (id,value) in &before.targets {
            if value.device==inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Succeeded {
                require(after.targets.get(id).is_some_and(|v|v.evidence["screenSettings"]["items"]==value.evidence["screenSettings"]["items"]),"只读核实覆盖了已成功子项")?;
            }
            if value.evidence["screenSettings"]["items"]["time"]["state"]=="not_started" {
                require(after.targets.get(id).is_some_and(|value|value.evidence["screenSettings"]["items"]["time"]["state"]=="not_started"),"只读核实接续了尚未开始的time子项")?;
            }
        }
        let mut side_effects=Vec::new();
        let audit=std::path::PathBuf::from(std::env::var_os("INX_COMBINED_ADB_AUDIT").ok_or("审计目录缺失")?);
        for entry in std::fs::read_dir(audit)? {
            let value:Value=serde_json::from_slice(&std::fs::read(entry?.path())?)?;
            let args=value["args"].as_array().ok_or("命令审计无参数")?.iter().map(|v|v.as_str().unwrap_or("")).collect::<Vec<_>>();
            if audit_has_device_side_effect(&args) {side_effects.push(value);}
        }
        require(side_effects.is_empty(),"原任务核实发生设备写入或重放")?;
        let had_unknown=before.targets.values().any(|target|matches!(target.device,inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Pending|inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Unknown));
        require(audit_coverage_confirmed(had_unknown,offset,audit_offset()?),"缺少实际只读核实命令审计，不能把空记录算作零副作用证明")?;
        require(!protected&&final_state!=TaskState::FinalizingFailed,"只读核实仍有未知，不能启动新实写case")?;
        require(std::fs::read(directory.join("result.json"))?==original,"核实覆盖了原result文件")?;
        Ok(json!({"passed":true,"readOnly":true,"currentStateVerificationOnly":true,"doesNotInferOriginalTimeoutWindowSuccess":true,"priorQaRestoreRebootOccurred":label=="execute-10-three","sourceCaseLabel":label,"sourceResultPreserved":true,"taskId":task,"beforeTaskState":old_state.as_str(),"afterTaskState":final_state.as_str(),"before":before,"after":after,"resultsProtected":protected,"auditStartOffset":offset,"auditEndOffset":audit_offset()?,"deviceSideEffects":side_effects}))
    }.await;
    let evidence = match &outcome {
        Ok(value) => value.clone(),
        Err(error) => json!({"passed":false,"readOnly":true,"error":error.to_string()}),
    };
    let verification_path = directory.join(format!(
        "read-only-original-verification-{}.json",
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    ));
    let verification_bytes = serde_json::to_vec_pretty(&evidence)?;
    std::fs::write(&verification_path, &verification_bytes)?;
    let first = directory.join("read-only-original-verification.json");
    if !first.exists() {
        std::fs::write(first, &verification_bytes)?;
    }
    std::fs::write(
        directory.join("read-only-original-verification-latest.json"),
        serde_json::to_vec_pretty(
            &json!({"file":verification_path,"sha256":format!("{:x}",Sha256::digest(&verification_bytes))}),
        )?,
    )?;
    eprintln!("[QA_READ_ONLY_RESULT] {}", verification_path.display());
    support::close(state).await;
    outcome?;
    Ok(())
}

#[cfg(test)]
mod qa_helpers {
    use super::*;
    const OLD: &str = "11111111-1111-4111-8111-111111111111";
    const NEW: &str = "22222222-2222-4222-8222-222222222222";

    #[test]
    fn both_entries_require_the_same_actual_audited_tool() {
        let directory = tempfile::tempdir().unwrap();
        let proxy = directory.path().join("adb-audit.exe");
        let sdk = directory.path().join("sdk-adb.exe");
        std::fs::write(&proxy, b"fixture, never executed").unwrap();
        std::fs::write(&sdk, b"fixture, never executed").unwrap();
        assert!(require_audited_tool(&proxy, Some(&proxy)).is_ok());
        assert!(require_audited_tool(&sdk, Some(&proxy)).is_err());
        assert!(require_audited_tool(&sdk, Some(&sdk)).is_err());
        assert!(require_audited_tool(&proxy, None).is_err());
    }
    #[test]
    fn empty_audit_is_never_zero_side_effect_proof_for_an_unknown_recovery() {
        assert!(!audit_coverage_confirmed(true, 0, 0));
        assert!(!audit_coverage_confirmed(true, 6, 6));
        assert!(!audit_coverage_confirmed(true, 6, 5));
        assert!(audit_coverage_confirmed(true, 6, 7));
        assert!(audit_coverage_confirmed(false, 6, 6));
    }

    #[test]
    fn original_verification_has_exact_two_directory_and_label_pairs() {
        assert_eq!(
            approved_verification_label(
                "combined-basic-01a1a4d1-2fc2-7200-b85d-c07f98baa500",
                None
            )
            .unwrap(),
            "execute-07-three"
        );
        assert_eq!(
            approved_verification_label(
                "combined-ntp-01a1a506-bd9e-7360-8b9c-ebe4ac9393ad",
                Some("execute-10-three")
            )
            .unwrap(),
            "execute-10-three"
        );
        assert!(
            approved_verification_label(
                "combined-ntp-01a1a506-bd9e-7360-8b9c-ebe4ac9393ad",
                Some("execute-07-three")
            )
            .is_err()
        );
        assert!(
            approved_verification_label("combined-ntp-another-case", Some("execute-10-three"))
                .is_err()
        );
    }
    #[test]
    fn read_only_audit_rejects_start_restart_resume_and_configuration_writes() {
        let prefix = ["-s", "192.168.3.42:5555", "shell"];
        for command in [
            vec!["am", "start", "package"],
            vec!["am", "force-stop", "package"],
            vec!["input", "keyevent", "224"],
            vec!["settings", "put", "global", "auto_time", "1"],
            vec!["date", "-u", "@1"],
            vec!["hwclock", "-w", "-u"],
            vec!["svc", "wifi", "enable"],
            vec!["pm", "grant", "package", "permission"],
            vec!["sh", "-c", "'reboot'"],
        ] {
            let all = prefix.into_iter().chain(command).collect::<Vec<_>>();
            assert!(audit_has_device_side_effect(&all));
        }
        for command in [
            vec!["getprop", "ro.serialno"],
            vec!["settings", "get", "global", "auto_time"],
            vec!["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"],
            vec!["sh", "-c", "'test -c /dev/rtc0'"],
            vec!["su", "0", "id"],
        ] {
            let all = prefix.into_iter().chain(command).collect::<Vec<_>>();
            assert!(!audit_has_device_side_effect(&all));
        }
        assert!(audit_has_device_side_effect(&[
            "-s",
            "192.168.3.42:5555",
            "reboot"
        ]));
    }

    #[test]
    fn default_and_only_fifteen_segments_have_exact_scope() {
        let full = segment_options("ntp", None, None, None, None).unwrap();
        assert_eq!(
            (
                full.start_mask,
                full.end_mask,
                full.include_clear,
                full.target_ips.len()
            ),
            (8, 15, true, 3)
        );
        let only = segment_options("ntp", Some("15"), Some("15"), Some("0"), None).unwrap();
        assert_eq!(
            (only.start_mask..=only.end_mask).collect::<Vec<_>>(),
            vec![15]
        );
        assert!(!only.include_clear);
        assert!(
            reboot_budget("ntp", &only)
                .values()
                .all(|count| *count == 1)
        );
    }
    #[test]
    fn invalid_segments_and_non_whitelisted_targets_are_rejected() {
        for (start, end, clear, targets) in [
            ("7", "15", "0", "192.168.3.42"),
            ("8", "16", "0", "192.168.3.42"),
            ("15", "14", "0", "192.168.3.42"),
            ("8", "15", "true", "192.168.3.42"),
            ("8", "15", "0", "192.168.3.99"),
            ("8", "15", "0", "192.168.3.42,192.168.3.42"),
            ("8", "15", "0", ""),
            ("8", "15", "0", "192.168.3.42:5555"),
        ] {
            assert!(
                segment_options("ntp", Some(start), Some(end), Some(clear), Some(targets)).is_err()
            );
        }
        assert!(segment_options("basic", None, None, Some("1"), None).is_err());
    }
    #[test]
    fn remaining_three_segments_budget_eight_ten_inch_reboots_without_repeating_fifteen() {
        let plans = [
            segment_options("ntp", Some("15"), Some("15"), Some("0"), None).unwrap(),
            segment_options(
                "ntp",
                Some("10"),
                Some("10"),
                Some("0"),
                Some("192.168.3.42"),
            )
            .unwrap(),
            segment_options("ntp", Some("11"), Some("14"), Some("1"), None).unwrap(),
        ];
        let mut totals = BTreeMap::<String, usize>::new();
        for plan in plans {
            for (ip, count) in reboot_budget("ntp", &plan) {
                *totals.entry(ip).or_default() += count;
            }
        }
        assert_eq!(totals["192.168.3.42"], 8);
        assert_eq!(totals["192.168.3.74"], 7);
        assert_eq!(totals["192.168.3.82"], 7);
    }
    #[test]
    fn boot_probe_never_reads_queued_completed_or_unready_devices() {
        let pending = json!({"device":"pending","evidence":{"phase":"screen_settings_reboot_requested","screenSettings":{"reboot":{"requested":true,"bootCompleted":false,"afterBootId":null}}}});
        assert!(needs_boot_probe(&pending));
        for state in ["succeeded", "failed", "cancelled"] {
            let mut value = pending.clone();
            value["device"] = json!(state);
            assert!(!needs_boot_probe(&value));
        }
        assert!(!needs_boot_probe(&json!({"device":"pending"})));
        let mut done = pending;
        done["evidence"]["screenSettings"]["reboot"]["bootCompleted"] = json!(true);
        assert!(!needs_boot_probe(&done));
        assert!(get_state_allows_boot(true, "device\r\n", ""));
        for (success, stdout, stderr) in [
            (false, "device", ""),
            (true, "offline", ""),
            (true, "", ""),
            (true, "device", "error"),
        ] {
            assert!(!get_state_allows_boot(success, stdout, stderr));
        }
    }
    #[test]
    fn boot_probe_accepts_real_unknown_waiting_phase_but_never_a_stopped_unknown() {
        // 正式aggregate在已请求但未确认boot时产生Unknown；不是终态失败。
        let waiting = json!({"device":"unknown","evidence":{"phase":"screen_settings_reboot_requested","screenSettings":{"reboot":{"required":true,"requested":true,"beforeBootId":OLD,"afterBootId":null}}}});
        assert!(needs_boot_probe(&waiting));
        let mut stopped = waiting.clone();
        stopped["evidence"]["phase"] = json!("screen_settings_stopped");
        stopped["evidence"]["screenSettings"]["stoppedAt"] = json!("reboot");
        assert!(!needs_boot_probe(&stopped));
        let mut cancelled = waiting.clone();
        cancelled["evidence"]["screenSettings"]["cancelled"] = json!(true);
        assert!(!needs_boot_probe(&cancelled));
        let mut not_started = waiting;
        not_started["evidence"]["screenSettings"]["reboot"]["requested"] = json!(false);
        assert!(!needs_boot_probe(&not_started));
    }
    fn clear_fixture(success: bool) -> Value {
        json!({"device":if success{"succeeded"}else{"failed"},"evidence":{"screenSettings":{
            "items":{"ntp":{"state":if success{"succeeded"}else{"failed"},"evidence":{"targetServer":"","saveConfirmed":true,"activationConfirmed":true,"syncConfirmed":success}},
            "time":{"state":if success{"skipped"}else{"not_started"}},"sleepOff":{"state":if success{"succeeded"}else{"not_started"}}},
            "reboot":{"requested":true,"beforeBootId":OLD,"afterBootId":NEW,"bootCompleted":true}}}})
    }
    #[test]
    fn clear_acceptance_distinguishes_synchronization_and_environment_failure() {
        assert_eq!(
            clear_outcome(&clear_fixture(true), 14).unwrap(),
            ClearOutcome::Succeeded
        );
        assert_eq!(
            clear_outcome(&clear_fixture(false), 14).unwrap(),
            ClearOutcome::ExpectedUnsyncedDefault
        );
        for (field, value) in [
            ("targetServer", json!("192.168.3.142")),
            ("saveConfirmed", Value::Null),
            ("saveConfirmed", json!(false)),
            ("activationConfirmed", json!(false)),
            ("syncConfirmed", Value::Null),
        ] {
            let mut target = clear_fixture(false);
            target["evidence"]["screenSettings"]["items"]["ntp"]["evidence"][field] = value;
            assert!(clear_outcome(&target, 14).is_err());
        }
        let mut unknown = clear_fixture(false);
        unknown["device"] = json!("unknown");
        assert!(clear_outcome(&unknown, 14).is_err());
        let mut executed = clear_fixture(false);
        executed["evidence"]["screenSettings"]["items"]["time"]["state"] = json!("succeeded");
        assert!(clear_outcome(&executed, 14).is_err());
        let mut no_boot = clear_fixture(false);
        no_boot["evidence"]["screenSettings"]["reboot"]["afterBootId"] = json!(OLD);
        assert!(clear_outcome(&no_boot, 14).is_err());
    }
    fn source_fixture() -> (Value, Value) {
        (
            json!({"bootId":NEW,"server":"192.168.3.142","autoTime":true,"activationConfirmed":true,"syncConfirmed":true,
            "status":{"systemServerPid":501},"sourceEvent":{"systemServerPid":501,"address":"/192.168.3.142"}}),
            json!({"kind":"success","systemServerPid":501,"address":"/192.168.3.142"}),
        )
    }
    #[test]
    fn original_source_requires_new_consistent_boot_and_direct_fresh_current_pid() {
        let (current, latest) = source_fixture();
        assert!(original_source_confirmed(
            OLD,
            NEW,
            NEW,
            "1",
            "192.168.3.142",
            &current,
            &latest
        ));
        assert!(!original_source_confirmed(
            OLD,
            NEW,
            OLD,
            "1",
            "192.168.3.142",
            &current,
            &latest
        ));
        assert!(!original_source_confirmed(
            NEW,
            NEW,
            NEW,
            "1",
            "192.168.3.142",
            &current,
            &latest
        ));
        assert!(!original_source_confirmed(
            OLD,
            NEW,
            NEW,
            "",
            "192.168.3.142",
            &current,
            &latest
        ));
        for field in ["sourceEvent", "status", "syncConfirmed", "autoTime"] {
            let mut missing = current.clone();
            missing[field] = Value::Null;
            assert!(!original_source_confirmed(
                OLD,
                NEW,
                NEW,
                "1",
                "192.168.3.142",
                &missing,
                &latest
            ));
        }
        let mut stale = current;
        stale["sourceEvent"]["systemServerPid"] = json!(500);
        assert!(!original_source_confirmed(
            OLD,
            NEW,
            NEW,
            "1",
            "192.168.3.142",
            &stale,
            &latest
        ));
        let (current, _) = source_fixture();
        let newer_other = json!({"systemServerPid":501,"address":"/192.0.2.1"});
        assert!(!original_source_confirmed(
            OLD,
            NEW,
            NEW,
            "1",
            "192.168.3.142",
            &current,
            &newer_other
        ));
    }
    #[test]
    fn restoration_never_turns_missing_proof_into_a_reboot() {
        let confirmed = json!({"snapshotReliable":true,"confirmed":true});
        assert_eq!(
            restore_decision(&confirmed, false, "192.168.3.142"),
            RestoreDecision::ConfirmedReadOnly
        );
        for missing in [
            json!({}),
            json!({"snapshotReliable":false}),
            json!({"snapshotReliable":true,"confirmed":false}),
        ] {
            assert_eq!(
                restore_decision(&missing, false, "192.168.3.142"),
                RestoreDecision::Unconfirmed
            );
        }
        assert_eq!(
            restore_decision(&json!({"snapshotReliable":false}), true, "192.168.3.142"),
            RestoreDecision::Unconfirmed
        );
        let other = json!({"snapshotReliable":true,"latestActivity":{"address":"time.android.com/192.0.2.1"}});
        assert_eq!(
            restore_decision(&other, false, "192.168.3.142"),
            RestoreDecision::KnownReloadNeeded
        );
    }
    #[test]
    fn latest_current_process_activity_prevents_old_source_success_from_hiding_a_later_source() {
        let log = "I/ntp_success(501): [/192.168.3.142,8,1]\r\nI/ntp_failure(501): [time.android.com/192.0.2.1,timeout]\nI/ntp_success(999): [/192.168.3.142,8,1]";
        let latest = latest_ntp_activity(log, 501).unwrap();
        assert_eq!(latest["kind"], "failure");
        assert_eq!(latest["address"], "time.android.com/192.0.2.1");
        assert!(latest_ntp_activity(log, 123).is_none());
    }
    #[test]
    fn only_complete_different_source_activation_can_request_a_qa_reload() {
        let mut report = json!({"assetIds":[{"id":"screen","ip":"192.168.3.42"}],"cases":[{"results":{"targets":{"screen":{"evidence":{"screenSettings":{
            "reboot":{"requested":true,"bootCompleted":true,"beforeBootId":OLD,"afterBootId":NEW},
            "items":{"ntp":{"evidence":{"saveConfirmed":true,"activationConfirmed":true,"targetServer":""}}}}}}}}}]});
        assert!(recorded_other_activation(
            &report,
            "192.168.3.42",
            NEW,
            "192.168.3.142"
        ));
        report["cases"][0]["results"]["targets"]["screen"]["evidence"]["screenSettings"]["reboot"]
            ["bootCompleted"] = Value::Null;
        assert!(!recorded_other_activation(
            &report,
            "192.168.3.42",
            NEW,
            "192.168.3.142"
        ));
    }
    #[test]
    fn later_segment_baseline_excludes_volatile_time_boot_and_pid_but_preserves_original_settings()
    {
        let current = Backup {
            ip: "192.168.3.42".into(),
            size: "10".into(),
            serial: "approved".into(),
            wifi_mac: "62914C938157".into(),
            wired_mac: "26DA357D85B5".into(),
            boot_id: OLD.into(),
            rtc: "rtc0".into(),
            root_mode: "su0".into(),
            settings: KEYS
                .iter()
                .map(|(ns, key)| (format!("{ns}.{key}"), "0".into()))
                .collect(),
            persistent_adb: "5555".into(),
            service_adb: "5555".into(),
            time_zone: "Asia/Shanghai".into(),
            system_seconds: 1,
            rtc_seconds: 1,
            app_pids: "10".into(),
            wakefulness: "Awake".into(),
            display: "ON".into(),
        };
        let formal = serde_json::to_value(&current).unwrap();
        let power = "mWakefulness=Awake\r\nDisplay Power: state=ON\r\n";
        let mut later = current;
        later.system_seconds = 999;
        later.rtc_seconds = 999;
        later.boot_id = NEW.into();
        later.app_pids = "20".into();
        assert!(baseline_matches(&later, &formal, power));
        later.settings.insert("global.auto_time".into(), "1".into());
        assert!(!baseline_matches(&later, &formal, power));
    }
    #[test]
    fn case_checkpoint_keeps_real_terminal_results_when_after_snapshot_fails() {
        let directory = tempfile::tempdir().unwrap();
        let mut report = json!({"cases":[]});
        let mut case = json!({"label":"segment-15","taskState":"finalizing_failed","taskTerminalObserved":true,"results":{"targets":{"screen":{"device":"unknown","evidence":{"phase":"reboot_requested"}}}},"after":[],"afterErrors":[]});
        store_case(directory.path(), &mut report, &case).unwrap();
        case["afterErrors"]
            .as_array_mut()
            .unwrap()
            .push(json!({"ip":"192.168.3.42","error":"只读未连接"}));
        store_case(directory.path(), &mut report, &case).unwrap();
        let saved: Value =
            serde_json::from_slice(&std::fs::read(directory.path().join("result.json")).unwrap())
                .unwrap();
        assert_eq!(saved["cases"].as_array().unwrap().len(), 1);
        assert_eq!(saved["cases"][0]["taskState"], "finalizing_failed");
        assert_eq!(
            saved["cases"][0]["results"]["targets"]["screen"]["device"],
            "unknown"
        );
        assert_eq!(
            saved["cases"][0]["afterErrors"].as_array().unwrap().len(),
            1
        );
    }
}
