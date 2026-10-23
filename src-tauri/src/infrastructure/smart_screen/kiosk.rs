//! 锁定策略保存、运行应用及系统回读分离；查询不启动界面或重放写入。
use super::device::{AdbDevice, DeviceCommandPort, XIAOXIN_PACKAGE};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::app_config::AppKioskRead;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KioskStatus {
    pub protocol_version: u32,
    pub package_name: String,
    pub device_owner: bool,
    pub configured: bool,
    pub enabled: bool,
    pub active: bool,
    pub activity_ready: bool,
    pub lock_task_mode: String,
    pub storage_confirmed: bool,
}

fn error(response: &Value) -> AppError {
    AppError::Conflict(match response["code"].as_str() {
        Some("NOT_DEVICE_OWNER") => "小新尚未成为设备所有者，未应用锁定",
        Some("STORAGE_UNCONFIRMED") => "锁定设置保存结果未确认，请只读核实",
        Some("FORBIDDEN") => "设备拒绝当前锁定管理身份",
        Some("UNSUPPORTED") => "当前小新不支持应用锁定管理，请先升级配套应用",
        _ => "锁定操作未确认，请重新读取；不会自动重复保存或应用",
    }.into())
}

fn state(response: &Value) -> AppResult<KioskStatus> {
    if response["status"] == "failed" { return Err(error(response)) }
    let status: KioskStatus = serde_json::from_value(response["data"].clone())
        .map_err(|_| AppError::Conflict("小新锁定状态不完整，请升级配套应用或重新读取".into()))?;
    if status.protocol_version != 1 || status.package_name != XIAOXIN_PACKAGE || !status.storage_confirmed
        || !["none", "locked", "pinned"].contains(&status.lock_task_mode.as_str())
        || (status.active && (!status.activity_ready || status.lock_task_mode != "locked")) {
        return Err(AppError::Conflict("小新锁定状态未确认，不能推断为已锁定或已解锁".into()));
    }
    Ok(status)
}

pub async fn query<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<KioskStatus> {
    let response = super::app_config::call(device, ip, "readKiosk", None).await?;
    if response["status"] != "ok" { return Err(error(&response)) }
    state(&response)
}

pub async fn read<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppKioskRead {
    project_read(query(device,ip).await)
}

pub(super) fn read_response(response:&Value)->AppKioskRead {
    project_read(if response["status"]=="ok" {state(response)} else {Err(error(response))})
}

fn project_read(value:AppResult<KioskStatus>)->AppKioskRead {
    match value {
        Ok(status) => AppKioskRead { supported: true, configured: Some(status.configured), enabled: Some(status.enabled),
            active: Some(status.active), activity_ready: Some(status.activity_ready), lock_task_mode: Some(status.lock_task_mode),
            message: "已读取锁定设置和运行状态；保存与实际锁定分别核验".into() },
        Err(error) => AppKioskRead { message: error.to_string(), ..Default::default() },
    }
}

pub async fn save<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, enabled: bool) -> AppResult<String> {
    let request_id = uuid::Uuid::now_v7().to_string();
    let response = super::app_config::call(device, ip, "saveKiosk", Some(&json!({"protocolVersion":1,"requestId":request_id,"enabled":enabled}))).await?;
    if !matches!(response["status"].as_str(), Some("saved" | "unchanged")) || response["requestId"] != request_id {
        return Err(error(&response));
    }
    let saved = state(&response)?;
    if !saved.device_owner || !saved.configured || saved.enabled != enabled { return Err(error(&response)) }
    Ok(response["status"].as_str().unwrap().into())
}

pub async fn apply<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, enabled: bool) -> AppResult<String> {
    let response = super::app_config::call(device, ip, "applyKiosk", None).await?;
    if !matches!(response["status"].as_str(), Some("applied" | "pending")) { return Err(error(&response)) }
    let current = state(&response)?;
    if !current.device_owner || !current.configured || current.enabled != enabled { return Err(error(&response)) }
    if response["status"] == "applied" && response["data"]["applied"] != true { return Err(error(&response)) }
    Ok(response["status"].as_str().unwrap().into())
}

/// 只发一次非切换唤醒键；不解除系统锁屏，不切换 HOME，不在核实阶段调用。
pub async fn wake<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, cancel: CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() { return Err(AppError::Cancelled) }
    let output = device.adb(ip, &["shell", "input", "keyevent", "KEYCODE_WAKEUP"], Duration::from_secs(10), cancel).await?;
    if !output.success || !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty() {
        return Err(AppError::Conflict("唤醒命令结果未确认，请核实屏端状态；不会自动重复唤醒或解除锁屏".into()));
    }
    Ok(())
}

