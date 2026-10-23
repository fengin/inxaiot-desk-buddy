//! 锁定/Owner 正式链路实机回归。默认忽略；只使用新隔离库及测试说明中的指定三屏。
//! 仅复用已有 self Owner，不制造权限缺失场景，不移除 Owner，不清数据，不连接平台。
#[path = "common/screen_test_support.rs"]
mod support;
#[path = "common/kiosk_fullscreen_support.rs"]
mod fullscreen;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            app_config::{field_value, AppConfigPatch, AppConfigRead},
            app_config_schema::{values_equal, ConfigSchema},
            model::{ResultState, ScreenFields, ScreenTargetResult},
            operation::ScreenOperationInput,
            rules::{normalize_mac, valid_mac},
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            app_config,
            device::{parse_package_version, AdbDevice, AndroidTools},
            device_owner, kiosk, maintenance, task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const TARGETS: [(&str, &str, &str); 3] = [
    ("192.168.3.42", "9c000c3c3643071205d", "10"),
    ("192.168.3.74", "A47A4AK02040", "4"),
    ("192.168.3.82", "A47A5CK00524", "4"),
];
const NAME: &str = "customDeviceName";
const AUDIO: &str = "audioConfig.keywordsScore";
const ENV_KEYS: [&str; 8] = [
    "INX_KIOSK_OWNER_REAL",
    "INX_KIOSK_OWNER_TARGET",
    "INX_KIOSK_OWNER_EXPECTED_SERIAL",
    "INX_KIOSK_OWNER_MODE",
    "INX_KIOSK_OWNER_ALLOW_WRITES",
    "INX_KIOSK_OWNER_CASE",
    "INX_KIOSK_FULLSCREEN_REAL",
    "INX_KIOSK_LIFECYCLE_CONFIG_SHA256",
];

#[derive(Debug)]
struct Authorization {
    ip: String,
    serial: String,
    mode: String,
    case: Option<String>,
    fullscreen: bool,
    lifecycle_config_sha256: Option<String>,
}
fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c|c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn authorization(
    values: &BTreeMap<String, String>,
    screens: &[support::LiveScreen],
) -> TestResult<Authorization> {
    let get = |key: &str| values.get(key).map(String::as_str).unwrap_or("");
    require(
        get(ENV_KEYS[0]) == "YES",
        "缺少明确门禁，未连接设备或创建数据",
    )?;
    let mode = get(ENV_KEYS[3]);
    require(
        ["read-only", "roundtrip", "restore", "seed-gui", "lifecycle"].contains(&mode),
        "未指定有效测试模式",
    )?;
    let ip = get(ENV_KEYS[1]);
    let serial = get(ENV_KEYS[2]);
    let case = get(ENV_KEYS[5]);
    require(
        get(ENV_KEYS[6]).is_empty() || get(ENV_KEYS[6]) == "YES",
        "全屏扩展门禁只接受空值或YES",
    )?;
    let fullscreen = get(ENV_KEYS[6]) == "YES";
    let lifecycle_config_sha256 = values.get(ENV_KEYS[7]).cloned();
    if let Some(hash) = &lifecycle_config_sha256 {
        require(mode == "lifecycle" && is_lowercase_sha256(hash),
            "配置基线覆盖仅允许生命周期补验显式提供64位小写十六进制SHA256；不接受空值或其他模式")?;
    }
    require(
        !fullscreen || ["roundtrip", "restore", "lifecycle"].contains(&mode),
        "全屏生命周期扩展仅允许已授权的完整回归或恢复模式",
    )?;
    if mode == "seed-gui" {
        require(
            ip.is_empty() && serial.is_empty() && case.is_empty(),
            "GUI种子模式不接受设备目标或恢复目录",
        )?;
    } else {
        let target = TARGETS
            .iter()
            .find(|(target, expected, _)| *target == ip && *expected == serial)
            .ok_or("IP和序列号不在本轮授权白名单")?;
        require(
            screens
                .iter()
                .any(|screen| screen.ip == ip && screen.size == target.2 && valid_mac(&screen.mac)),
            "当前测试说明没有匹配尺寸和MAC的目标，不回退旧地址",
        )?;
        require(
            mode == "read-only" || get(ENV_KEYS[4]) == "YES",
            "写入/恢复缺少独立ALLOW_WRITES门禁",
        )?;
    }
    require(
        ["restore", "lifecycle"].contains(&mode) == !case.is_empty(),
        "仅恢复或生命周期补验模式接受原用例UUID",
    )?;
    require(mode != "lifecycle" || fullscreen, "生命周期补验必须明确开启全屏扩展门禁")?;
    if !case.is_empty() {
        require(
            uuid::Uuid::parse_str(case).is_ok_and(|id| id.to_string() == case),
            "恢复目录仅接受UUID，不接受路径",
        )?;
    }
    Ok(Authorization {
        ip: ip.into(),
        serial: serial.into(),
        mode: mode.into(),
        case: (!case.is_empty()).then(|| case.into()),
        fullscreen,
        lifecycle_config_sha256,
    })
}
fn sha(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON"))
    )
}
fn root() -> TestResult<PathBuf> {
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("仓库缺失")?
        .canonicalize()?)
}
fn evidence_root() -> TestResult<PathBuf> {
    let workspace = root()?;
    let base = workspace.join(".review-tools/kiosk-owner-20261023");
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    require(base.starts_with(&workspace), "证据根目录不在工作台仓库中")?;
    Ok(base)
}
fn persist(directory: &Path, record: &Value) -> TestResult<()> {
    let pending = directory.join("restore-state.pending.json");
    let mut file = std::fs::File::create(&pending)?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(pending, directory.join("restore-state.json"))?;
    Ok(())
}
fn phase(directory: &Path, record: &mut Value, name: &str) -> TestResult<()> {
    record["phase"] = json!(name);
    if !record["checkpointHistory"].is_array() {
        record["checkpointHistory"] = json!([]);
    }
    record["checkpointHistory"]
        .as_array_mut()
        .unwrap()
        .push(json!(name));
    persist(directory, record)
}
fn safe_for_formal_evidence(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.values().all(safe_for_formal_evidence),
        Value::Array(values) => values.iter().all(safe_for_formal_evidence),
        Value::String(text) if text.contains("://") => reqwest::Url::parse(text).is_ok_and(|url| {
            url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        }),
        _ => true,
    }
}
fn input(id: &str) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "app_config".into(),
        target_ids: vec![id.into()],
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 1,
        expected_targets: Default::default(),
        retry_of_operation_id: None,
    }
}
fn controls(enabled: bool) -> AppConfigPatch {
    AppConfigPatch {
        ensure_device_owner: Some(true),
        kiosk_enabled: Some(enabled),
        ..Default::default()
    }
}
fn set_value(patch: &mut AppConfigPatch, key: &str, value: &Value) {
    if value.is_null() {
        patch.clear.push(key.into());
    } else {
        patch.set.insert(key.into(), value.clone());
    }
}
fn without_test_fields(config: &Value) -> Value {
    let mut config = config.clone();
    if let Some(object) = config.as_object_mut() {
        object.remove(NAME);
    }
    if let Some(object) = config["audioConfig"].as_object_mut() {
        object.remove("keywordsScore");
    }
    config
}

fn safe_error(error: &dyn std::fmt::Display) -> String {
    let text = error.to_string();
    let text = regex::Regex::new(r"(?i)[a-z][a-z0-9+.-]*://[^\s]+")
        .unwrap().replace_all(&text, "<地址已省略>");
    let text = regex::Regex::new(r"(?i)(password|passwd|token|secret|authorization|authcode)\s*[:=]\s*[^\s,;]+")
        .unwrap().replace_all(&text, "$1=<已省略>");
    text.chars().filter(|c| !c.is_control()).take(1000).collect()
}

