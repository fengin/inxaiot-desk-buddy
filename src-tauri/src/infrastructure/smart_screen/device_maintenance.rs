use super::{
    device::{AdbDevice, AndroidTools, DeviceCommandPort, XIAOXIN_PACKAGE},
    leases::HeldScreenLeases,
    task_data,
    value_updates::reliable_identity,
};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::{model::*, operation::ScreenPlan},
    formal::app_state::FormalAppState,
    infrastructure::local_sqlite::screen_repository::ScreenRepository,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

mod clock;

const PID_EXIT_MARKER: &str = "__INX_XIAOXIN_PIDOF_EXIT__=";
const STOP_WAIT_LIMIT: Duration = Duration::from_secs(15);
const STOP_WAIT_INTERVAL: Duration = Duration::from_millis(150);

async fn application_pids<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<Vec<u32>> {
    // ADB 的退出状态与 pidof 的退出状态分开判断，不能把断连当作应用已经停止。
    let command = format!(
        "pidof {XIAOXIN_PACKAGE}; status=$?; printf '\\n{PID_EXIT_MARKER}%s\\n' \"$status\""
    );
    let output = device
        .adb(
            ip,
            &["shell", &command],
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await?;
    if !output.success || !output.stderr.trim().is_empty() {
        return Err(AppError::Conflict(
            "无法读取小新进程状态，请检查设备连接和系统命令；未继续重启".into(),
        ));
    }
    let (pids, code) = output
        .stdout
        .rsplit_once(PID_EXIT_MARKER)
        .ok_or_else(|| AppError::Conflict("小新进程检查未返回完整结果，未继续重启".into()))?;
    if code.trim() == "1" && pids.trim().is_empty() {
        return Ok(Vec::new());
    }
    if code.trim() != "0" || pids.trim().is_empty() {
        return Err(AppError::Conflict("小新进程检查失败，未继续重启".into()));
    }
    let mut parsed = pids
        .split_whitespace()
        .map(|pid| {
            if !pid.bytes().all(|byte| byte.is_ascii_digit()) {
                return Err(AppError::Conflict("小新进程编号无效，未停止进程".into()));
            }
            pid.parse::<u32>()
                .ok()
                .filter(|pid| *pid > 0)
                .ok_or_else(|| AppError::Conflict("小新进程编号无效，未停止进程".into()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    parsed.sort_unstable();
    parsed.dedup();
    Ok(parsed)
}

fn pid_text(pids: &[u32]) -> String {
    pids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn main_activity_resumed(activities: &str) -> bool {
    activities.lines().any(|line| {
        (line.contains("mResumedActivity:")
            || line.contains("mResumedActivity=")
            || line.trim_start().starts_with("ResumedActivity:")
            || line.contains("topResumedActivity="))
            && (line.contains("chat.xiaoxin.app/.MainActivity ")
                || line.contains("chat.xiaoxin.app/chat.xiaoxin.app.MainActivity "))
    })
}

async fn read<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    args: &[&str],
) -> AppResult<String> {
    device.shell(ip, args, CancellationToken::new()).await
}
async fn boot_id<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<String> {
    let id = read(device, ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?;
    uuid::Uuid::parse_str(&id)
        .map_err(|_| AppError::Conflict("无法取得可靠的系统启动编号".into()))?;
    Ok(id)
}
async fn root_mode<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<String> {
    if read(device, ip, &["id"]).await?.contains("uid=0(") {
        return Ok("shell".into());
    }
    if read(device, ip, &["su", "0", "id"])
        .await?
        .contains("uid=0(")
    {
        return Ok("su0".into());
    }
    Err(AppError::Conflict(
        "当前固件未提供所需的管理权限，不能执行此操作".into(),
    ))
}
async fn root_command<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    args: &[&str],
) -> AppResult<String> {
    let mut command = Vec::new();
    if mode == "su0" {
        command.extend(["su", "0"]);
    } else if mode != "shell" {
        return Err(AppError::Conflict("管理权限未确认".into()));
    }
    command.extend(args);
    read(device, ip, &command).await
}
async fn clock_settings<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<Value> {
    Ok(
        json!({"zone":read(device,ip,&["getprop","persist.sys.timezone"]).await?,"autoTime":read(device,ip,&["settings","get","global","auto_time"]).await?,"autoZone":read(device,ip,&["settings","get","global","auto_time_zone"]).await?}),
    )
}
pub async fn capability<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
    action: &str,
) -> AppResult<Value> {
    let ip = &screen.fields.ip;
    let mut value = json!({});
    match action {
        "time" => {
            value["root"] = json!(root_mode(device, ip).await?);
            let help = read(device, ip, &["date", "--help"]).await?;
            if !help.contains("@UNIXTIME") {
                return Err(AppError::Conflict(
                    "当前固件日期命令尚未验证，不修改时间".into(),
                ));
            }
            value["clockSettings"] = clock_settings(device, ip).await?;
            value["clock"] = clock::capability(
                device,
                screen,
                observed,
                value["root"].as_str().unwrap_or(""),
            )
            .await?;
        }
        "adb" => {
            if screen.fields.size != "10"
                || observed.sdk != Some(29)
                || !observed
                    .firmware
                    .as_deref()
                    .is_some_and(|s| s.starts_with("STATION/ceres_c4/ceres-c3:10/"))
            {
                return Err(AppError::Conflict(
                    "保持端口目前只支持已验证的10寸 STATION Android 10 固件".into(),
                ));
            }
            value["root"] = json!(root_mode(device, ip).await?);
            value["bootId"] = json!(boot_id(device, ip).await?);
            value["previousPort"] =
                json!(read(device, ip, &["getprop", "persist.adb.tcp.port"]).await?);
        }
        "reboot" => {
            let persistent = read(device, ip, &["getprop", "persist.adb.tcp.port"]).await?;
            let known_four = screen.fields.size == "4"
                && observed
                    .firmware
                    .as_deref()
                    .is_some_and(|s| s.starts_with("rockchip/px30_evb/px30_evb:8.1.0/"))
                && read(device, ip, &["getprop", "ro.product.model"]).await? == "px30_evb";
            if persistent != "5555" && !known_four {
                return Err(AppError::Conflict(
                    "尚未确认重启后管理端口能恢复，请先处理端口设置".into(),
                ));
            }
            if known_four && persistent != "5555" {
                let build = read(device, ip, &["cat", "/vendor/build.prop"]).await?;
                if !build
                    .lines()
                    .any(|line| line.trim() == "service.adb.tcp.port=5555")
                {
                    return Err(AppError::Conflict(
                        "4寸固件未提供已验证的开机端口配置".into(),
                    ));
                }
            }
            value["bootId"] = json!(boot_id(device, ip).await?);
        }
        "restart" => {
            if observed.app_installed != Some(true) {
                return Err(AppError::Conflict("未确认小新已安装，不能重启应用".into()));
            }
            value["pid"] = json!(pid_text(&application_pids(device, ip).await?));
            let policy = read(device, ip, &["dumpsys", "device_policy"]).await?;
            let owner = policy.contains("admin=ComponentInfo{chat.xiaoxin.app/");
            value["deviceOwner"] = json!(owner);
            if owner {
                value["root"] = json!(root_mode(device, ip).await?);
            }
        }
        _ => return Err(AppError::InvalidConfig("设备维护动作不支持".into())),
    }
    Ok(value)
}
async fn checkpoint(
    state: &FormalAppState,
    plan: &ScreenPlan,
    id: &str,
    result: &ScreenTargetResult,
) -> AppResult<()> {
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await
}
async fn authorize(
    state: &FormalAppState,
    plan: &ScreenPlan,
    held: Option<&HeldScreenLeases>,
) -> AppResult<()> {
    if let Some(held) = held {
        crate::infrastructure::project_context::project_operator(state, &plan.project_id).await?;
        held.valid().await?;
    }
    Ok(())
}
pub async fn execute(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    original: &ScreenObservation,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
) -> AppResult<()> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let ip = &screen.fields.ip;
    let action = plan.input.action.as_str();
    let current = ScreenRepository::new(state.local_store.pool().clone())
        .asset(&plan.project_id, &screen.id)
        .await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current) {
        return Err(AppError::Conflict("设备资料已经变化，未执行维护".into()));
    }
    let before = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !reliable_identity(screen, &before) || before.observed_mac != original.observed_mac {
        return Err(AppError::Conflict(
            "设备身份与检查时不一致，未执行维护".into(),
        ));
    }
    let capabilities = capability(&device, screen, &before, action).await?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    authorize(state, plan, held).await?;
    result.evidence =
        json!({"phase":"requested","action":action,"before":before,"capability":capabilities});
    result.device = ResultState::Unknown;
    checkpoint(state, plan, id, result).await?;
    match action {
        "time" => {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            result.evidence["requestedSeconds"] = json!(now);
            checkpoint(state, plan, id, result).await?;
            let mode = capabilities["root"].as_str().unwrap_or("");
            clock::set_system(&device, ip, mode, now).await?;
            result.evidence["phase"] = json!("clock_set");
            checkpoint(state, plan, id, result).await?;
            authorize(state, plan, held).await?;
            result.evidence["phase"] = json!("hardware_clock_requested");
            checkpoint(state, plan, id, result).await?;
            clock::set_hardware(&device, ip, mode, &capabilities["clock"]["rtc"])
                .await
                .map_err(|error| {
                    AppError::Conflict(format!(
                        "系统时间已校准，但硬件时钟设置尚未确认，请核实结果：{error}"
                    ))
                })?;
            result.evidence["phase"] = json!("hardware_clock_set");
            checkpoint(state, plan, id, result).await?;
        }
        "adb" => {
            root_command(
                &device,
                ip,
                capabilities["root"].as_str().unwrap_or(""),
                &["setprop", "persist.adb.tcp.port", "5555"],
            )
            .await?;
            if read(&device, ip, &["getprop", "persist.adb.tcp.port"]).await? != "5555" {
                result.device = ResultState::Failed;
                return Err(AppError::Conflict("持久端口回读未通过，未重启".into()));
            }
            root_command(
                &device,
                ip,
                capabilities["root"].as_str().unwrap_or(""),
                &["sync"],
            )
            .await?;
            authorize(state, plan, held).await?;
            result.evidence["phase"] = json!("reboot_requested");
            checkpoint(state, plan, id, result).await?;
            let _ = device
                .adb(
                    ip,
                    &["reboot"],
                    Duration::from_secs(20),
                    CancellationToken::new(),
                )
                .await;
            wait_boot(&device, screen, &capabilities).await?;
        }
        "reboot" => {
            result.evidence["phase"] = json!("reboot_requested");
            checkpoint(state, plan, id, result).await?;
            let _ = device
                .adb(
                    ip,
                    &["reboot"],
                    Duration::from_secs(20),
                    CancellationToken::new(),
                )
                .await;
            wait_boot(&device, screen, &capabilities).await?;
        }
        "restart" => {
            stop_application(&device, ip, &capabilities).await?;
            result.evidence["phase"] = json!("app_stopped");
            checkpoint(state, plan, id, result).await?;
            authorize(state, plan, held).await?;
            if let Err(error) = start_application(&device, ip).await {
                result.device = ResultState::Failed;
                return Err(error);
            }
            result.evidence["phase"] = json!("app_started");
            checkpoint(state, plan, id, result).await?;
            wait_application_started_with_limit(
                &device,
                ip,
                capabilities["pid"].as_str().unwrap_or(""),
                Duration::from_secs(15),
                Duration::from_millis(250),
            )
            .await?;
        }
        _ => unreachable!(),
    }
    verify(state, id, plan, screen, result).await
}
/// 配置任务复用相同的停止、启动命令，在自己的任务内记录每一步。
pub(super) async fn stop_application<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    capability: &Value,
) -> AppResult<()> {
    stop_application_with_limit(device, ip, capability, STOP_WAIT_LIMIT, STOP_WAIT_INTERVAL).await
}
async fn stop_application_with_limit<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    capability: &Value,
    limit: Duration,
    interval: Duration,
) -> AppResult<()> {
    let original_pids = application_pids(device, ip).await?;
    if capability["deviceOwner"] == true {
        for pid in &original_pids {
            root_command(
                device,
                ip,
                capability["root"].as_str().unwrap_or(""),
                &["kill", "-15", &pid.to_string()],
            )
            .await?;
        }
    } else {
        read(
            device,
            ip,
            &["am", "force-stop", "--user", "0", XIAOXIN_PACKAGE],
        )
        .await?;
    }
    if original_pids.is_empty() {
        return Ok(());
    }
    tokio::time::timeout(limit, async {
        loop {
            let current_pids = application_pids(device, ip).await?;
            if original_pids.iter().all(|pid| !current_pids.contains(pid)) {
                return Ok(());
            }
            tokio::time::sleep(interval).await;
        }
    })
    .await
    .map_err(|_| AppError::Conflict("等待小新原进程停止超时，未再次启动；请检查屏端状态".into()))?
}
pub(super) async fn start_application<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<()> {
    let output = read(
        device,
        ip,
        &[
            "am",
            "start",
            "-n",
            "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity",
        ],
    )
    .await?;
    if output.contains("Error:") || output.contains("Exception") {
        return Err(AppError::Conflict(
            "小新已停止，但启动未通过，请检查设备；不会清空数据".into(),
        ));
    }
    Ok(())
}
async fn wait_application_started_with_limit<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    before_pid: &str,
    limit: Duration,
    interval: Duration,
) -> AppResult<()> {
    // 屏幕休眠时 Android 不一定产生 Displayed 事件；只核对进程和 Activity，不唤醒屏幕。
    tokio::time::timeout(limit, async {
        loop {
            let pids = application_pids(device, ip).await?;
            if !pids.is_empty() && pid_text(&pids) != before_pid {
                let activities = read(device, ip, &["dumpsys", "activity", "activities"]).await?;
                if main_activity_resumed(&activities) {
                    return Ok(());
                }
            }
            tokio::time::sleep(interval).await;
        }
    })
    .await
    .map_err(|_| {
        AppError::Conflict(
            "已发送启动请求，但尚未确认小新新进程和主界面；不会重复启动或清空数据".into(),
        )
    })?
}
async fn wait_boot(device: &AdbDevice, screen: &ScreenAsset, capability: &Value) -> AppResult<()> {
    wait_boot_with_limit(
        device,
        screen,
        capability,
        Duration::from_secs(180),
        Duration::from_secs(3),
    )
    .await
}
async fn wait_boot_with_limit<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    capability: &Value,
    limit: Duration,
    interval: Duration,
) -> AppResult<()> {
    tokio::time::timeout(limit, async {
        loop {
            tokio::time::sleep(interval).await;
            if device
                .connect(&screen.fields.ip, CancellationToken::new())
                .await
                .is_err()
            {
                continue;
            }
            let Ok(current_boot) = boot_id(device, &screen.fields.ip).await else {
                continue;
            };
            if Some(current_boot.as_str()) == capability["bootId"].as_str() {
                continue;
            }
            if read(
                device,
                &screen.fields.ip,
                &["getprop", "sys.boot_completed"],
            )
            .await
            .ok()
            .as_deref()
                != Some("1")
            {
                continue;
            }
            return;
        }
    })
    .await
    .map_err(|_| AppError::Conflict("等待重启后连接超时，保留待核实记录；不会重复重启".into()))
}
pub async fn verify(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let ip = &screen.fields.ip;
    let action = plan.input.action.as_str();
    let mut observed = device
        .inspect(screen, "inspect", CancellationToken::new())
        .await?;
    if !reliable_identity(screen, &observed)
        || observed.observed_mac.as_deref() != result.evidence["before"]["observedMac"].as_str()
    {
        return Err(AppError::Conflict(
            "尚未重新确认原设备身份，保留待核实记录".into(),
        ));
    }
    observed.task_id = Some(id.into());
    observed.operation_type = action.into();
    ScreenRepository::new(state.local_store.pool().clone())
        .append_observation(&plan.project_id, &observed)
        .await?;
    result.observation = Some(observed.clone());
    let capability = &result.evidence["capability"];
    match action {
        "time" => {
            let verified = clock::verify(&device, ip, &result.evidence).await?;
            result.evidence["clockVerification"] = verified.readings;
            result.device = if verified.succeeded {
                ResultState::Succeeded
            } else {
                ResultState::Failed
            };
            result.message = verified.message;
            if verified.succeeded {
                result.evidence["phase"] = json!("clock_verified");
            }
        }
        "adb" | "reboot" => {
            let boot = boot_id(&device, ip).await?;
            if Some(boot.as_str()) == capability["bootId"].as_str()
                || read(&device, ip, &["getprop", "sys.boot_completed"]).await? != "1"
            {
                return Err(AppError::Conflict(
                    "尚未确认本次系统重启完成，继续保留原记录，不再次重启".into(),
                ));
            }
            if action == "adb"
                && (read(&device, ip, &["getprop", "persist.adb.tcp.port"]).await? != "5555"
                    || read(&device, ip, &["getprop", "service.adb.tcp.port"]).await? != "5555")
            {
                result.device = ResultState::Failed;
                result.message = "系统已重启，但持久端口或运行端口未通过回读".into();
            } else if result.evidence["before"]["appRunning"] == true
                && observed.app_running != Some(true)
            {
                result.device = ResultState::Failed;
                result.message = "系统和管理连接已恢复，但原来运行的小新未恢复，请检查应用".into();
            } else {
                result.device = ResultState::Succeeded;
                result.message = if action == "adb" {
                    "已确认系统实际重启，持久端口和运行端口均为5555，管理连接已恢复"
                } else {
                    "已确认系统实际重启、开机完成和管理连接恢复"
                }
                .into();
            }
            result.evidence["afterBootId"] = json!(boot);
        }
        "restart" => {
            let pid = pid_text(&application_pids(&device, ip).await?);
            let activities = read(&device, ip, &["dumpsys", "activity", "activities"]).await?;
            if observed.app_running == Some(true)
                && !pid.is_empty()
                && Some(pid.as_str()) != capability["pid"].as_str()
                && main_activity_resumed(&activities)
            {
                result.device = ResultState::Succeeded;
                result.message = "小新已重新启动，已确认新进程和应用主界面，未清空应用数据".into();
                result.evidence["afterPid"] = json!(pid);
            } else {
                result.device = ResultState::Failed;
                result.message =
                    "未确认小新新进程及应用主界面；保留实际状态，不自动再次重启".into();
            }
        }
        _ => return Err(AppError::InvalidConfig("不能核实此维护动作".into())),
    }
    checkpoint(state, plan, id, result).await
}