/// 独立查询系统任务，防止仅以偏好、冷 Provider 或其他应用的 Lock Task 判成功。
pub fn runtime_matches(text: &str, enabled: bool) -> AppResult<bool> {
    if !text.trim_start().starts_with("ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)") {
        return Err(AppError::Conflict("系统锁定运行状态格式未确认".into()));
    }
    let modes = regex::Regex::new(r"(?m)^\s*mLockTaskModeState\s*[=:]\s*(NONE|LOCKED|PINNED)\b").unwrap()
        .captures_iter(text).map(|c| c[1].to_string()).collect::<std::collections::BTreeSet<_>>();
    if modes.len() != 1 { return Err(AppError::Conflict("系统未返回唯一明确锁定模式，结果待核实".into())) }
    let mode = modes.iter().next().unwrap();
    if !enabled { return Ok(mode == "NONE") }
    if mode != "LOCKED" { return Ok(false) }
    let resumed = regex::Regex::new(r"(?m)^\s*(?:mResumedActivity:|mResumedActivity=|ResumedActivity:|topResumedActivity=)\s*ActivityRecord\{[^\r\n]*\bu0\s+chat\.xiaoxin\.app/(?:\.MainActivity|chat\.xiaoxin\.app\.MainActivity)\s+t(\d+)\}").unwrap()
        .captures(text).map(|c| c[1].to_string());
    let Some((_, tasks)) = text.split_once("mLockTaskModeTasks") else { return Err(AppError::Conflict("系统未返回锁定任务身份，结果待核实".into())) };
    // 8.1 的任务列表在同一行方括号内；10 的任务列表在后续独立行。
    // 严格限制到该列表，不能把后面的普通任务/最近任务误认成锁定任务。
    let mut lines = tasks.lines();
    let first = lines.next().unwrap_or("").trim().trim_start_matches('=').trim();
    let tasks = if first.starts_with('[') {
        first.split_once(']').map(|(list, _)| list.to_string())
            .ok_or_else(|| AppError::Conflict("系统锁定任务列表不完整".into()))?
    } else if first.is_empty() {
        let task_line = regex::Regex::new(r"^(?:#?\d+:?\s+)?(?:TaskRecord|Task)\{").unwrap();
        lines.map(str::trim).take_while(|line| task_line.is_match(line)).collect::<Vec<_>>().join("\n")
    } else {
        return Err(AppError::Conflict("系统锁定任务列表格式未确认".into()));
    };
    Ok(resumed.is_some_and(|id| regex::Regex::new(r"(?:TaskRecord|Task)\{[^}\r\n]*#(\d+)(?:\s|\})").unwrap().captures_iter(&tasks).any(|c| c[1] == id)))
}

pub async fn verify<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, enabled: bool) -> AppResult<(KioskStatus, bool)> {
    let current = query(device, ip).await?;
    let mut runtime = device.shell(ip, &["dumpsys", "activity", "activities"], CancellationToken::new()).await?;
    // Android 8.1 只在完整 activity 转储尾段提供 LockTask；活动身份仍取前面的 activities。
    if !runtime.contains("mLockTaskModeState") {
        runtime.push('\n');
        runtime.push_str(&device.shell(ip, &["dumpsys", "activity"], CancellationToken::new()).await?);
    }
    let matched = current.device_owner && current.configured && current.enabled == enabled
        && current.active == enabled && (if enabled { current.lock_task_mode == "locked" } else { current.lock_task_mode == "none" })
        && runtime_matches(&runtime, enabled)?;
    Ok((current, matched))
}