async fn identity(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    println!("身份检查：连接ADB");
    device.connect(&auth.ip, CancellationToken::new()).await
        .map_err(|error| format!("identity.connect: {}",safe_error(&error)))?;
    println!("身份检查：连接成功，读取硬件序列号");
    let serial = device
        .shell(
            &auth.ip,
            &["getprop", "ro.serialno"],
            CancellationToken::new(),
        )
        .await.map_err(|error| format!("identity.serial-read: {}",safe_error(&error)))?;
    if serial.trim() != auth.serial {
        return Err(format!("identity.serial-mismatch: expectedSha256={}, observedSha256={}",sha(&json!(auth.serial)),sha(&json!(serial.trim()))).into());
    }
    println!("身份检查：硬件序列号匹配，读取设备Owner");
    let owner = device_owner::read(device, &auth.ip).await;
    println!("身份检查：Owner state={}，{}",owner.state,owner.message);
    if owner.state != "self" {
        return Err(format!("identity.owner: state={}, message={}；仅允许已有小新Owner，不设置/替换/移除",owner.state,owner.message).into());
    }
    if auth.fullscreen {
        let package = device.shell(&auth.ip, &["dumpsys", "package", "chat.xiaoxin.app"], CancellationToken::new()).await?;
        require(parse_package_version(&package).1 == Some(20036),
            "全屏扩展仅允许本轮已修复的20036测试包，不在其他版本执行写入")?;
    }
    Ok(())
}
async fn runtime(device: &AdbDevice, ip: &str) -> TestResult<(String, String)> {
    let pid = device
        .shell(ip, &["pidof", "chat.xiaoxin.app"], CancellationToken::new())
        .await?;
    let boot = device
        .shell(
            ip,
            &["cat", "/proc/sys/kernel/random/boot_id"],
            CancellationToken::new(),
        )
        .await?;
    require(
        !pid.trim().is_empty()
            && pid
                .bytes()
                .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace())
            && uuid::Uuid::parse_str(boot.trim()).is_ok(),
        "运行标识不可确认",
    )?;
    Ok((pid.trim().into(), boot.trim().into()))
}
async fn capture_runtime(device: &AdbDevice, ip: &str, directory: &Path) -> TestResult<()> {
    let mut text = device
        .shell(
            ip,
            &["dumpsys", "activity", "activities"],
            CancellationToken::new(),
        )
        .await?;
    if !text.contains("mLockTaskModeState") {
        text.push('\n');
        text.push_str(
            &device
                .shell(ip, &["dumpsys", "activity"], CancellationToken::new())
                .await?,
        );
    }
    // 仅保留锁定解析需要的系统模式、任务及前台身份，不落盘Intent extras/网页地址。
    let selected = text
        .lines()
        .filter(|line| {
            [
                "ACTIVITY MANAGER ACTIVITIES",
                "mLockTask",
                "ResumedActivity",
                "TaskRecord{",
            ]
            .iter()
            .any(|key| line.contains(key))
                && !line.contains("://")
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(directory.join("activity-lock-state.txt"), selected)?;
    Ok(())
}
async fn read_one(state: &FormalAppState, project: &str, id: &str) -> TestResult<AppConfigRead> {
    let rows = app_config::read(state, project, &[id.into()]).await?;
    require(rows.len() == 1, "读取目标不唯一")?;
    let row = rows.into_iter().next().ok_or("读取结果缺失")?;
    require(
        row.config.is_some()
            && row
                .capabilities
                .as_ref()
                .is_some_and(|cap| cap["ready"] == true)
            && row
                .owner
                .as_ref()
                .is_some_and(|owner| owner.state == "self")
            && row.kiosk.as_ref().is_some_and(|kiosk| {
                kiosk.supported && kiosk.enabled.is_some() && kiosk.configured.is_some()
            })
            && row
                .home
                .as_ref()
                .is_some_and(|home| home.supported && home.enabled.is_some()),
        "配置/锁定/Owner/主屏状态不完整",
    )?;
    require(
        safe_for_formal_evidence(row.config.as_ref().unwrap()),
        "配置含不透明鉴权地址，未进入保存快照的正式任务",
    )?;
    Ok(row)
}
async fn settle(state: &FormalAppState, project: &str, id: &str) -> TestResult<()> {
    for task in state.task_repository.list_recent(project, 100).await? {
        if task.operation_type != "app_config" {
            continue;
        }
        require(
            !matches!(
                task.state,
                TaskState::Queued | TaskState::Running | TaskState::Cancelling
            ) && !state.job_supervisor.contains(&task.id).await,
            "原任务仍在运行，未并发操作",
        )?;
        let result = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
        if task.state == TaskState::FinalizingFailed
            || state.task_repository.results_protected(&task.id).await?
            || result
                .targets
                .get(id)
                .is_none_or(|row| matches!(row.device, ResultState::Pending | ResultState::Unknown))
        {
            tasks::verify(state, project, &task.id).await?;
            let after =
                task_data::read_results(state.local_store.pool(), project, &task.id).await?;
            require(
                !state.task_repository.results_protected(&task.id).await?
                    && after.targets.get(id).is_some_and(|row| {
                        !matches!(row.device, ResultState::Pending | ResultState::Unknown)
                    }),
                "结果仍待核实，保留恢复责任，不重放动作",
            )?;
        }
    }
    Ok(())
}
async fn submit(
    state: &FormalAppState,
    auth: &Authorization,
    project: &str,
    id: &str,
    patch: AppConfigPatch,
) -> TestResult<(String, ScreenTargetResult)> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    settle(state, project, id).await?;
    read_one(state, project, id).await?;
    patch.validate()?;
    let preview = maintenance::preflight_with_config(
        state,
        project,
        input(id),
        Some(BTreeMap::from([(id.into(), patch)])),
    )
    .await?;
    if preview.items.len() != 1 || preview.items[0].state != "ready" {
        let items = preview.items.iter().map(|item|json!({"state":item.state,"reason":safe_error(&item.reason)})).collect::<Vec<_>>();
        return Err(format!("正式预检未通过：{}",serde_json::to_string(&items)?).into());
    }
    identity(&device, auth).await?;
    let task_id = tasks::submit(state, project, &preview.id, input(id)).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let task = state.task_repository.get(&task_id).await?;
            if (task.state.is_terminal() || task.state == TaskState::FinalizingFailed)
                && !state.job_supervisor.contains(&task_id).await
            {
                break Ok::<(), inxaiot_desk_buddy_lib::core::error::AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await??;
    settle(state, project, id).await?;
    let result = task_data::read_results(state.local_store.pool(), project, &task_id)
        .await?
        .targets
        .remove(id)
        .ok_or("目标结果缺失")?;
    if !matches!(result.device, ResultState::Succeeded | ResultState::Skipped)
        || result.business != ResultState::NotRequired || result.shared != ResultState::NotRequired {
        return Err(format!("正式任务未完成：{}",json!({"taskId":task_id,"device":result.device,"business":result.business,
            "shared":result.shared,"message":safe_error(&result.message),"phase":result.evidence["phase"],
            "config":{"save":result.evidence["config"]["save"],"restart":result.evidence["config"]["restart"],"readback":result.evidence["config"]["readback"]},
            "owner":{"ensure":result.evidence["owner"]["ensure"],"readback":result.evidence["owner"]["readback"]},
            "kiosk":{"save":result.evidence["kiosk"]["save"],"apply":result.evidence["kiosk"]["apply"],"readback":result.evidence["kiosk"]["readback"]}})).into());
    }
    Ok((task_id, result))
}
fn runtime_lines(text: &str, power: bool) -> Vec<String> {
    let keys: &[&str] = if power {
        &["mWakefulness=", "mWakefulnessChanging=", "mIsPowered=", "mUserActivitySummary=",
            "mLastWakeTime=", "mLastSleepTime=", "mLastUserActivityTime=", "Display Power: state=", "mHoldingDisplaySuspendBlocker="]
    } else {
        &["ACTIVITY MANAGER ACTIVITIES", "mLockTask", "ResumedActivity", "TaskRecord{", "Task{",
            "mSleeping=", "mShuttingDown=", "mFocusedActivity", "mKeyguardShowing=", "mKeyguardGoingAway=", "mFocusedStack="]
    };
    text.lines().filter(|line| keys.iter().any(|key|line.contains(key)) && !line.contains("://") && !line.contains("Intent{"))
        .take(180).map(|line|line.trim().chars().take(600).collect()).collect()
}

async fn verify_lock(device: &AdbDevice, ip: &str, enabled: bool, directory: &Path) -> TestResult<()> {
    let checked = kiosk::wait_verified(device, ip, enabled).await;
    if checked.as_ref().is_ok_and(|(_,matched)|*matched) { return Ok(()) }
    // 在自动恢复之前保留只读现场；不唤醒、不重发save/apply、不改变锁屏或电源设置。
    let mut evidence = json!({"at":inxaiot_desk_buddy_lib::infrastructure::local_sqlite::screen_repository::now(),"targetEnabled":enabled});
    match &checked {
        Ok((status,matched)) => { evidence["providerAtFailedCheck"]=json!(status); evidence["matched"]=json!(matched); }
        Err(error) => evidence["verificationError"]=json!(safe_error(error)),
    }
    match kiosk::query(device,ip).await {
        Ok(status) => evidence["providerAfterFailedCheck"]=json!(status),
        Err(error) => evidence["providerError"]=json!(safe_error(&error)),
    }
    for (key,args,power) in [
        ("activities",vec!["dumpsys","activity","activities"],false),
        ("allActivity",vec!["dumpsys","activity"],false),
        ("power",vec!["dumpsys","power"],true),
    ] {
        match device.shell(ip,&args,CancellationToken::new()).await {
            Ok(text) => evidence[key]=json!(runtime_lines(&text,power)),
            Err(error) => evidence[key]=json!({"readError":safe_error(&error)}),
        }
    }
    let filename=format!("lock-check-failure-{}.json",uuid::Uuid::now_v7());
    let saved=std::fs::write(directory.join(&filename),serde_json::to_vec_pretty(&evidence)?);
    if let Err(error)=saved { return Err(format!("锁定核验失败且现场记录失败：{}",safe_error(&error)).into()) }
    Err(format!("Provider与系统锁定任务身份未同时符合目标；恢复前只读现场已保存：{filename}；Provider={}",evidence["providerAtFailedCheck"]).into())
}

async fn fullscreen_sample(device: &AdbDevice, ip: &str) -> TestResult<Value> {
    let windows = device.shell(ip, &["dumpsys", "window", "windows"], CancellationToken::new()).await?;
    let activities = device.shell(ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
    let parsed = fullscreen::main_window_flags(&windows);
    let resumed = fullscreen::main_is_resumed(&activities);
    Ok(match parsed {
        Ok(flags) => json!({"mainResumed":resumed,"systemUiVisibility":format!("0x{flags:x}"),
            "requiredFlags":"0x1006","fullscreen":resumed && flags & fullscreen::REQUIRED_SYSTEM_UI_FLAGS == fullscreen::REQUIRED_SYSTEM_UI_FLAGS}),
        Err(reason) => json!({"mainResumed":resumed,"fullscreen":false,"parseError":reason}),
    })
}

/// 首次采样及第二次确认均等待超过旧生命周期回调的 500ms，不用一次瞬态结果代替稳定全屏。
async fn verify_fullscreen(device: &AdbDevice, auth: &Authorization, directory: &Path, stage: &str) -> TestResult<()> {
    if !auth.fullscreen { return Ok(()); }
    tokio::time::sleep(Duration::from_millis(750)).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    let mut samples = Vec::new();
    let mut passed = false;
    loop {
        let sample = fullscreen_sample(device, &auth.ip).await?;
        let first_ok = sample["fullscreen"] == true;
        samples.push(sample);
        if first_ok {
            tokio::time::sleep(Duration::from_millis(750)).await;
            let confirmation = fullscreen_sample(device, &auth.ip).await?;
            passed = confirmation["fullscreen"] == true;
            samples.push(confirmation);
            // 已经成功全屏后被延迟回调破坏必须失败，不能继续等待掩盖回归。
            break;
        }
        if tokio::time::Instant::now() >= deadline { break; }
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let filename = format!("fullscreen-{}-{}.json", stage, uuid::Uuid::now_v7());
    std::fs::write(directory.join(&filename), serde_json::to_vec_pretty(&json!({
        "stage":stage,"passed":passed,"readOnly":true,"minimumSettleMs":750,"samples":samples,
        "assertion":"唯一可见小新MainActivity窗口并处于前台；FULLSCREEN、HIDE_NAVIGATION、IMMERSIVE_STICKY三位全部存在"
    }))?)?;
    require(passed, "小新主窗口未保持稳定沉浸全屏，见fullscreen阶段证据")
}

async fn settings_foreground(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    identity(device, auth).await?;
    device.shell(&auth.ip, &["am", "start", "-W", "-a", "android.settings.SETTINGS"], CancellationToken::new()).await?;
    tokio::time::sleep(Duration::from_millis(750)).await;
    let activities = device.shell(&auth.ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
    require(regex::Regex::new(r"(?m)^\s*(?:mResumedActivity:|mResumedActivity=|ResumedActivity:|topResumedActivity=)\s*ActivityRecord\{[^\r\n]*\bu0 com\.android\.settings/\S+ t\d+\}").unwrap().is_match(&activities),
        "未确认已切换到Android设置，不继续停止或重启小新")
}

async fn start_main(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    identity(device, auth).await?;
    device.shell(&auth.ip, &["am", "start", "-W", "-n", "chat.xiaoxin.app/.MainActivity"], CancellationToken::new()).await?;
    Ok(())
}

async fn activity_manager_warnings(device: &AdbDevice, ip: &str) -> TestResult<String> {
    // 不清空系统日志、不添加测试日志。只读限定标签，最终只落盘本次命令新增的精确拒停行。
    Ok(device.shell(ip, &["logcat", "-b", "all", "-d", "-v", "epoch", "-t", "200", "ActivityManager:W", "*:S"],
        CancellationToken::new()).await?)
}

async fn verify_cold_read(
    state: &FormalAppState, device: &AdbDevice, auth: &Authorization,
    project: &str, id: &str, expected_sha: &Value, directory: &Path,
) -> TestResult<()> {
    let before = device.shell(&auth.ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
    require(!fullscreen::main_activity_exists(&before), "force-stop后小新界面仍存在，未进入冷读取回归")?;
    let row = read_one(state, project, id).await?;
    tokio::time::sleep(Duration::from_millis(750)).await;
    let activities = device.shell(&auth.ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
    let services = device.shell(&auth.ip, &["dumpsys", "activity", "services", "chat.xiaoxin.app"], CancellationToken::new()).await?;
    let kiosk = kiosk::query(device, &auth.ip).await?;
    let evidence = json!({"configUnchanged":sha(row.config.as_ref().unwrap()) == *expected_sha,
        "mainActivityExists":fullscreen::main_activity_exists(&activities),
        "voiceServiceExists":fullscreen::voice_service_exists(&services),
        "activityReady":kiosk.activity_ready,"active":kiosk.active,"enabled":kiosk.enabled,"lockTaskMode":kiosk.lock_task_mode});
    std::fs::write(directory.join("fullscreen-cold-read.json"), serde_json::to_vec_pretty(&evidence)?)?;
    require(evidence["configUnchanged"] == true && evidence["mainActivityExists"] == false
        && evidence["voiceServiceExists"] == false && !kiosk.activity_ready && !kiosk.active
        && !kiosk.enabled && kiosk.lock_task_mode == "none",
        "冷配置读取改变了配置或启动了界面/语音/锁定；见fullscreen-cold-read证据")
}

/// 扩展仅切换/重启测试应用，普通配置不再增加写入；始终返回小新页面供后续人工查看。
async fn fullscreen_lifecycle(
    state: &FormalAppState, auth: &Authorization, directory: &Path, record: &mut Value,
) -> TestResult<()> {
    if !auth.fullscreen { return Ok(()); }
    require(record["restoreRequired"] == false, "原配置尚未恢复，不能进入全屏生命周期扩展")?;
    let project = record["projectId"].as_str().ok_or("隔离项目缺失")?.to_string();
    let id = record["screenId"].as_str().ok_or("隔离资产缺失")?.to_string();
    let device = AdbDevice::new(AndroidTools::discover()?);
    let expected = if auth.mode == "lifecycle" {record["acceptedConfigSha256"].clone()} else {record["allConfigSha256"].clone()};
    record["uiRestoreRequired"] = json!(true);
    phase(directory, record, "fullscreen-settings-return")?;
    let result: TestResult<()> = async {
        settings_foreground(&device, auth).await?;
        start_main(&device, auth).await?;
        verify_fullscreen(&device, auth, directory, "settings-return").await?;
        require(sha(read_one(state, &project, &id).await?.config.as_ref().unwrap()) == expected,
            "切换界面后普通配置改变")?;
        phase(directory, record, "fullscreen-force-stop-cold-read")?;
        settings_foreground(&device, auth).await?;
        let before_stop = runtime(&device, &auth.ip).await?;
        let before_stop_log = activity_manager_warnings(&device, &auth.ip).await?;
        device.shell(&auth.ip, &["am", "force-stop", "chat.xiaoxin.app"], CancellationToken::new()).await?;
        tokio::time::sleep(Duration::from_millis(750)).await;
        // 先只读系统状态分类。Owner 受保护包可能被系统拒绝强停，不能仅凭命令退出码
        // 当作进程重启，更不能把仍存在的主界面算作冷 Provider；不修改Owner/HOME造条件。
        let activities = device.shell(&auth.ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
        if fullscreen::main_activity_exists(&activities) {
            let resolved = device.shell(&auth.ip, &["cmd", "package", "resolve-activity", "--brief", "--user", "0",
                "-a", "android.intent.action.MAIN", "-c", "android.intent.category.HOME", "-c", "android.intent.category.DEFAULT"], CancellationToken::new()).await?;
            let after_stop = runtime(&device, &auth.ip).await?;
            let relaunched = fullscreen::system_home_relaunch_confirmed(&activities, &resolved,
                (&before_stop.0, &before_stop.1), (&after_stop.0, &after_stop.1));
            let after_stop_log = activity_manager_warnings(&device, &auth.ip).await?;
            let refusals = fullscreen::new_protected_package_refusals(&before_stop_log, &after_stop_log);
            let owner = device_owner::read(&device, &auth.ip).await;
            let protected = fullscreen::protected_force_stop_refusal_confirmed(owner.state == "self",
                (&before_stop.0, &before_stop.1), (&after_stop.0, &after_stop.1), &refusals);
            record["postStopPreparation"] = json!({"scenario":if protected {"protected-package-stop-not-applicable"}
                else if relaunched {"system-home-relaunch"} else {"stop-outcome-unconfirmed"},"confirmed":protected || relaunched,
                "coldReadPerformed":false,"applicationRestartPerformed":relaunched,
                "stopStepApplicable":!protected,"beforePid":before_stop.0,"afterPid":after_stop.0,
                "beforeBootId":before_stop.1,"afterBootId":after_stop.1,"mainResumed":fullscreen::main_is_resumed(&activities),
                "ownerState":owner.state,"newProtectedStopRefusals":refusals,
                "resolvedHome":resolved.lines().map(str::trim).filter(|line|line.starts_with("chat.xiaoxin.app/")).collect::<Vec<_>>()});
            persist(directory, record)?;
            require(protected || relaunched, "force-stop后主界面仍存在，但未确认本次受保护包拒停或实际HOME重新启动；未执行冷读取或掩盖失败")?;
            if protected {
                // 仅回到尚未停止的现有Activity；此分支不宣称验证了冷读或应用重启。
                phase(directory, record, "fullscreen-return-after-protected-stop")?;
                start_main(&device, auth).await?;
                require(runtime(&device, &auth.ip).await? == before_stop,
                    "受保护包拒停后返回界面却改变了进程，不能判为既有页面恢复")?;
            }
        } else {
            record["postStopPreparation"] = json!({"scenario":"cold-provider","coldReadPerformed":true,
                "applicationRestartPerformed":false,"stopStepApplicable":true,
                "beforePid":before_stop.0,"beforeBootId":before_stop.1,"mainActivityExists":false});
            persist(directory, record)?;
            // 严格保留读前和读后无Activity/语音的断言，后续被拉起不能转为HOME场景通过。
            verify_cold_read(state, &device, auth, &project, &id, &expected, directory).await?;
        }
        let protected_stop = record["postStopPreparation"]["scenario"] == "protected-package-stop-not-applicable";
        phase(directory, record, if protected_stop {"fullscreen-existing-activity-return"} else {"fullscreen-restart"})?;
        if record["postStopPreparation"]["scenario"] == "cold-provider" {
            start_main(&device, auth).await?;
            record["postStopPreparation"]["applicationRestartPerformed"] = json!(true);
        }
        verify_fullscreen(&device, auth, directory, if protected_stop {"protected-stop-existing-activity"} else {"restart-unlocked"}).await?;
        verify_lock(&device, &auth.ip, false, directory).await?;
        require(sha(read_one(state, &project, &id).await?.config.as_ref().unwrap()) == expected,
            "生命周期步骤后普通配置改变")?;
        Ok(())
    }.await;
    if result.is_ok() {
        record["uiRestoreRequired"] = json!(false);
        record["fullscreenLifecyclePassed"] = json!(true);
        persist(directory, record)?;
    } else {
        // 数据已恢复；仅尝试恢复本轮离开的页面，保留原测试失败，绝不重放配置提交。
        record["uiRestoreRequired"] = json!(!(start_main(&device, auth).await.is_ok()
            && verify_fullscreen(&device, auth, directory, "failure-return").await.is_ok()));
        persist(directory, record)?;
    }
    result
}

fn lifecycle_record(source: &Value, auth: &Authorization) -> TestResult<Value> {
    require(auth.mode == "lifecycle" && auth.fullscreen && auth.case.is_some(), "缺少生命周期补验门禁")?;
    require(source["target"] == auth.ip && source["serialSha256"] == sha(&json!(auth.serial))
        && source["mode"] == "roundtrip" && source["fullscreenRegression"] == true
        && source["restoreRequired"] == false && source["restoredEffectiveState"] == true
        && source["uiRestoreRequired"] != true,
        "原用例身份、全屏范围或恢复结果不符合生命周期补验条件")?;
    require(source["allConfigSha256"].as_str().is_some_and(is_lowercase_sha256)
        && source["homeEnabled"].is_boolean() && source["projectId"].is_string() && source["screenId"].is_string(),
        "原用例缺少完整基线，不创建或猜测补验目标")?;
    require(auth.lifecycle_config_sha256.as_deref().is_none_or(is_lowercase_sha256),
        "显式接受的补验配置SHA256格式无效")?;
    // 仅使用调用者明确提供且经授权确认的基线，不从设备即时取值自动替换。
    let accepted = auth.lifecycle_config_sha256.as_deref().unwrap_or(source["allConfigSha256"].as_str().unwrap());
    let mut record = json!({"mode":"lifecycle","sourceCase":auth.case,"sourceRestoreStateSha256":sha(source),
        "sourcePassed":source["passed"],"sourceFailurePhase":source["failedPhase"],
        "oldConfigSha256":source["allConfigSha256"],"acceptedConfigSha256":accepted,
        "explicitConfigBaselineOverride":auth.lifecycle_config_sha256.is_some(),
        "fullscreenRegression":true,"restoreRequired":false,"passed":false,"phase":"lifecycle-created"});
    for key in ["target","serialSha256","projectId","screenId","allConfigSha256","homeEnabled"] {
        record[key] = source[key].clone();
    }
    Ok(record)
}

async fn run_lifecycle_only(state: &FormalAppState, auth: &Authorization, directory: &Path, record: &mut Value) -> TestResult<()> {
    phase(directory, record, "lifecycle-verify-restored-baseline")?;
    let project = record["projectId"].as_str().ok_or("原隔离项目缺失")?.to_string();
    let id = record["screenId"].as_str().ok_or("原隔离屏缺失")?.to_string();
    let asset = ScreenRepository::new(state.local_store.pool().clone()).asset(&project, &id).await?;
    require(asset.source == "local" && asset.fields.ip == auth.ip, "补验目标不是原本机隔离资产")?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    for task in state.task_repository.list_recent(&project, 100).await? {
        require(task.state.is_terminal() && !state.job_supervisor.contains(&task.id).await
            && !state.task_repository.results_protected(&task.id).await?,
            "原隔离项目仍有未确认任务，不进入生命周期补验")?;
    }
    let before = read_one(state, &project, &id).await?;
    require(sha(before.config.as_ref().unwrap()) == record["acceptedConfigSha256"]
        && before.home.as_ref().unwrap().enabled == record["homeEnabled"].as_bool()
        && before.kiosk.as_ref().unwrap().enabled == Some(false)
        && before.kiosk.as_ref().unwrap().active == Some(false)
        && before.kiosk.as_ref().unwrap().lock_task_mode.as_deref() == Some("none"),
        "当前配置、主屏或锁定不是原用例已恢复状态，不执行补验")?;
    let configured = before.kiosk.as_ref().unwrap().configured;
    record["originalConfiguredForLifecycle"] = json!(configured);
    persist(directory, record)?;
    fullscreen_lifecycle(state, auth, directory, record).await?;
    let after = read_one(state, &project, &id).await?;
    require(sha(after.config.as_ref().unwrap()) == record["acceptedConfigSha256"]
        && after.home.as_ref().unwrap().enabled == before.home.as_ref().unwrap().enabled
        && after.kiosk.as_ref().unwrap().configured == configured
        && after.kiosk.as_ref().unwrap().enabled == Some(false),
        "生命周期补验后原配置/主屏/锁定键状态发生变化")?;
    record["passed"] = json!(true);
    phase(directory, record, "lifecycle-passed")
}
async fn unknown_read_only(
    state: &FormalAppState,
    auth: &Authorization,
    project: &str,
    id: &str,
    task: &str,
    directory: &Path,
) -> TestResult<()> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    let before = read_one(state, project, id).await?;
    let before_runtime = runtime(&device, &auth.ip).await?;
    let mut result = task_data::read_results(state.local_store.pool(), project, task)
        .await?
        .targets
        .remove(id)
        .ok_or("结果缺失")?;
    // 只修改本次UUID隔离库中的结果证据，模拟回包丢失；设备上不制造故障。
    result.device = ResultState::Unknown;
    result.evidence["phase"] = json!("config_kiosk_apply_requested");
    result.evidence["owner"]["ensure"] = json!("unknown");
    result.evidence["owner"]["readback"] = json!("unknown");
    result.evidence["kiosk"]["save"] = json!("unknown");
    result.evidence["kiosk"]["apply"] = json!("unknown");
    result.evidence["kiosk"]["readback"] = json!("unknown");
    state
        .task_repository
        .protect_results(task, "隔离测试模拟响应丢失，只核实既有结果")
        .await?;
    task_data::save_target(state.local_store.pool(), project, task, &result).await?;
    tasks::verify(state, project, task).await?;
    let verified = task_data::read_results(state.local_store.pool(), project, task)
        .await?
        .targets
        .remove(id)
        .ok_or("核实结果缺失")?;
    let after = read_one(state, project, id).await?;
    require(
        verified.device == ResultState::Succeeded
            && verified.evidence["kiosk"]["save"] == "verified"
            && verified.evidence["kiosk"]["apply"] == "verified"
            && verified.evidence["owner"]["readback"] == "succeeded"
            && before.config == after.config
            && runtime(&device, &auth.ip).await? == before_runtime,
        "未知结果核实未成功或改变了已确认配置/进程",
    )?;
    verify_lock(&device, &auth.ip, true, directory).await
}
async fn restore(
    state: &FormalAppState,
    auth: &Authorization,
    directory: &Path,
    record: &mut Value,
) -> TestResult<()> {
    phase(directory, record, "restore-read-and-settle")?;
    let project = record["projectId"]
        .as_str()
        .ok_or("恢复项目缺失")?
        .to_string();
    let id = record["screenId"]
        .as_str()
        .ok_or("恢复资产缺失")?
        .to_string();
    let asset = ScreenRepository::new(state.local_store.pool().clone())
        .asset(&project, &id)
        .await?;
    require(
        asset.source == "local" && asset.fields.ip == auth.ip && record["target"] == auth.ip,
        "恢复目标不是本次本机隔离资产",
    )?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    settle(state, &project, &id).await?;
    let current = read_one(state, &project, &id).await?;
    let config = current.config.as_ref().unwrap();
    require(
        sha(&without_test_fields(config)) == record["otherFieldsSha256"],
        "非测试配置已变化，停止覆盖并保留恢复责任",
    )?;
    let mut patch = controls(false);
    for key in [NAME, AUDIO] {
        let original = &record["originalValues"][key];
        let actual = field_value(config, key).ok_or("恢复当前值缺失")?;
        require(
            values_equal(original, actual) || values_equal(&record["attemptedValues"][key], actual),
            "测试字段被外部改动，未覆盖",
        )?;
        if !values_equal(original, actual) {
            set_value(&mut patch, key, original);
        }
    }
    let original_home = record["homeEnabled"].as_bool().ok_or("恢复主屏值缺失")?;
    require(
        current.home.as_ref().unwrap().enabled == Some(original_home)
            || current.home.as_ref().unwrap().enabled == Some(true),
        "主屏状态不是原值或本轮设置值，未覆盖",
    )?;
    patch.home_enabled = Some(original_home);
    phase(directory, record, "restore-formal-task")?;
    submit(state, auth, &project, &id, patch).await?;
    let after = read_one(state, &project, &id).await?;
    require(
        sha(after.config.as_ref().unwrap()) == record["allConfigSha256"]
            && after.home.as_ref().unwrap().enabled == record["homeEnabled"].as_bool(),
        "公开配置或主屏未恢复",
    )?;
    verify_lock(&device, &auth.ip, false, directory).await?;
    record["restoreRequired"] = json!(false);
    record["restoredEffectiveState"] = json!(true);
    record["nativeMissingKeyRestoreRequired"] = json!(record["originalConfigured"] == false);
    phase(directory, record, "restored")?;
    verify_fullscreen(&device, auth, directory, "restored-unlocked").await
}
async fn run_case(
    state: &FormalAppState,
    auth: &Authorization,
    screens: &[support::LiveScreen],
    directory: &Path,
    record: &mut Value,
) -> TestResult<()> {
    phase(directory, record, "discover-adb")?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    phase(directory, record, "identity-and-owner")?;
    identity(&device, auth).await?;
    record["identity"] = json!({"connected":true,"serialMatched":true,"ownerState":"self"});
    phase(directory, record, "capture-runtime")?;
    capture_runtime(&device, &auth.ip, directory).await?;
    let target = screens
        .iter()
        .find(|screen| screen.ip == auth.ip)
        .ok_or("测试说明目标缺失")?;
    phase(directory, record, "create-local-project")?;
    let project = Stage75Adapter::new(state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    record["projectId"] = json!(project);
    phase(directory, record, "create-local-screen")?;
    let id = repo
        .save_local(
            &project,
            &ScreenFields {
                name: "Owner锁定隔离回归".into(),
                ip: auth.ip.clone(),
                size: target.size.clone(),
                mac: target.mac.clone(),
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let asset = repo.asset(&project, &id).await?;
    record["screenId"] = json!(id);
    phase(directory, record, "inspect-and-bind-wired-mac")?;
    let observed = device
        .inspect(&asset, "inspect", CancellationToken::new())
        .await?;
    let actual_mac = observed.observed_mac.as_deref().filter(|mac| valid_mac(mac)).map(normalize_mac);
    let description_matches = actual_mac.as_deref().is_some_and(|mac| mac == normalize_mac(&target.mac));
    record["macIdentity"] = json!({"assetSource":asset.source,"adbAvailable":observed.adb_available,
        "expectedSha256":sha(&json!(normalize_mac(&target.mac))),"observedSha256":actual_mac.as_ref().map(|mac|sha(&json!(mac))),
        "source":observed.mac_source,"matchesDescription":description_matches,
        "errors":observed.errors.iter().map(|error|safe_error(error)).collect::<Vec<_>>(),
        "reason":if description_matches {"说明与实际有线网卡一致"} else {"说明MAC与本次采集不一致或不可用；隔离测试仅按已确认IP及硬件序列号核实本屏，再绑定有效实测有线MAC，不修改说明或正式资料"}});
    persist(directory,record)?;
    require(asset.source == "local" && observed.adb_available && actual_mac.is_some(),
        "inspect.mac: 未取得同一已确认硬件的有效有线MAC，安全详情见macIdentity")?;
    // 测试说明可能登记无线或人为测试MAC；测试仅绑定当前新建的隔离本机记录。
    // 后续正式预检/执行仍按产品既有的有线MAC、资料快照和租约规则检查。
    let mut bound_fields = asset.fields.clone();
    bound_fields.mac = actual_mac.unwrap();
    repo.save_local(&project,&bound_fields,Some(&id),Some(asset.revision)).await?;
    phase(directory, record, "baseline-read")?;
    let before = read_one(state, &project, &id).await?;
    let config = before.config.as_ref().unwrap();
    let old_kiosk = before.kiosk.as_ref().unwrap();
    record["baselineKiosk"] = serde_json::to_value(old_kiosk)?;
    record["allConfigSha256"] = json!(sha(config));
    if auth.mode == "read-only" {
        record["passed"] = json!(true);
        return phase(directory, record, "read-only-passed");
    }
    require(
        old_kiosk.enabled == Some(false) && old_kiosk.lock_task_mode.as_deref() == Some("none"),
        "本轮仅从原目标false且系统未锁定的屏开始，未进行写入",
    )?;
    let schema = ConfigSchema::from_capabilities(before.capabilities.as_ref().unwrap())?
        .ok_or("缺少字段定义")?;
    let definition = schema.field(AUDIO).ok_or("缺少音频字段")?;
    require(
        schema.fields.len() == 23
            && definition.writable
            && definition.accepts(field_value(config, AUDIO).ok_or("音频原值缺失")?),
        "字段定义或原音频值不符合本轮可恢复范围",
    )?;
    let score = field_value(config, AUDIO)
        .and_then(Value::as_f64)
        .ok_or("音频原值缺失")?;
    let candidate = [score + 0.1, score - 0.1]
        .into_iter()
        .map(|value| json!((value * 10.0).round() / 10.0))
        .find(|value| {
            definition.accepts(value) && !values_equal(value, field_value(config, AUDIO).unwrap())
        })
        .ok_or("没有可恢复音频测试值")?;
    record["originalConfigured"] = json!(old_kiosk.configured.unwrap());
    record["homeEnabled"] = json!(before.home.as_ref().unwrap().enabled.unwrap());
    record["originalValues"] = json!({NAME:field_value(config,NAME).ok_or("名称值缺失")?, AUDIO:field_value(config,AUDIO).unwrap()});
    record["attemptedValues"] = json!({NAME:format!("锁定回归-{}", &uuid::Uuid::now_v7().to_string()[..8]), AUDIO:candidate});
    record["otherFieldsSha256"] = json!(sha(&without_test_fields(config)));
    record["restoreRequired"] = json!(true);
    persist(directory, record)?;

    phase(directory, record, "owner-idempotent")?;
    let (_, owner) = submit(
        state,
        auth,
        &project,
        &id,
        AppConfigPatch {
            ensure_device_owner: Some(true),
            ..Default::default()
        },
    )
    .await?;
    require(
        owner.evidence["owner"]["ensure"] == "unchanged",
        "已有Owner未被识别为幂等",
    )?;
    phase(directory, record, "lock-on")?;
    submit(state, auth, &project, &id, controls(true)).await?;
    verify_lock(&device, &auth.ip, true, directory).await?;
    verify_fullscreen(&device, auth, directory, "locked").await?;
    phase(directory, record, "same-lock-value")?;
    let old_runtime = runtime(&device, &auth.ip).await?;
    let (same_task, same) = submit(state, auth, &project, &id, controls(true)).await?;
    require(
        same.evidence["kiosk"]["save"] == "unchanged"
            && same.evidence["kiosk"]["apply"] == "unchanged"
            && runtime(&device, &auth.ip).await? == old_runtime,
        "同值重复写入/应用或改变进程",
    )?;
    phase(directory, record, "unknown-result-read-only-verification")?;
    unknown_read_only(state, auth, &project, &id, &same_task, directory).await?;

    phase(directory, record, "ordinary-config-while-locked")?;
    let mut rename = AppConfigPatch::default();
    set_value(&mut rename, NAME, &record["attemptedValues"][NAME]);
    let (_, result) = submit(state, auth, &project, &id, rename).await?;
    require(
        result.evidence["config"]["save"] == "saved"
            && result.evidence["config"]["restart"] == "not_required"
            && runtime(&device, &auth.ip).await? == old_runtime,
        "锁定中普通配置保存或免重启语义不符合预期",
    )?;
    verify_lock(&device, &auth.ip, true, directory).await?;

    phase(directory, record, "owner-home-locked-preflight-gate")?;
    let mut combined = controls(true);
    combined.home_enabled = Some(true);
    set_value(&mut combined, AUDIO, &candidate);
    let gate_before = read_one(state, &project, &id).await?;
    require(
        combined.restart_required_with_capabilities(
            gate_before.config.as_ref().unwrap(),
            gate_before.capabilities.as_ref().unwrap(),
        ),
        "组合测试未触发真实重启规则",
    )?;
    let rejected = maintenance::preflight_with_config(
        state,
        &project,
        input(&id),
        Some(BTreeMap::from([(id.clone(), combined.clone())])),
    )
    .await?;
    require(
        rejected.items.len() == 1
            && rejected.items[0].state != "ready"
            && rejected.items[0].reason.contains("锁定"),
        "锁定中需要HOME重新激活的组合未被预检拒绝",
    )?;
    require(
        read_one(state, &project, &id).await?.config == gate_before.config
            && runtime(&device, &auth.ip).await? == old_runtime,
        "被拒绝预检改变了配置或进程",
    )?;

    phase(
        directory,
        record,
        "explicit-unlock-before-combined-operation",
    )?;
    let mut unlock = controls(false);
    unlock.home_enabled = Some(true);
    submit(state, auth, &project, &id, unlock).await?;
    verify_lock(&device, &auth.ip, false, directory).await?;
    verify_fullscreen(&device, auth, directory, "unlocked").await?;
    phase(directory, record, "restart-home-then-lock")?;
    let old_runtime = runtime(&device, &auth.ip).await?;
    let (_, combined_result) = submit(state, auth, &project, &id, combined).await?;
    let new_runtime = runtime(&device, &auth.ip).await?;
    require(
        old_runtime.0 != new_runtime.0
            && old_runtime.1 == new_runtime.1
            && combined_result.evidence["config"]["restart"] == "succeeded"
            && combined_result.evidence["home"]["runtimeReadback"] == "succeeded"
            && combined_result.evidence["kiosk"]["readback"] == "succeeded",
        "未确认重启/HOME运行态/最终锁定完整组合",
    )?;
    verify_lock(&device, &auth.ip, true, directory).await?;
    verify_fullscreen(&device, auth, directory, "restart-locked").await?;
    restore(state, auth, directory, record).await?;
    fullscreen_lifecycle(state, auth, directory, record).await?;
    record["passed"] = json!(true);
    phase(directory, record, "roundtrip-passed")
}
async fn seed_gui(screens: &[support::LiveScreen]) -> TestResult<()> {
    let base = evidence_root()?;
    let directory = base.join("data");
    require(!directory.exists(), "GUI隔离目录已存在，不覆盖或复用")?;
    require(
        TARGETS.iter().all(|(ip, _, size)| {
            screens
                .iter()
                .any(|screen| screen.ip == *ip && screen.size == *size)
        }),
        "当前测试说明缺少指定三屏",
    )?;
    std::fs::create_dir(&directory)?;
    let state = support::state_at(&directory, false).await;
    let outcome: TestResult<Value> = async {
        let mut input = support::local_input(); input.name = "Owner与锁定隔离回归".into();
        let project = Stage75Adapter::new(&state).create_project(input).await?.project.id;
        let repo = ScreenRepository::new(state.local_store.pool().clone()); let mut records = Vec::new();
        for (ip, _, size) in TARGETS {
            let source = screens.iter().find(|screen| screen.ip == ip).ok_or("种子目标缺失")?;
            let id = repo.save_local(&project, &ScreenFields { name: source.name.clone(), ip: ip.into(), size: size.into(),
                ..Default::default() }, None, None).await?;
            records.push(json!({"id":id,"name":source.name,"size":size,"ip":ip}));
        }
        Ok(json!({"projectId":project,"projectName":"Owner与锁定隔离回归","screens":records,"deviceConnections":0,"platformConnections":0}))
    }.await;
    support::close(state).await;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("gui-seed-ids.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&outcome?)?)?;
    file.sync_all()?;
    println!("GUI隔离库：{}", directory.display());
    Ok(())
}

#[tokio::test]
#[ignore = "必须显式授权当前三屏IP/序列号和独立写门禁；seed-gui只创建新本机库"]
async fn kiosk_owner_formal_real() -> TestResult<()> {
    let values = ENV_KEYS
        .into_iter()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value)))
        .collect();
    let screens = support::live_screens()?;
    let auth = authorization(&values, &screens)?;
    if auth.mode == "seed-gui" {
        return seed_gui(&screens).await;
    }
    let base = evidence_root()?.join("cases");
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    require(base.starts_with(root()?), "用例目录不在仓库内")?;
    let (directory, state_directory, mut record) = if let Some(case) = &auth.case {
        let source_directory = base.join(case).canonicalize()?;
        require(
            source_directory.parent() == Some(base.as_path()),
            "恢复目录不是直接UUID子目录",
        )?;
        let record: Value =
            serde_json::from_slice(&std::fs::read(source_directory.join("restore-state.json"))?)?;
        if auth.mode == "lifecycle" {
            let supplement = lifecycle_record(&record, &auth)?;
            let supplements = evidence_root()?.join("lifecycle-cases");
            std::fs::create_dir_all(&supplements)?;
            let supplements = supplements.canonicalize()?;
            require(supplements.parent() == Some(evidence_root()?.as_path()), "补验证据不在隔离根目录")?;
            let directory = supplements.join(uuid::Uuid::now_v7().to_string());
            std::fs::create_dir(&directory)?;
            persist(&directory, &supplement)?;
            // 复用原隔离库里的同一目标；原恢复文件、失败字段和其证据文件均不重写。
            (directory, source_directory, supplement)
        } else {
            require(
                record["target"] == auth.ip
                    && record["serialSha256"] == sha(&json!(auth.serial))
                    && record["mode"] == "roundtrip"
                    && record["restoreRequired"] == true,
                "没有对应恢复责任",
            )?;
            require(record["fullscreenRegression"].as_bool().unwrap_or(false) == auth.fullscreen,
                "恢复时的全屏扩展门禁与原用例不一致")?;
            (source_directory.clone(), source_directory, record)
        }
    } else {
        let directory = base.join(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir(&directory)?;
        let record = json!({"target":auth.ip,"serialSha256":sha(&json!(auth.serial)),"mode":auth.mode,
            "restoreRequired":false,"passed":false,"phase":"created","fullscreenRegression":auth.fullscreen});
        persist(&directory, &record)?;
        (directory.clone(), directory, record)
    };
    println!("隔离用例目录：{}", directory.display());
    let state = support::state_at(&state_directory, true).await;
    let outcome = if auth.mode == "restore" {
        restore(&state, &auth, &directory, &mut record).await
    } else if auth.mode == "lifecycle" {
        run_lifecycle_only(&state, &auth, &directory, &mut record).await
    } else {
        run_case(&state, &auth, &screens, &directory, &mut record).await
    };
    if let Err(error) = &outcome {
        record["failedPhase"] = record["phase"].clone();
        record["failureReason"] = json!(safe_error(error));
        eprintln!("回归阶段失败：{}；原因：{}",record["failedPhase"],record["failureReason"]);
        record["passed"] = json!(false);
        let _ = persist(&directory, &record);
        if auth.mode == "roundtrip" && record["restoreRequired"] == true {
            record["automaticRestorePassed"] =
                json!(restore(&state, &auth, &directory, &mut record)
                    .await
                    .is_ok());
            let _ = persist(&directory, &record);
        }
    }
    support::close(state).await;
    if outcome.is_err() {
        return Err(format!(
            "回归未通过；阶段={}；原因={}；恢复责任={}。原始证据保留，未知结果不重放。",
            record["failedPhase"].as_str().unwrap_or("unknown"),
            record["failureReason"].as_str().unwrap_or("unknown"),
            record["restoreRequired"]
        )
        .into());
    }
    Ok(())
}

#[test]
fn kiosk_gate_requires_current_description_serial_and_separate_write_authorization() {
    let screens = support::parse_live_screens("10寸屏：62-91-4C-93-81-57 192.168.3.42\n4寸屏：28-2D-06-15-59-AC 192.168.3.74\n4寸屏2：40-FD-F3-01-AE-2C 192.168.3.82").unwrap();
    let mut env = BTreeMap::from([
        (ENV_KEYS[0].into(), "YES".into()),
        (ENV_KEYS[1].into(), TARGETS[0].0.into()),
        (ENV_KEYS[2].into(), TARGETS[0].1.into()),
        (ENV_KEYS[3].into(), "read-only".into()),
    ]);
    assert!(authorization(&env, &screens).is_ok());
    env.insert(ENV_KEYS[3].into(), "roundtrip".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[4].into(), "YES".into());
    assert!(authorization(&env, &screens).is_ok());
    env.insert(ENV_KEYS[1].into(), "192.168.3.106".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[1].into(), TARGETS[0].0.into());
    env.insert(ENV_KEYS[2].into(), TARGETS[1].1.into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[2].into(), TARGETS[0].1.into());
    assert!(authorization(&env, &[]).is_err());
    env.insert(ENV_KEYS[3].into(), "restore".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[5].into(), "../../production".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[5].into(), uuid::Uuid::now_v7().to_string());
    assert!(authorization(&env, &screens).is_ok());
    env.insert(ENV_KEYS[0].into(), "NO".into());
    assert!(authorization(&env, &screens).is_err());
}
#[test]
fn kiosk_gui_seed_is_local_only_and_restore_evidence_rejects_sensitive_urls() {
    let mut env = BTreeMap::from([
        (ENV_KEYS[0].into(), "YES".into()),
        (ENV_KEYS[3].into(), "seed-gui".into()),
    ]);
    assert!(authorization(&env, &[]).is_ok());
    env.insert(ENV_KEYS[1].into(), TARGETS[0].0.into());
    assert!(authorization(&env, &[]).is_err());
    assert!(safe_for_formal_evidence(
        &json!({"h5":"https://example.org/page"})
    ));
    for text in [
        "https://u:p@example.org",
        "https://example.org/?token=secret",
        "https://example.org/#secret",
    ] {
        assert!(!safe_for_formal_evidence(&json!({"h5":text})));
    }
    let error = safe_error(&"identity.connect: failed https://u:p@example.org/?token=raw password=raw-secret");
    assert!(error.contains("identity.connect: failed"));
    assert!(!error.contains("example.org") && !error.contains("raw"));
}

#[test]
fn fullscreen_extension_requires_explicit_yes_and_writable_roundtrip_mode() {
    let screens = support::parse_live_screens("10寸屏：62-91-4C-93-81-57 192.168.3.42").unwrap();
    let mut env = BTreeMap::from([
        (ENV_KEYS[0].into(), "YES".into()),
        (ENV_KEYS[1].into(), TARGETS[0].0.into()),
        (ENV_KEYS[2].into(), TARGETS[0].1.into()),
        (ENV_KEYS[3].into(), "roundtrip".into()),
        (ENV_KEYS[4].into(), "YES".into()),
    ]);
    assert!(!authorization(&env, &screens).unwrap().fullscreen);
    env.insert(ENV_KEYS[6].into(), "YES".into());
    assert!(authorization(&env, &screens).unwrap().fullscreen);
    env.insert(ENV_KEYS[3].into(), "read-only".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[3].into(), "roundtrip".into());
    env.insert(ENV_KEYS[6].into(), "true".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[6].into(), "YES".into());
    env.insert(ENV_KEYS[3].into(), "lifecycle".into());
    assert!(authorization(&env, &screens).is_err());
    env.insert(ENV_KEYS[5].into(), uuid::Uuid::now_v7().to_string());
    assert!(authorization(&env, &screens).is_ok());
    env.remove(ENV_KEYS[6]);
    assert!(authorization(&env, &screens).is_err());
}

#[test]
fn lifecycle_supplement_requires_restored_matching_source_and_preserves_its_failure() {
    let auth = Authorization { ip:TARGETS[0].0.into(), serial:TARGETS[0].1.into(),
        mode:"lifecycle".into(), case:Some(uuid::Uuid::now_v7().to_string()), fullscreen:true, lifecycle_config_sha256:None };
    let source = json!({"target":auth.ip,"serialSha256":sha(&json!(auth.serial)),"mode":"roundtrip",
        "fullscreenRegression":true,"restoreRequired":false,"restoredEffectiveState":true,"uiRestoreRequired":false,
        "allConfigSha256":sha(&json!({})),"homeEnabled":true,"projectId":"isolated-project","screenId":"isolated-screen",
        "passed":false,"failedPhase":"fullscreen-force-stop-cold-read"});
    let snapshot = source.clone();
    let result = lifecycle_record(&source,&auth).unwrap();
    assert_eq!(source,snapshot);
    assert_eq!(result["mode"],"lifecycle");
    assert_eq!(result["sourcePassed"],false);
    assert_eq!(result["sourceFailurePhase"],"fullscreen-force-stop-cold-read");
    assert_eq!(result["sourceRestoreStateSha256"],sha(&source));
    assert_eq!(result["oldConfigSha256"],source["allConfigSha256"]);
    assert_eq!(result["acceptedConfigSha256"],source["allConfigSha256"]);
    assert_eq!(result["explicitConfigBaselineOverride"],false);
    for (key,value) in [("restoreRequired",json!(true)),("restoredEffectiveState",json!(false)),
        ("uiRestoreRequired",json!(true)),("target",json!(TARGETS[1].0)),("allConfigSha256",json!("missing"))] {
        let mut invalid = source.clone(); invalid[key]=value;
        assert!(lifecycle_record(&invalid,&auth).is_err());
    }
}

#[test]
fn lifecycle_config_override_is_explicit_lowercase_sha256_and_rejected_in_other_modes() {
    let screens = support::parse_live_screens("10寸屏：62-91-4C-93-81-57 192.168.3.42").unwrap();
    let hash = "ea3a18b0e3d013b186c03d1042053b07c38205a6ea735d2d4c35b8e55c3f1720";
    let mut env = BTreeMap::from([
        (ENV_KEYS[0].into(),"YES".into()), (ENV_KEYS[1].into(),TARGETS[0].0.into()),
        (ENV_KEYS[2].into(),TARGETS[0].1.into()), (ENV_KEYS[3].into(),"lifecycle".into()),
        (ENV_KEYS[4].into(),"YES".into()), (ENV_KEYS[5].into(),uuid::Uuid::now_v7().to_string()),
        (ENV_KEYS[6].into(),"YES".into()), (ENV_KEYS[7].into(),hash.into()),
    ]);
    assert_eq!(authorization(&env,&screens).unwrap().lifecycle_config_sha256.as_deref(),Some(hash));
    for invalid in [String::new(),hash.to_uppercase(),"a".repeat(63),"g".repeat(64),format!(" {hash}")] {
        env.insert(ENV_KEYS[7].into(),invalid);
        assert!(authorization(&env,&screens).is_err());
    }
    env.insert(ENV_KEYS[7].into(),hash.into());
    env.insert(ENV_KEYS[3].into(),"restore".into());
    assert!(authorization(&env,&screens).is_err());
    env.remove(ENV_KEYS[5]);
    for mode in ["roundtrip","read-only","seed-gui"] {
        env.insert(ENV_KEYS[3].into(),mode.into());
        assert!(authorization(&env,&screens).is_err());
    }
}

#[test]
fn accepted_lifecycle_hash_keeps_source_and_old_hash_without_modifying_source_case() {
    let accepted = "ea3a18b0e3d013b186c03d1042053b07c38205a6ea735d2d4c35b8e55c3f1720";
    let auth = Authorization {ip:TARGETS[0].0.into(),serial:TARGETS[0].1.into(),mode:"lifecycle".into(),
        case:Some(uuid::Uuid::now_v7().to_string()),fullscreen:true,lifecycle_config_sha256:Some(accepted.into())};
    let source = json!({"target":auth.ip,"serialSha256":sha(&json!(auth.serial)),"mode":"roundtrip",
        "fullscreenRegression":true,"restoreRequired":false,"restoredEffectiveState":true,"uiRestoreRequired":false,
        "allConfigSha256":sha(&json!({})),"homeEnabled":true,"projectId":"isolated-project","screenId":"isolated-screen",
        "passed":false,"failedPhase":"fullscreen-force-stop-cold-read"});
    let original = source.clone();
    let record = lifecycle_record(&source,&auth).unwrap();
    assert_eq!(source,original);
    assert_eq!(record["sourceRestoreStateSha256"],sha(&source));
    assert_eq!(record["allConfigSha256"],source["allConfigSha256"]);
    assert_eq!(record["oldConfigSha256"],source["allConfigSha256"]);
    assert_eq!(record["acceptedConfigSha256"],accepted);
    assert_eq!(record["explicitConfigBaselineOverride"],true);
    let mut unresolved = source.clone(); unresolved["restoreRequired"]=json!(true);
    assert!(lifecycle_record(&unresolved,&auth).is_err());
}

#[test]
fn failed_lock_evidence_retains_runtime_and_power_without_intent_payloads() {
    let activity="mLockTaskModeState=LOCKED\nmResumedActivity: ActivityRecord{abc u0 chat.xiaoxin.app/.MainActivity t42}\nmSleeping=true\nTaskRecord{abc} Intent{https://example.org/?token=private}\nprivate unrelated text";
    assert_eq!(runtime_lines(activity,false).len(),3);
    let power=runtime_lines("mWakefulness=Asleep\nmLastSleepTime=123\nDisplay Power: state=OFF\nWakeLock: package=private\n",true);
    assert_eq!(power,vec!["mWakefulness=Asleep","mLastSleepTime=123","Display Power: state=OFF"]);
}