#[cfg(test)]
mod tests {
    use super::super::device::ProcessOutput;
    use super::*;
    use std::path::Path;
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    #[derive(Clone)]
    enum PidReply {
        Running(&'static str),
        Exited,
        AdbFailure,
        CommandFailure,
        TransportFailure,
        ErrorOutput,
    }
    #[derive(Clone)]
    struct RestartCommands {
        replies: Arc<Mutex<VecDeque<PidReply>>>,
        fallback: PidReply,
        calls: Arc<Mutex<Vec<String>>>,
        activities: Arc<Mutex<VecDeque<&'static str>>>,
        activity_fallback: &'static str,
        start_output: &'static str,
    }
    impl RestartCommands {
        fn new(replies: Vec<PidReply>, fallback: PidReply) -> Self {
            Self {
                replies: Arc::new(Mutex::new(replies.into())),
                fallback,
                calls: Arc::default(),
                activities: Arc::default(),
                activity_fallback: "mResumedActivity: ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t8}",
                start_output: "Starting: Intent { cmp=chat.xiaoxin.app/.MainActivity }",
            }
        }
    }
    impl DeviceCommandPort for RestartCommands {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            let mut output = ProcessOutput {
                success: true,
                stdout: String::new(),
                stderr: String::new(),
            };
            if args.iter().any(|value| value.contains(PID_EXIT_MARKER)) {
                let reply = self
                    .replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| self.fallback.clone());
                let description = match reply {
                    PidReply::Running(pids) => {
                        output.stdout = format!("{pids}\n{PID_EXIT_MARKER}0\n");
                        format!("pid:{pids}")
                    }
                    PidReply::Exited => {
                        output.stdout = format!("\n{PID_EXIT_MARKER}1\n");
                        "pid:exited".into()
                    }
                    PidReply::AdbFailure => {
                        output.success = false;
                        "pid:adb-failed".into()
                    }
                    PidReply::CommandFailure => {
                        output.stdout = format!("\n{PID_EXIT_MARKER}2\n");
                        "pid:command-failed".into()
                    }
                    PidReply::ErrorOutput => {
                        output.stdout = format!("\n{PID_EXIT_MARKER}1\n");
                        output.stderr = "permission denied".into();
                        "pid:error-output".into()
                    }
                    PidReply::TransportFailure => {
                        self.calls.lock().unwrap().push("pid:disconnected".into());
                        return Err(AppError::Conflict("ADB 连接断开".into()));
                    }
                };
                self.calls.lock().unwrap().push(description);
            } else if args.iter().any(|value| value == "kill") {
                self.calls
                    .lock()
                    .unwrap()
                    .push(format!("kill:{}", args.last().unwrap()));
            } else if args.iter().any(|value| value == "force-stop") {
                self.calls.lock().unwrap().push("force-stop".into());
            } else if args.iter().any(|value| value == "start") {
                assert!(
                    !args.iter().any(|value| value == "-W"),
                    "启动不得等待屏幕绘制"
                );
                self.calls.lock().unwrap().push("start".into());
                output.stdout = self.start_output.into();
            } else if args.iter().any(|value| value == "activities") {
                let activity = self
                    .activities
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(self.activity_fallback);
                self.calls.lock().unwrap().push(
                    if main_activity_resumed(activity) {
                        "activity:resumed"
                    } else {
                        "activity:waiting"
                    }
                    .into(),
                );
                output.stdout = activity.into();
            } else {
                panic!("unexpected restart command: {args:?}");
            }
            Ok(output)
        }
    }
    async fn restart_with_limits(
        commands: RestartCommands,
        owner: bool,
        limit: Duration,
    ) -> AppResult<()> {
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            commands,
        );
        stop_application_with_limit(
            &device,
            "192.0.2.1",
            &json!({"deviceOwner":owner,"root":"shell"}),
            limit,
            Duration::from_millis(1),
        )
        .await?;
        start_application(&device, "192.0.2.1").await
    }

    #[tokio::test]
    async fn owner_restart_waits_for_all_original_pids_before_starting_activity() {
        let commands = RestartCommands::new(
            vec![
                PidReply::Running("2125 2130"),
                PidReply::Running("2125 2130"),
                PidReply::Running("2130"),
                PidReply::Running("4520"),
            ],
            PidReply::Exited,
        );
        restart_with_limits(commands.clone(), true, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            *commands.calls.lock().unwrap(),
            vec![
                "pid:2125 2130",
                "kill:2125",
                "kill:2130",
                "pid:2125 2130",
                "pid:2130",
                "pid:4520",
                "start"
            ]
        );
    }

    #[tokio::test]
    async fn force_stop_also_waits_for_exit_and_code_one_empty_output_means_stopped() {
        let commands = RestartCommands::new(
            vec![
                PidReply::Running("2125"),
                PidReply::Running("2125"),
                PidReply::Exited,
            ],
            PidReply::Exited,
        );
        restart_with_limits(commands.clone(), false, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            *commands.calls.lock().unwrap(),
            vec!["pid:2125", "force-stop", "pid:2125", "pid:exited", "start"]
        );
        let absent = RestartCommands::new(vec![PidReply::Exited], PidReply::Exited);
        restart_with_limits(absent.clone(), true, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(*absent.calls.lock().unwrap(), vec!["pid:exited", "start"]);
    }

    #[tokio::test]
    async fn stop_timeout_never_starts_activity_or_repeats_kill() {
        let commands =
            RestartCommands::new(vec![PidReply::Running("2125")], PidReply::Running("2125"));
        let error = restart_with_limits(commands.clone(), true, Duration::from_millis(15))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("停止超时"));
        let calls = commands.calls.lock().unwrap();
        assert!(!calls.iter().any(|call| call == "start"));
        assert_eq!(calls.iter().filter(|call| *call == "kill:2125").count(), 1);
    }

    #[tokio::test]
    async fn pid_connection_command_and_parse_errors_never_count_as_stopped() {
        for reply in [
            PidReply::AdbFailure,
            PidReply::CommandFailure,
            PidReply::TransportFailure,
            PidReply::ErrorOutput,
            PidReply::Running("invalid"),
            PidReply::Running("0"),
        ] {
            let commands =
                RestartCommands::new(vec![PidReply::Running("2125"), reply], PidReply::Exited);
            assert!(
                restart_with_limits(commands.clone(), true, Duration::from_secs(1))
                    .await
                    .is_err()
            );
            assert!(
                !commands
                    .calls
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|call| call == "start")
            );
        }
        let commands = RestartCommands::new(vec![PidReply::AdbFailure], PidReply::Exited);
        assert!(
            restart_with_limits(commands.clone(), true, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert_eq!(*commands.calls.lock().unwrap(), vec!["pid:adb-failed"]);
    }

    #[tokio::test]
    async fn start_without_drawing_wait_accepts_sleeping_screen_after_read_only_checks() {
        let mut commands = RestartCommands::new(Vec::new(), PidReply::Running("5447"));
        commands
            .activities
            .lock()
            .unwrap()
            .push_back("mResumedActivity: ActivityRecord{abcd u0 other.app/.MainActivity t1}");
        commands.activity_fallback = "mSleeping=true\nmResumedActivity: ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t8}";
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            commands.clone(),
        );
        start_application(&device, "192.0.2.1").await.unwrap();
        wait_application_started_with_limit(
            &device,
            "192.0.2.1",
            "2125",
            Duration::from_secs(1),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert_eq!(
            *commands.calls.lock().unwrap(),
            vec![
                "start",
                "pid:5447",
                "activity:waiting",
                "pid:5447",
                "activity:resumed"
            ]
        );
    }

    #[tokio::test]
    async fn missing_main_activity_or_connection_error_never_replays_start() {
        let mut commands = RestartCommands::new(Vec::new(), PidReply::Running("5447"));
        commands.activity_fallback = "ProviderRecord{abcd chat.xiaoxin.app/.MaintenanceProvider}";
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            commands.clone(),
        );
        start_application(&device, "192.0.2.1").await.unwrap();
        let error = wait_application_started_with_limit(
            &device,
            "192.0.2.1",
            "2125",
            Duration::from_millis(15),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("不会重复启动"));
        assert_eq!(
            commands
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|call| *call == "start")
                .count(),
            1
        );

        let disconnected = RestartCommands::new(Vec::new(), PidReply::TransportFailure);
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            disconnected.clone(),
        );
        start_application(&device, "192.0.2.1").await.unwrap();
        assert!(
            wait_application_started_with_limit(
                &device,
                "192.0.2.1",
                "2125",
                Duration::from_secs(1),
                Duration::from_millis(1)
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("连接断开")
        );
        assert_eq!(
            *disconnected.calls.lock().unwrap(),
            vec!["start", "pid:disconnected"]
        );
    }

    #[tokio::test]
    async fn explicit_activity_start_errors_are_still_rejected() {
        for output in [
            "Error: Activity class does not exist",
            "Exception: Permission denied",
        ] {
            let mut commands = RestartCommands::new(Vec::new(), PidReply::Exited);
            commands.start_output = output;
            let device = AdbDevice::with_commands(
                AndroidTools {
                    adb: "unused".into(),
                },
                commands,
            );
            assert!(start_application(&device, "192.0.2.1").await.is_err());
        }
    }

    #[test]
    fn restart_confirmation_requires_main_activity_not_only_provider_pid() {
        assert!(main_activity_resumed(
            "mResumedActivity: ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t3}"
        ));
        assert!(main_activity_resumed(
            "ResumedActivity: ActivityRecord{abcd u0 chat.xiaoxin.app/chat.xiaoxin.app.MainActivity t3}"
        ));
        assert!(main_activity_resumed(
            "topResumedActivity=ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t3}"
        ));
        assert!(!main_activity_resumed(
            "ProcessRecord{abcd 4520:chat.xiaoxin.app/u0a1}\nProviderRecord{abcd chat.xiaoxin.app/.MaintenanceProvider}"
        ));
        assert!(!main_activity_resumed(
            "Hist #0: ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t3}\nmResumedActivity: ActivityRecord{abcd u0 other.app/.MainActivity t4}"
        ));
    }
    struct Unprivileged;
    struct NoReboot;
    impl DeviceCommandPort for NoReboot {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            assert!(
                !args
                    .iter()
                    .any(|value| value == "reboot" || value == "setprop")
            );
            let stdout = match args.last().map(String::as_str) {
                Some("get-state") => "device",
                Some("/proc/sys/kernel/random/boot_id") => "00000000-0000-4000-8000-000000000001",
                Some("sys.boot_completed") => "1",
                _ => "",
            };
            Ok(ProcessOutput {
                success: true,
                stdout: stdout.into(),
                stderr: String::new(),
            })
        }
    }
    #[tokio::test]
    async fn unchanged_boot_id_times_out_without_resending_reboot() {
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            NoReboot,
        );
        let screen = ScreenAsset {
            fields: ScreenFields {
                ip: "192.0.2.1".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let error = wait_boot_with_limit(
            &device,
            &screen,
            &json!({"bootId":"00000000-0000-4000-8000-000000000001"}),
            Duration::from_millis(30),
            Duration::from_millis(1),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("不会重复重启"));
    }
    impl DeviceCommandPort for Unprivileged {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            Ok(ProcessOutput {
                success: true,
                stdout: if args.last().is_some_and(|s| s == "id") {
                    "uid=2000(shell)".into()
                } else {
                    String::new()
                },
                stderr: String::new(),
            })
        }
    }
    #[tokio::test]
    async fn missing_permissions_unsupported_firmware_and_absent_app_are_blocked() {
        let device = AdbDevice::with_commands(
            AndroidTools {
                adb: "unused".into(),
            },
            Unprivileged,
        );
        let screen = ScreenAsset {
            id: "screen".into(),
            source: "local".into(),
            fields: ScreenFields {
                ip: "192.0.2.1".into(),
                size: "4".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let observed = ScreenObservation {
            sdk: Some(27),
            app_installed: Some(false),
            ..Default::default()
        };
        assert!(
            capability(&device, &screen, &observed, "time")
                .await
                .unwrap_err()
                .to_string()
                .contains("权限")
        );
        assert!(
            capability(&device, &screen, &observed, "adb")
                .await
                .unwrap_err()
                .to_string()
                .contains("10寸")
        );
        assert!(
            capability(&device, &screen, &observed, "restart")
                .await
                .is_err()
        );
        assert!(
            capability(&device, &screen, &observed, "reboot")
                .await
                .is_err()
        );
    }
}