pub async fn wait_verified<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, enabled: bool) -> AppResult<(KioskStatus, bool)> {
    let mut last = None;
    for attempt in 0..10 {
        let result = verify(device, ip, enabled).await?;
        if result.1 { return Ok(result) }
        last = Some(result);
        if attempt < 9 { tokio::time::sleep(Duration::from_millis(250)).await; }
    }
    Ok(last.expect("read at least once"))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use super::super::device::{AndroidTools, ProcessOutput};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use std::{path::Path, sync::{Arc, Mutex}};

    #[derive(Clone)]
    pub(crate) struct Commands {
        pub status: Value,
        pub runtime: String,
        pub calls: Arc<Mutex<Vec<String>>>,
        pub wrong_request_id: bool,
        pub read_only: bool,
    }
    impl Commands {
        pub fn new(enabled: bool) -> Self {
            Self { status: json!({"protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"deviceOwner":true,
                "configured":true,"enabled":enabled,"active":enabled,"activityReady":enabled,
                "lockTaskMode":if enabled {"locked"} else {"none"},"storageConfirmed":true}),
                runtime: format!("ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)\n mLockTaskModeState={}\n mLockTaskModeTasks=[TaskRecord{{abc #42 A=chat.xiaoxin.app}}]\n mResumedActivity: ActivityRecord{{abc u0 chat.xiaoxin.app/.MainActivity t42}}\n", if enabled {"LOCKED"} else {"NONE"}),
                calls: Arc::default(), wrong_request_id: false, read_only: false }
        }
        pub fn device(&self) -> AdbDevice<Self> {
            AdbDevice::with_commands(AndroidTools { adb: "mock-never-executed".into() }, self.clone())
        }
    }
    impl DeviceCommandPort for Commands {
        async fn execute(&self, _: &Path, args: Vec<String>, _: Duration, _: CancellationToken) -> AppResult<ProcessOutput> {
            let stdout = if let Some(pair) = args.windows(2).find(|pair| pair[0] == "--method") {
                let method = &pair[1];
                self.calls.lock().unwrap().push(method.clone());
                assert!(!self.read_only || method == "readKiosk", "只读核实不得重放保存或应用");
                let response = match method.as_str() {
                    "readKiosk" => json!({"status":"ok","data":self.status}),
                    "saveKiosk" => {
                        let encoded = args.iter().find_map(|arg|arg.strip_prefix("payload:s:")).unwrap();
                        let payload:Value = serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
                        assert_eq!(payload.as_object().unwrap().len(), 3);
                        assert_eq!(payload["protocolVersion"],1);
                        assert!(uuid::Uuid::parse_str(payload["requestId"].as_str().unwrap()).is_ok());
                        assert_eq!(payload["enabled"],self.status["enabled"]);
                        json!({"status":"saved","requestId":if self.wrong_request_id {json!("wrong")} else {payload["requestId"].clone()},"data":self.status})
                    }
                    "applyKiosk" => {
                        assert!(!args.iter().any(|arg|arg.starts_with("payload:s:")));
                        let mut data=self.status.clone(); data["applied"]=json!(true);
                        json!({"status":"applied","data":data})
                    }
                    _ => panic!("unexpected provider call: {method}"),
                };
                format!("Result: Bundle[{{result={}}}]", STANDARD.encode(serde_json::to_vec(&response).unwrap()))
            } else if args.ends_with(&["input".into(),"keyevent".into(),"KEYCODE_WAKEUP".into()]) {
                assert!(!self.read_only, "只读核实不得唤醒");
                self.calls.lock().unwrap().push("wake".into()); String::new()
            } else if args.ends_with(&["dumpsys".into(),"device_policy".into()]) {
                self.calls.lock().unwrap().push("owner-read".into());
                format!("Current Device Policy Manager state:\nDevice Owner:\nadmin=ComponentInfo{{{}}}\nUser ID: 0\nEnabled Device Admins (User 0):",super::super::device_owner::ADMIN)
            } else {
                assert!(args.ends_with(&["dumpsys".into(),"activity".into(),"activities".into()])
                    || args.ends_with(&["dumpsys".into(),"activity".into()]), "不得隐式启动/重启应用或设置设备所有者");
                self.calls.lock().unwrap().push("runtime-read".into()); self.runtime.clone()
            };
            Ok(ProcessOutput { success:true,stdout,stderr:String::new() })
        }
    }
    #[tokio::test]
    async fn save_and_apply_are_separate_and_request_identity_is_required() {
        let commands=Commands::new(true);
        assert_eq!(save(&commands.device(),"192.0.2.1",true).await.unwrap(),"saved");
        assert_eq!(*commands.calls.lock().unwrap(),vec!["saveKiosk"]);
        commands.calls.lock().unwrap().clear();
        assert_eq!(apply(&commands.device(),"192.0.2.1",true).await.unwrap(),"applied");
        assert_eq!(*commands.calls.lock().unwrap(),vec!["applyKiosk"]);
        let mut bad=commands.clone(); bad.wrong_request_id=true;
        assert!(save(&bad.device(),"192.0.2.1",true).await.is_err());
    }
    #[tokio::test]
    async fn explicit_wakeup_sends_one_non_toggle_key_and_cancel_sends_nothing() {
        let commands=Commands::new(false);
        wake(&commands.device(),"192.0.2.1",CancellationToken::new()).await.unwrap();
        assert_eq!(*commands.calls.lock().unwrap(),vec!["wake"]);
        let cancel=CancellationToken::new();cancel.cancel();
        assert!(matches!(wake(&commands.device(),"192.0.2.1",cancel).await,Err(AppError::Cancelled)));
        assert_eq!(*commands.calls.lock().unwrap(),vec!["wake"]);
    }
    #[tokio::test]
    async fn cold_read_and_saved_target_do_not_prove_runtime_or_launch_activity() {
        let mut commands=Commands::new(true); commands.read_only=true;
        commands.status["active"]=json!(false); commands.status["activityReady"]=json!(false);
        commands.status["lockTaskMode"]=json!("none");
        let read=read(&commands.device(),"192.0.2.1").await;
        assert!(read.supported); assert_eq!(read.enabled,Some(true)); assert_eq!(read.active,Some(false));
        assert!(!verify(&commands.device(),"192.0.2.1",true).await.unwrap().1);
        assert_eq!(*commands.calls.lock().unwrap(),vec!["readKiosk","readKiosk","runtime-read"]);
    }
    #[test]
    fn missing_fields_unconfirmed_storage_and_unknown_modes_are_not_valid_states() {
        let commands=Commands::new(true);
        for (key,value) in [("storageConfirmed",json!(false)),("lockTaskMode",json!("unknown")),("packageName",json!("other.app")),("activityReady",json!(false))] {
            let mut data=commands.status.clone();data[key]=value;
            assert!(state(&json!({"status":"ok","data":data})).is_err());
        }
        let mut data=commands.status.clone(); data.as_object_mut().unwrap().remove("configured");
        assert!(state(&json!({"status":"ok","data":data})).is_err());
    }
    #[test]
    fn runtime_requires_locked_mode_and_the_resumed_xiaoxin_task() {
        let base = "ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)\n mLockTaskModeState=LOCKED\n mLockTaskModeTasks=[TaskRecord{abc #42 A=chat.xiaoxin.app}]\n mResumedActivity: ActivityRecord{abc u0 chat.xiaoxin.app/.MainActivity t42}\n";
        assert!(runtime_matches(base, true).unwrap());
        assert!(!runtime_matches(base, false).unwrap());
        for other in [base.replace("LOCKED", "PINNED"), base.replace("t42", "t43"), base.replace("u0", "u10"), base.replace("chat.xiaoxin.app/.MainActivity", "other.app/.MainActivity")] {
            assert!(!runtime_matches(&other, true).unwrap());
        }
        assert!(runtime_matches(&base.replace("LOCKED", "NONE"), false).unwrap());
        assert!(runtime_matches("", false).is_err());
        assert!(runtime_matches("ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)\n", false).is_err());
        let empty = base.replace("[TaskRecord{abc #42 A=chat.xiaoxin.app}]", "[]");
        assert!(!runtime_matches(&format!("{empty}Recent Tasks:\nTaskRecord{{abc #42 A=chat.xiaoxin.app}}"), true).unwrap());
        let multiline = base.replace("[TaskRecord{abc #42 A=chat.xiaoxin.app}]", "\n      #0: TaskRecord{abc #42 A=chat.xiaoxin.app}\n    mLockTaskPackages (userId:packages)=");
        assert!(runtime_matches(&multiline, true).unwrap());
        let android8 = base.replace("mLockTaskModeState=LOCKED", "mLockTaskModeState=LOCKED  mLockTaskPackages (userId:packages)=\n 0:[chat.xiaoxin.app]")
            .replace("mLockTaskModeTasks=", "mLockTaskModeTasks");
        assert!(runtime_matches(&android8, true).unwrap());
    }

    #[test]
    fn observed_android8_locked_tasks_match_only_current_main_activity() {
        // 2026-10-23 4寸屏系统输出中仅保留模式、任务和活动身份；无配置或设备私有数据。
        let runtime = "ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)\r\r\n\
    mResumedActivity: ActivityRecord{a2d307c u0 chat.xiaoxin.app/.MainActivity t603}\r\r\n\
  ResumedActivity: ActivityRecord{a2d307c u0 chat.xiaoxin.app/.MainActivity t603}\r\r\n\
  mLockTaskModeState=LOCKED  mLockTaskPackages (userId:packages)=\r\r\n\
    0:[chat.xiaoxin.app]\r\r\n\
 mLockTaskModeTasks[TaskRecord{1cc8dcc #603 I=chat.xiaoxin.app/.MainActivity U=0 StackId=1 sz=1}]\r\r\n\
  KeyguardController:\r\r\n";
        assert!(runtime_matches(runtime,true).unwrap());
        assert!(!runtime_matches(&runtime.replace("t603", "t595"),true).unwrap());
        assert!(!runtime_matches(&runtime.replace("chat.xiaoxin.app/.MainActivity t603", "com.android.settings/.Settings t603"),true).unwrap());
    }
}
