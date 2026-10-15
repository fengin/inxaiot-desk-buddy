//! 小新持久主屏策略。应用负责设备所有者 API，工作台另读系统规则确认实际结果。
use super::device::{AdbDevice, DeviceCommandPort, ProcessOutput, XIAOXIN_PACKAGE};
use crate::core::error::{AppError, AppResult};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const URI: &str = "content://chat.xiaoxin.app.workbench.home";
pub const COMPONENT: &str = "chat.xiaoxin.app/chat.xiaoxin.app.MainActivity";
#[cfg(test)]
const SHORT_COMPONENT: &str = "chat.xiaoxin.app/.MainActivity";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeStatus {
    pub protocol_version: u32,
    pub package_name: String,
    pub device_owner: bool,
    pub persistent_preferred: bool,
    pub resolved_home: Option<String>,
}

impl HomeStatus {
    pub fn verified(&self) -> bool {
        self.device_owner
            && self.persistent_preferred
            && self.resolved_home.as_deref() == Some(COMPONENT)
    }

    /// 撤销仅解除持久固定；小新仍可作为普通默认主屏，不要求同时切换到其他桌面。
    pub fn matches_target(&self, enabled: bool) -> bool {
        if enabled {
            self.verified()
        } else {
            self.device_owner && !self.persistent_preferred
        }
    }
}

/// 系统运行状态与持久策略分开核验。进程只取同一活动记录中的 app 字段，
/// 普通任务中的小新、历史 HOME 记录及别的用户/显示屏不能代表主屏已激活。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HomeRuntimeStatus {
    pub home_task: bool,
    pub resumed: bool,
    pub pid: Option<u32>,
    pub task_id: Option<u32>,
    pub component: Option<String>,
}

impl HomeRuntimeStatus {
    pub fn verified(&self) -> bool {
        self.home_task
            && self.resumed
            && self.pid.is_some_and(|pid| pid > 0)
            && self.task_id.is_some()
            && self.component.as_deref() == Some(COMPONENT)
    }

    fn inactive() -> Self {
        Self {
            home_task: false,
            resumed: false,
            pid: None,
            task_id: None,
            component: None,
        }
    }
}

fn runtime_format_error() -> AppError {
    AppError::Conflict(
        "系统未返回可识别的完整主屏运行状态，未确认主屏已激活；不会自动重复激活".into(),
    )
}

/// 解析完整 dumpsys activity activities。按显示屏、栈、任务、活动逐层绑定，
/// 不对整段输出搜索小新名称后直接认定成功。
pub fn parse_runtime(text: &str) -> AppResult<HomeRuntimeStatus> {
    use regex::Regex;
    if text.len() > 8 * 1024 * 1024
        || !text
            .trim_start()
            .starts_with("ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)")
    {
        return Err(runtime_format_error());
    }
    let (activities, supervisor) =
        if let Some(sections) = text.split_once("ActivityStackSupervisor state:") {
            sections
        } else {
            // 4 寸 Android 8.1 实机直接输出焦点与任务映射，没有 Supervisor 标题。
            // 只接受顶层精确字段作为分界；不能把活动详情中的相似文本当作尾段。
            let boundary = Regex::new(r"(?m)^  mFocusedStack=ActivityStack\{[^\r\n]+\}\r?$")
                .unwrap()
                .find(text)
                .ok_or_else(runtime_format_error)?;
            text.split_at(boundary.start())
        };
    // 两种已验证格式都包含这些尾段字段。运行证据必须读到 Keyguard 的末项，
    // 避免已读到某个 RESUMED 活动但系统尾段截断时提前判定成功。
    let mut tail_position = 0;
    for pattern in [
        r"(?m)^  mCurTaskIdForUser=\{[^\r\n]*\}\r?$",
        r"(?m)^  mUserStackInFront=\{[^\r\n]*\}\r?$",
        r"(?m)^[^\r\n]*KeyguardController:\r?$",
        r"(?m)^    mVisibilityTransactionDepth=\d+\r?$",
    ] {
        let field = Regex::new(pattern)
            .unwrap()
            .find_at(supervisor, tail_position)
            .ok_or_else(runtime_format_error)?;
        tail_position = field.end();
    }
    let display_re = Regex::new(r"^Display #(\d+) \(activities from top to bottom\):$").unwrap();
    let stack_re = Regex::new(r"^  Stack #(\d+):(?: type=([a-z]+) mode=[a-z-]+)?$").unwrap();
    let task_re = Regex::new(r"^    Task id #(\d+)$").unwrap();
    let task_record_re =
        Regex::new(r"^    \* TaskRecord\{[^\s]+ #(\d+) .* U=(\d+) StackId=(\d+) sz=\d+\}$")
            .unwrap();
    let hist_re =
        Regex::new(r"^      \* Hist #\d+: ActivityRecord\{([^\s]+) u(\d+) ([^\s]+) t(\d+)[^}]*\}$")
            .unwrap();
    let resumed_re = Regex::new(
        r"^    mResumedActivity: ActivityRecord\{([^\s]+) u(\d+) ([^\s]+) t(\d+)[^}]*\}$",
    )
    .unwrap();
    let process_re =
        Regex::new(r"^app=ProcessRecord\{[^\s]+ ([1-9]\d*):chat\.xiaoxin\.app/u0a\d+\}$").unwrap();
    let focused_re = Regex::new(r"(?m)^  topDisplayFocusedStack=ActivityStack\{[^\s]+ stackId=(\d+) type=home mode=[^\s]+ visible=true(?:[ ,])").unwrap();
    let legacy_focused_re =
        Regex::new(r"(?m)^  mFocusedStack=ActivityStack\{[^\s]+ stackId=(\d+), \d+ tasks\}")
            .unwrap();
    let modern = supervisor
        .lines()
        .any(|line| line.trim_start().starts_with("topDisplayFocusedStack="));
    let legacy = supervisor
        .lines()
        .any(|line| line.trim_start().starts_with("mFocusedStack="));
    let focused_stack = if modern {
        focused_re.captures(supervisor)
    } else {
        legacy_focused_re.captures(supervisor)
    }
    .and_then(|capture| capture[1].parse::<u32>().ok());
    // A completed supervisor section must identify focus even when the foreground is not HOME.
    if !modern && !legacy {
        return Err(runtime_format_error());
    }
    let lines: Vec<&str> = activities
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect();
    let mut display = None;
    let mut saw_default_display = false;
    let mut saw_stack = false;
    let mut result = HomeRuntimeStatus::inactive();
    let mut index = 0;
    while index < lines.len() {
        if lines[index].trim_start().starts_with("Display #") {
            let capture = display_re
                .captures(lines[index])
                .ok_or_else(runtime_format_error)?;
            display = Some(
                capture[1]
                    .parse::<u32>()
                    .map_err(|_| runtime_format_error())?,
            );
            saw_default_display |= display == Some(0);
        }
        if lines[index].starts_with("  Stack #") {
            let stack = stack_re
                .captures(lines[index])
                .ok_or_else(runtime_format_error)?;
            if modern != stack.get(2).is_some() {
                return Err(runtime_format_error());
            }
            saw_stack = true;
            let stack_id: u32 = stack[1].parse().map_err(|_| runtime_format_error())?;
            let end = (index + 1..lines.len())
                .find(|&next| {
                    lines[next].starts_with("  Stack #")
                        || lines[next].trim_start().starts_with("Display #")
                })
                .unwrap_or(lines.len());
            // Android 8.1 没有栈 type 字段，使用该任务 taskType=1 与活动
            // mActivityType=HOME_ACTIVITY_TYPE 双重证明 HOME，绝不按栈编号猜测。
            if display == Some(0) && (!modern || &stack[2] == "home") {
                let block = &lines[index + 1..end];
                let awake = block.iter().any(|line| *line == "  isSleeping=false");
                let resumed = block.iter().find_map(|line| resumed_re.captures(line));
                let mut task_start = 0;
                while task_start < block.len() {
                    let Some(task) = task_re.captures(block[task_start]) else {
                        task_start += 1;
                        continue;
                    };
                    let task_id: u32 = task[1].parse().map_err(|_| runtime_format_error())?;
                    let task_end = (task_start + 1..block.len())
                        .find(|&next| {
                            task_re.is_match(block[next])
                                || block[next].starts_with("    Running activities")
                        })
                        .unwrap_or(block.len());
                    let task_block = &block[task_start + 1..task_end];
                    let record = task_block
                        .iter()
                        .find_map(|line| task_record_re.captures(line))
                        .ok_or_else(runtime_format_error)?;
                    let target_task = record[1].parse::<u32>().ok() == Some(task_id)
                        && &record[2] == "0"
                        && record[3].parse::<u32>().ok() == Some(stack_id)
                        && task_block
                            .iter()
                            .take_while(|line| !hist_re.is_match(line))
                            .any(|line| {
                                line.split_whitespace().any(|token| {
                                    token
                                        == if modern {
                                            "activityType=2"
                                        } else {
                                            "taskType=1"
                                        }
                                })
                            });
                    if target_task {
                        for (hist_start, line) in task_block.iter().enumerate() {
                            let Some(hist) = hist_re.captures(line) else {
                                continue;
                            };
                            if &hist[2] != "0"
                                || hist[4].parse::<u32>().ok() != Some(task_id)
                                || normalize_component(&hist[3]).as_deref() != Some(COMPONENT)
                            {
                                continue;
                            }
                            let hist_end = (hist_start + 1..task_block.len())
                                .find(|&next| hist_re.is_match(task_block[next]))
                                .unwrap_or(task_block.len());
                            let details = &task_block[hist_start + 1..hist_end];
                            let activity_is_home = details.iter().any(|line| {
                                line.trim()
                                    == if modern {
                                        "mActivityType=home"
                                    } else {
                                        "mActivityType=HOME_ACTIVITY_TYPE"
                                    }
                            });
                            if !activity_is_home {
                                continue;
                            }
                            let pid = details
                                .iter()
                                .find_map(|line| process_re.captures(line.trim()))
                                .and_then(|capture| capture[1].parse::<u32>().ok());
                            let is_resumed = details.iter().any(|line| {
                                line.split_whitespace().any(|word| word == "state=RESUMED")
                            }) && details.iter().any(|line| {
                                line.split_whitespace().any(|word| word == "visible=true")
                            }) && resumed.as_ref().is_some_and(|active| {
                                active[1] == hist[1]
                                    && active[2] == hist[2]
                                    && normalize_component(&active[3]).as_deref() == Some(COMPONENT)
                                    && active[4].parse::<u32>().ok() == Some(task_id)
                            }) && awake
                                && focused_stack == Some(stack_id);
                            let candidate = HomeRuntimeStatus {
                                home_task: true,
                                resumed: is_resumed,
                                pid,
                                task_id: Some(task_id),
                                component: Some(COMPONENT.into()),
                            };
                            if candidate.verified() || !result.verified() {
                                result = candidate;
                            }
                        }
                    }
                    task_start = task_end;
                }
            }
            index = end;
            continue;
        }
        index += 1;
    }
    if !saw_default_display || !saw_stack {
        return Err(runtime_format_error());
    }
    Ok(result)
}

/// 只读取系统活动管理器，不调用会启动应用进程的 ContentProvider。
pub async fn query_runtime<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    cancel: CancellationToken,
) -> AppResult<HomeRuntimeStatus> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let output = device
        .adb(
            ip,
            &["shell", "dumpsys", "activity", "activities"],
            Duration::from_secs(10),
            cancel,
        )
        .await?;
    check_output(&output)?;
    parse_runtime(&output.stdout)
}

/// 单次请求依次发送 WAKEUP 与 HOME，由系统唤醒显示并按已保存策略激活主屏。
/// WAKEUP 不会像 POWER 一样反向关闭已亮显示；请求结果未知时不重复发送。
pub async fn activate_home<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    cancel: CancellationToken,
) -> AppResult<()> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let output = device
        .adb(
            ip,
            &[
                "shell",
                "input",
                "keyevent",
                "KEYCODE_WAKEUP",
                "KEYCODE_HOME",
            ],
            Duration::from_secs(10),
            cancel,
        )
        .await?;
    check_output(&output)?;
    if !output.stdout.trim().is_empty() {
        return Err(AppError::Conflict(
            "系统未确认唤醒并激活主屏命令的结果，请重新核实；不会自动重复激活".into(),
        ));
    }
    Ok(())
}

/// 唤醒并激活后有界等待；每次只读取系统状态，不再发按键或调用应用。
pub async fn wait_runtime_verified<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    limit: Duration,
    cancel: CancellationToken,
) -> AppResult<HomeRuntimeStatus> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let status = tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            result = tokio::time::timeout(remaining, query_runtime(device, ip, cancel.clone())) => {
                result.map_err(|_| AppError::Conflict("等待系统主屏激活核验超时，请重新核实；不会自动重复激活".into()))??
            }
        };
        if status.verified() {
            return Ok(status);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(status);
        }
        tokio::select! {
            _ = cancel.cancelled() => return Err(AppError::Cancelled),
            _ = tokio::time::sleep(remaining.min(Duration::from_millis(500))) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(status);
        }
    }
}

pub enum HomeWrite {
    Applied,
    /// 仅在应用明确拒绝且尚未调用策略写入时，才确定没有写入。
    Rejected(String),
}

fn response_error(code: Option<&str>) -> &'static str {
    match code {
        Some("NOT_DEVICE_OWNER") => {
            "小新不是设备所有者，不能持久设置主屏；请先按屏端部署流程配置管理权限"
        }
        Some("FORBIDDEN") => "设备拒绝当前管理身份，不能设置主屏应用",
        Some("NOT_READY") => "小新主屏管理尚未就绪，请稍后重新检查",
        Some("UNSUPPORTED") => "当前小新不支持持久主屏管理，请先升级到支持此功能的版本",
        Some("INVALID_REQUEST") => "小新拒绝不受支持的主屏管理参数，未修改主屏策略",
        _ => "小新未确认主屏策略结果，请重新核实；不会自动重复设置",
    }
}

fn parse_response(text: &str) -> AppResult<Value> {
    let malformed = || {
        AppError::Conflict(
            "未收到有效的小新主屏管理结果，请检查应用版本和连接；设置结果需重新核实".into(),
        )
    };
    if text.len() > 24 * 1024 {
        return Err(malformed());
    }
    let encoded = text
        .trim()
        .strip_prefix("Result: Bundle[{result=")
        .and_then(|text| text.strip_suffix("}]"))
        .ok_or_else(malformed)?;
    let bytes = STANDARD.decode(encoded).map_err(|_| malformed())?;
    if bytes.len() > 16 * 1024 {
        return Err(malformed());
    }
    let response: Value = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    if !matches!(response["status"].as_str(), Some("ok" | "failed")) {
        return Err(malformed());
    }
    Ok(response)
}

async fn call<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    method: &str,
) -> AppResult<Value> {
    let output = device
        .adb(
            ip,
            &["shell", "content", "call", "--uri", URI, "--method", method],
            Duration::from_secs(20),
            CancellationToken::new(),
        )
        .await?;
    if output.stdout.contains("Could not find provider")
        || output.stderr.contains("Could not find provider")
    {
        return Err(AppError::Conflict(
            response_error(Some("UNSUPPORTED")).into(),
        ));
    }
    check_output(&output)?;
    parse_response(&output.stdout)
}

fn check_output(output: &ProcessOutput) -> AppResult<()> {
    let text = output.stdout.to_ascii_lowercase();
    if !output.success
        || !output.stderr.trim().is_empty()
        || [
            "permission denial",
            "permission denied",
            "securityexception",
            "exception occurred",
            "error:",
            "can't find service",
            "unknown command",
        ]
        .iter()
        .any(|word| text.contains(word))
    {
        return Err(AppError::Conflict(
            "主屏管理命令未成功，请检查 ADB 连接及权限；不会自动重复设置".into(),
        ));
    }
    Ok(())
}

fn normalize_component(text: &str) -> Option<String> {
    let value = text.trim();
    if value == "null" || value.is_empty() || value == "No activity found" {
        None
    } else if let Some((package, class)) = value.split_once('/') {
        Some(if class.starts_with('.') {
            format!("{package}/{package}{class}")
        } else {
            value.into()
        })
    } else {
        Some(value.into())
    }
}

fn parse_resolved_home(text: &str) -> AppResult<Option<String>> {
    let lines: Vec<_> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let component = match lines.as_slice() {
        [component] => *component,
        [details, component] if details.starts_with("priority=") => *component,
        _ => {
            return Err(AppError::Conflict(
                "系统未返回完整主屏解析结果，请重新核实".into(),
            ));
        }
    };
    if component == "No activity found" {
        return Ok(None);
    }
    if !regex::Regex::new(r"^[A-Za-z][A-Za-z0-9_.]*/[A-Za-z.][A-Za-z0-9_.$]*$")
        .unwrap()
        .is_match(component)
    {
        return Err(AppError::Conflict(
            "系统主屏解析结果格式不支持，未确认设置成功".into(),
        ));
    }
    Ok(normalize_component(component))
}

fn read_data(response: &Value) -> AppResult<(bool, Option<String>)> {
    if response["status"] != "ok" {
        return Err(AppError::Conflict(
            response_error(response["code"].as_str()).into(),
        ));
    }
    let data = &response["data"];
    if data["protocolVersion"] != 1
        || data["packageName"] != XIAOXIN_PACKAGE
        || !data["deviceOwner"].is_boolean()
        || data.get("resolvedHome").is_none()
        || !(data["resolvedHome"].is_null() || data["resolvedHome"].is_string())
    {
        return Err(AppError::Conflict(
            "小新主屏管理协议不匹配，请升级到配套版本".into(),
        ));
    }
    Ok((
        data["deviceOwner"] == true,
        data["resolvedHome"].as_str().and_then(normalize_component),
    ))
}

/// 只读取 User 0 的系统策略文件。普通 preferred-activities 或不同过滤器不能算持久主屏。
pub fn has_persistent_home(text: &str) -> AppResult<bool> {
    use quick_xml::{Reader, events::Event};
    let invalid =
        || AppError::Conflict("系统主屏策略文件不完整或格式不支持，未确认持久设置".into());
    if text.is_empty() || text.len() > 4 * 1024 * 1024 {
        return Err(invalid());
    }
    let mut reader = Reader::from_str(text);
    let mut path: Vec<String> = Vec::new();
    let mut root_seen = false;
    let mut root_closed = false;
    let mut item_target = false;
    let mut filter_flags = 0u8;
    let mut unrestricted = true;
    let mut found = false;
    let mut elements = 0usize;
    loop {
        let event = reader.read_event().map_err(|_| invalid())?;
        match event {
            Event::Start(ref element) | Event::Empty(ref element) => {
                elements += 1;
                if elements > 50000 || path.len() > 32 || root_closed {
                    return Err(invalid());
                }
                let name = std::str::from_utf8(element.name().as_ref())
                    .map_err(|_| invalid())?
                    .to_owned();
                let mut attribute_name = None;
                for attribute in element.attributes() {
                    let attribute = attribute.map_err(|_| invalid())?;
                    let value = attribute
                        .decoded_and_normalized_value(
                            quick_xml::XmlVersion::Implicit1_0,
                            reader.decoder(),
                        )
                        .map_err(|_| invalid())?;
                    if attribute.key.as_ref() == b"name" {
                        attribute_name = Some(value.into_owned());
                    }
                }
                if path.is_empty() {
                    if root_seen || name != "package-restrictions" {
                        return Err(invalid());
                    }
                    root_seen = true;
                }
                path.push(name);
                if path
                    == [
                        "package-restrictions",
                        "persistent-preferred-activities",
                        "item",
                    ]
                {
                    item_target = attribute_name
                        .as_deref()
                        .and_then(normalize_component)
                        .as_deref()
                        == Some(COMPONENT);
                } else if path
                    == [
                        "package-restrictions",
                        "persistent-preferred-activities",
                        "item",
                        "filter",
                    ]
                {
                    filter_flags = 0;
                    unrestricted = true;
                } else if item_target
                    && path.len() == 5
                    && path[..4]
                        == [
                            "package-restrictions",
                            "persistent-preferred-activities",
                            "item",
                            "filter",
                        ]
                {
                    unrestricted &= matches!(path[4].as_str(), "action" | "cat");
                    filter_flags |= match (path[4].as_str(), attribute_name.as_deref()) {
                        ("action", Some("android.intent.action.MAIN")) => 1,
                        ("cat", Some("android.intent.category.HOME")) => 2,
                        ("cat", Some("android.intent.category.DEFAULT")) => 4,
                        _ => 0,
                    };
                }
                if matches!(event, Event::Empty(_)) {
                    if path.len() == 1 {
                        root_closed = true;
                    }
                    path.pop();
                }
            }
            Event::End(_) => {
                if path
                    == [
                        "package-restrictions",
                        "persistent-preferred-activities",
                        "item",
                        "filter",
                    ]
                {
                    found |= item_target && unrestricted && filter_flags == 7;
                }
                if path
                    == [
                        "package-restrictions",
                        "persistent-preferred-activities",
                        "item",
                    ]
                {
                    item_target = false;
                }
                if path.len() == 1 {
                    root_closed = true;
                }
                path.pop().ok_or_else(invalid)?;
            }
            Event::Eof => break,
            Event::Text(value) if value.iter().all(u8::is_ascii_whitespace) => {}
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) => {}
            _ => return Err(invalid()),
        }
    }
    if !root_seen || !root_closed || !path.is_empty() {
        return Err(invalid());
    }
    Ok(found)
}

pub async fn query<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<HomeStatus> {
    let response = call(device, ip, "readHome").await?;
    let (owner, reported) = read_data(&response)?;
    if !owner {
        return Err(AppError::Conflict(
            response_error(Some("NOT_DEVICE_OWNER")).into(),
        ));
    }
    // Android 8/10 的 dumpsys package preferred 不导出持久策略，必须独立读取系统保存的规则。
    let mode = super::device_maintenance::root_mode(device, ip)
        .await
        .map_err(|_| {
            AppError::Conflict(
                "当前固件未提供读取系统主屏策略的管理权限，不能可靠验证持久主屏".into(),
            )
        })?;
    let mut args = vec!["shell"];
    if mode == "su0" {
        args.extend(["su", "0"]);
    }
    args.extend(["cat", "/data/system/users/0/package-restrictions.xml"]);
    let preferred = device
        .adb(ip, &args, Duration::from_secs(20), CancellationToken::new())
        .await?;
    check_output(&preferred)?;
    let resolved = device
        .adb(
            ip,
            &[
                "shell",
                "cmd",
                "package",
                "resolve-activity",
                "--brief",
                "--user",
                "0",
                "-a",
                "android.intent.action.MAIN",
                "-c",
                "android.intent.category.HOME",
                "-c",
                "android.intent.category.DEFAULT",
            ],
            Duration::from_secs(20),
            CancellationToken::new(),
        )
        .await?;
    check_output(&resolved)?;
    let resolved_home = parse_resolved_home(&resolved.stdout)?;
    if resolved_home != reported {
        return Err(AppError::Conflict(
            "应用与系统读取的主屏结果不一致，请重新检查；未确认持久主屏设置".into(),
        ));
    }
    Ok(HomeStatus {
        protocol_version: 1,
        package_name: XIAOXIN_PACKAGE.into(),
        device_owner: owner,
        persistent_preferred: has_persistent_home(&preferred.stdout)?,
        resolved_home,
    })
}

pub async fn set_home<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<HomeWrite> {
    set_enabled(device, ip, true).await
}

pub async fn clear_home<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<HomeWrite> {
    set_enabled(device, ip, false).await
}

/// 每次只发送一个明确设置或撤销请求；结果未知不重发。
pub async fn set_enabled<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    enabled: bool,
) -> AppResult<HomeWrite> {
    let method = if enabled { "setHome" } else { "clearHome" };
    let response = call(device, ip, method).await?;
    if response["status"] == "failed" {
        let code = response["code"].as_str();
        if matches!(
            code,
            Some(
                "NOT_DEVICE_OWNER" | "FORBIDDEN" | "NOT_READY" | "UNSUPPORTED" | "INVALID_REQUEST"
            )
        ) {
            return Ok(HomeWrite::Rejected(response_error(code).into()));
        }
        return Err(AppError::Conflict(response_error(code).into()));
    }
    let (owner, _) = read_data(&response)?;
    let acknowledgement = if enabled {
        "policyApplied"
    } else {
        "policyCleared"
    };
    if !owner || response["data"][acknowledgement] != true {
        return Err(AppError::Conflict(
            "小新未确认持久主屏策略已提交，请核实当前结果；不会自动重复设置".into(),
        ));
    }
    Ok(HomeWrite::Applied)
}

/// 保留设置主屏后的读取接口；撤销调用 wait_for_target(enabled=false)。
pub async fn wait_verified<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    limit: Duration,
) -> AppResult<HomeStatus> {
    wait_for_target(device, ip, true, limit).await
}

/// PMS 异步保存策略文件；等待时只读，不重新调用 setHome 或 clearHome。
pub async fn wait_for_target<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    enabled: bool,
    limit: Duration,
) -> AppResult<HomeStatus> {
    let deadline = tokio::time::Instant::now() + limit;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let status = tokio::time::timeout(remaining, query(device, ip))
            .await
            .map_err(|_| {
                AppError::Conflict(
                    "等待系统主屏策略回读超时，请核实当前结果；不会自动重复设置".into(),
                )
            })??;
        if status.matches_target(enabled) {
            return Ok(status);
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(status);
        }
        tokio::time::sleep(remaining.min(Duration::from_millis(500))).await;
        if tokio::time::Instant::now() >= deadline {
            return Ok(status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::device::AndroidTools;
    use super::*;
    use serde_json::json;
    use std::{
        collections::VecDeque,
        path::Path,
        sync::{Arc, Mutex},
    };

    const PERSISTENT: &str = r#"<?xml version="1.0" encoding="utf-8"?><package-restrictions><preferred-activities /><persistent-preferred-activities><item name="chat.xiaoxin.app/.MainActivity"><filter><action name="android.intent.action.MAIN" /><cat name="android.intent.category.HOME" /><cat name="android.intent.category.DEFAULT" /></filter></item></persistent-preferred-activities></package-restrictions>"#;
    const EMPTY_POLICY: &str =
        "<package-restrictions><persistent-preferred-activities /></package-restrictions>";
    const RUNTIME_BEFORE: &str =
        include_str!("../../../tests/fixtures/home_runtime/android10_before_activation.txt");
    const RUNTIME_ACTIVE: &str =
        include_str!("../../../tests/fixtures/home_runtime/android10_after_activation.txt");
    // 4 寸 Android 8.1 实机 dumpsys 的必要系统字段摘录，保留原有行形态和顺序。
    // 分别来自 persistent/before-activity、android81-real-activities、ordinary/before-activity。
    const RUNTIME_LEGACY: &str =
        include_str!("../../../tests/fixtures/home_runtime/android81_real_home_awake.txt");
    const RUNTIME_LEGACY_SLEEPING: &str =
        include_str!("../../../tests/fixtures/home_runtime/android81_real_home_sleeping.txt");
    const RUNTIME_LEGACY_ORDINARY: &str =
        include_str!("../../../tests/fixtures/home_runtime/android81_real_ordinary_home_awake.txt");

    #[test]
    fn runtime_distinguishes_real_field_standard_and_home_tasks() {
        assert!(!parse_runtime(RUNTIME_BEFORE).unwrap().verified());
        let status = parse_runtime(RUNTIME_ACTIVE).unwrap();
        assert!(status.verified());
        assert_eq!(status.pid, Some(10104));
        assert_eq!(status.task_id, Some(22));
        assert_eq!(status.component.as_deref(), Some(COMPONENT));
    }

    #[test]
    fn runtime_legacy_requires_both_task_and_activity_home_types() {
        assert!(!RUNTIME_LEGACY.contains("ActivityStackSupervisor state:"));
        let status = parse_runtime(RUNTIME_LEGACY).unwrap();
        assert!(status.verified());
        assert_eq!(status.pid, Some(14094));
        assert_eq!(status.task_id, Some(526));
        // Stack number is not proof of HOME; nor is a task's destination after finishing.
        for changed in [
            RUNTIME_LEGACY.replace("taskType=1", "taskType=0"),
            RUNTIME_LEGACY.replace(
                "mActivityType=HOME_ACTIVITY_TYPE",
                "mActivityType=APPLICATION_ACTIVITY_TYPE",
            ),
            RUNTIME_LEGACY.replace(
                "mFocusedStack=ActivityStack{1e8d5b8 stackId=0",
                "mFocusedStack=ActivityStack{1e8d5b8 stackId=1",
            ),
        ] {
            assert!(!parse_runtime(&changed).unwrap().verified());
        }
        assert!(
            parse_runtime(
                &RUNTIME_LEGACY
                    .replace("Stack #0", "Stack #7")
                    .replace("StackId=0", "StackId=7")
                    .replace("stackId=0", "stackId=7")
            )
            .unwrap()
            .verified()
        );
    }

    #[test]
    fn runtime_legacy_real_sleeping_is_readable_but_not_active() {
        let status = parse_runtime(RUNTIME_LEGACY_SLEEPING).unwrap();
        assert!(status.home_task);
        assert_eq!(status.pid, Some(13424));
        assert_eq!(status.task_id, Some(523));
        assert!(!status.resumed);
        assert!(!status.verified());
        // 普通默认 HOME 也有 HOME 运行任务；是否持久固定由独立策略读取判定。
        assert!(parse_runtime(RUNTIME_LEGACY_ORDINARY).unwrap().verified());
    }

    #[test]
    fn runtime_legacy_does_not_borrow_global_resumed_or_other_task_evidence() {
        for changed in [
            RUNTIME_LEGACY.replace("    mResumedActivity:", "    mLastPausedActivity:"),
            RUNTIME_LEGACY.replace(
                "mResumedActivity: ActivityRecord{dbb6ffb",
                "mResumedActivity: ActivityRecord{other",
            ),
            RUNTIME_LEGACY.replace(
                "mResumedActivity: ActivityRecord{dbb6ffb u0 chat.xiaoxin.app/.MainActivity t526}",
                "mResumedActivity: ActivityRecord{dbb6ffb u0 chat.xiaoxin.app/.MainActivity t14}",
            ),
            RUNTIME_LEGACY.replace(" U=0 ", " U=10 "),
            RUNTIME_LEGACY.replace(" u0 chat.xiaoxin.app/", " u10 chat.xiaoxin.app/"),
            RUNTIME_LEGACY.replace("Display #0", "Display #1"),
            RUNTIME_LEGACY.replace("Task id #526", "Task id #14"),
            RUNTIME_LEGACY.replace(
                "app=ProcessRecord{ed8d5af 14094:chat.xiaoxin.app/u0a46}",
                "app=null",
            ),
            RUNTIME_LEGACY.replace(
                "app=ProcessRecord{ed8d5af 14094:chat.xiaoxin.app/u0a46}",
                "app=ProcessRecord{ed8d5af 0:chat.xiaoxin.app/u0a46}",
            ),
            RUNTIME_LEGACY.replace(
                "app=ProcessRecord{ed8d5af 14094:chat.xiaoxin.app/u0a46}",
                "app=ProcessRecord{ed8d5af 14094:other.app/u0a46}",
            ),
        ] {
            assert!(!parse_runtime(&changed).is_ok_and(|status| status.verified()));
        }
    }

    #[test]
    fn runtime_requires_complete_recognized_tail_after_activity_evidence() {
        for runtime in [RUNTIME_LEGACY, RUNTIME_ACTIVE] {
            for field in [
                "  mCurTaskIdForUser=",
                "  mUserStackInFront=",
                "KeyguardController:",
                "    mVisibilityTransactionDepth=",
            ] {
                let position = runtime.find(field).unwrap();
                assert!(
                    parse_runtime(&runtime[..position]).is_err(),
                    "不接受截断于 {field} 的输出"
                );
                assert!(parse_runtime(&runtime.replacen(field, "unsupported=", 1)).is_err());
            }
        }
        assert!(
            parse_runtime(&RUNTIME_LEGACY.replace("  mFocusedStack=", "   mFocusedStack="))
                .is_err()
        );
        // 即使第一个主屏任务满足条件，也必须完成后续活动段的格式检查。
        assert!(
            parse_runtime(&RUNTIME_LEGACY.replace("  Stack #1:", "  Stack #1: unknown")).is_err()
        );
    }

    #[test]
    fn runtime_does_not_mix_users_displays_tasks_or_processes() {
        for changed in [
            RUNTIME_ACTIVE.replace("Display #0", "Display #1"),
            RUNTIME_ACTIVE.replace(" U=0 ", " U=10 "),
            RUNTIME_ACTIVE.replace(" u0 chat.xiaoxin.app/", " u10 chat.xiaoxin.app/"),
            RUNTIME_ACTIVE.replace("Task id #22", "Task id #23"),
            RUNTIME_ACTIVE.replace("StackId=0", "StackId=17"),
            RUNTIME_ACTIVE.replace(
                "app=ProcessRecord{82c2a10 10104:chat.xiaoxin.app/u0a106}",
                "app=null",
            ),
            RUNTIME_ACTIVE.replace(
                "app=ProcessRecord{82c2a10 10104:chat.xiaoxin.app/u0a106}",
                "app=ProcessRecord{82c2a10 0:chat.xiaoxin.app/u0a106}",
            ),
            RUNTIME_ACTIVE.replace(
                "app=ProcessRecord{82c2a10 10104:chat.xiaoxin.app/u0a106}",
                "app=ProcessRecord{82c2a10 10104:other.app/u0a106}",
            ),
            RUNTIME_ACTIVE.replace(
                "app=ProcessRecord{82c2a10 10104:chat.xiaoxin.app/u0a106}",
                "app=ProcessRecord{82c2a10 10104:chat.xiaoxin.app/u10a106}",
            ),
        ] {
            assert!(!parse_runtime(&changed).is_ok_and(|status| status.verified()));
        }
    }

    #[test]
    fn runtime_unknown_display_headers_never_inherit_default_display() {
        const DEFAULT_DISPLAY: &str = "Display #0 (activities from top to bottom):";
        for runtime in [RUNTIME_ACTIVE, RUNTIME_LEGACY] {
            for unknown in [
                "Display #1 (activities from top to bottom): unsupported",
                "Display #1:",
                "  Display #1 (activities from top to bottom):",
                "Display #999999999999999999999 (activities from top to bottom):",
            ] {
                // 未知标题后紧跟合法的主屏栈，也不能继承先前的 display=0。
                let changed =
                    runtime.replacen(DEFAULT_DISPLAY, &format!("{DEFAULT_DISPLAY}\n{unknown}"), 1);
                assert!(parse_runtime(&changed).is_err());
                // 解析栈内容时也必须在未知显示屏标题前停止，交给顶层拒绝。
                let changed = runtime.replacen(
                    "    Running activities",
                    &format!("{unknown}\n    Running activities"),
                    1,
                );
                assert!(parse_runtime(&changed).is_err());
            }
            let secondary = runtime.replacen(
                DEFAULT_DISPLAY,
                &format!("{DEFAULT_DISPLAY}\nDisplay #1 (activities from top to bottom):"),
                1,
            );
            assert!(!parse_runtime(&secondary).unwrap().verified());
        }
    }

    #[test]
    fn runtime_requires_same_resumed_record_visible_awake_and_focused() {
        for changed in [
            RUNTIME_ACTIVE.replace("state=RESUMED", "state=PAUSED"),
            RUNTIME_ACTIVE.replace("visible=true", "visible=false"),
            RUNTIME_ACTIVE.replace("  isSleeping=false", "  isSleeping=true"),
            RUNTIME_ACTIVE.replace("    mResumedActivity: ActivityRecord{e426305", "    mResumedActivity: ActivityRecord{different"),
            RUNTIME_ACTIVE.replace("    mResumedActivity: ActivityRecord{e426305 u0 chat.xiaoxin.app/.MainActivity t22}", "    mResumedActivity: ActivityRecord{e426305 u0 chat.xiaoxin.app/.MainActivity t99}"),
            RUNTIME_ACTIVE.replace("topDisplayFocusedStack=ActivityStack{fc4edb9 stackId=0", "topDisplayFocusedStack=ActivityStack{fc4edb9 stackId=17"),
        ] { assert!(!parse_runtime(&changed).unwrap().verified()); }
    }

    #[test]
    fn runtime_unknown_and_incomplete_formats_never_report_success() {
        for text in [
            "",
            "Permission denied",
            "ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)",
        ] {
            assert!(parse_runtime(text).is_err());
        }
        assert!(
            parse_runtime(&RUNTIME_ACTIVE.replace("ActivityStackSupervisor state:", "unknown:"))
                .is_err()
        );
        assert!(
            parse_runtime(&RUNTIME_ACTIVE.replace("Stack #0: type=home", "RootTask #0: type=home"))
                .is_err()
        );
        assert!(
            parse_runtime(&RUNTIME_ACTIVE.replace("topDisplayFocusedStack=", "unsupportedFocus="))
                .is_err()
        );
    }

    #[tokio::test]
    async fn runtime_query_is_system_read_only_and_activation_sends_wakeup_home_once() {
        let (device, commands) = device(vec![Ok(output(RUNTIME_ACTIVE)), Ok(output(""))]);
        assert!(
            query_runtime(&device, "192.0.2.1", CancellationToken::new())
                .await
                .unwrap()
                .verified()
        );
        activate_home(&device, "192.0.2.1", CancellationToken::new())
            .await
            .unwrap();
        let calls = commands.calls.lock().unwrap();
        assert!(calls[0].ends_with(&[
            "shell".into(),
            "dumpsys".into(),
            "activity".into(),
            "activities".into()
        ]));
        assert!(calls[1].ends_with(&[
            "shell".into(),
            "input".into(),
            "keyevent".into(),
            "KEYCODE_WAKEUP".into(),
            "KEYCODE_HOME".into()
        ]));
        assert_eq!(calls.len(), 2);
    }

    #[tokio::test]
    async fn runtime_wait_polls_reads_without_reactivating_or_starting_provider() {
        let (device, commands) =
            device(vec![Ok(output(RUNTIME_BEFORE)), Ok(output(RUNTIME_ACTIVE))]);
        let status = wait_runtime_verified(
            &device,
            "192.0.2.1",
            Duration::from_secs(2),
            CancellationToken::new(),
        )
        .await
        .unwrap();
        assert!(status.verified());
        let calls = commands.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|call| call.ends_with(&[
            "shell".into(),
            "dumpsys".into(),
            "activity".into(),
            "activities".into()
        ])));
    }

    #[tokio::test]
    async fn runtime_timeout_preserves_inactive_and_cancel_never_sends_home() {
        let (device, commands) = device(vec![Ok(output(RUNTIME_BEFORE))]);
        assert!(
            !wait_runtime_verified(
                &device,
                "192.0.2.1",
                Duration::from_millis(200),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .verified()
        );
        assert_eq!(commands.calls.lock().unwrap().len(), 1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            activate_home(&device, "192.0.2.1", cancel.clone()).await,
            Err(AppError::Cancelled)
        ));
        assert!(matches!(
            query_runtime(&device, "192.0.2.1", cancel.clone()).await,
            Err(AppError::Cancelled)
        ));
        assert!(matches!(
            wait_runtime_verified(&device, "192.0.2.1", Duration::from_secs(1), cancel).await,
            Err(AppError::Cancelled)
        ));
        assert_eq!(commands.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn runtime_activation_unknown_response_is_not_replayed() {
        for result in [
            Err(AppError::Conflict("连接中断".into())),
            Ok(output("Error: SecurityException")),
            Ok(output("unexpected result")),
            Ok(ProcessOutput {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
            }),
        ] {
            let (device, commands) = device(vec![result]);
            assert!(
                activate_home(&device, "192.0.2.1", CancellationToken::new())
                    .await
                    .is_err()
            );
            assert_eq!(commands.calls.lock().unwrap().len(), 1);
        }
    }

    #[test]
    fn requires_same_filter_in_persistent_policy_section() {
        assert!(has_persistent_home(PERSISTENT).unwrap());
        assert!(has_persistent_home(&PERSISTENT.replace(SHORT_COMPONENT, COMPONENT)).unwrap());
        assert!(
            !has_persistent_home(
                &PERSISTENT.replace("persistent-preferred-activities", "preferred-activities")
            )
            .unwrap()
        );
        assert!(
            !has_persistent_home(&PERSISTENT.replace(SHORT_COMPONENT, "other.app/.Home")).unwrap()
        );
        assert!(
            !has_persistent_home(
                &PERSISTENT.replace("<cat name=\"android.intent.category.DEFAULT\" />", "")
            )
            .unwrap()
        );
        assert!(
            !has_persistent_home(&PERSISTENT.replace(
                "<cat name=\"android.intent.category.DEFAULT\" />",
                "</filter><filter><cat name=\"android.intent.category.DEFAULT\" />"
            ))
            .unwrap()
        );
        assert!(!has_persistent_home(&PERSISTENT.replace(
            "<cat name=\"android.intent.category.DEFAULT\" />",
            "</filter></item><item name=\"other.app/.Home\"><filter><cat name=\"android.intent.category.DEFAULT\" />"
        )).unwrap());
        assert!(!has_persistent_home(EMPTY_POLICY).unwrap());
        for data in [
            "<scheme name=\"https\" />",
            "<type name=\"text/plain\" />",
            "<unknown-data />",
        ] {
            assert!(
                !has_persistent_home(&PERSISTENT.replace("</filter>", &format!("{data}</filter>")))
                    .unwrap()
            );
        }
    }

    #[test]
    fn incomplete_or_malformed_policy_is_unknown_not_absent() {
        for xml in [
            "",
            "Permission denied",
            "<package-restrictions>",
            "<package-restrictions></wrong>",
            "<other />",
            "<!DOCTYPE package-restrictions><package-restrictions />",
        ] {
            assert!(has_persistent_home(xml).is_err(), "应拒绝不完整策略：{xml}");
        }
        assert!(has_persistent_home(&PERSISTENT.replace("</package-restrictions>", "")).is_err());
    }

    #[test]
    fn resolver_accepts_android_brief_output_and_normalizes_components() {
        assert_eq!(parse_resolved_home(&format!("priority=0 preferredOrder=0 match=0x100000 specificIndex=-1 isDefault=true\n{SHORT_COMPONENT}")).unwrap().as_deref(), Some(COMPONENT));
        assert_eq!(
            parse_resolved_home("other.app/.Home").unwrap(),
            Some("other.app/other.app.Home".into())
        );
        assert_eq!(parse_resolved_home("No activity found").unwrap(), None);
        assert!(parse_resolved_home("").is_err());
        assert!(parse_resolved_home("Error resolving home").is_err());
    }

    fn output(stdout: impl Into<String>) -> ProcessOutput {
        ProcessOutput {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }
    fn response(data: Value) -> ProcessOutput {
        output(format!(
            "Result: Bundle[{{result={}}}]",
            STANDARD.encode(serde_json::to_vec(&data).unwrap())
        ))
    }
    fn read_response(owner: bool, resolved: Option<&str>) -> ProcessOutput {
        response(
            json!({"status":"ok","data":{"protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"deviceOwner":owner,"resolvedHome":resolved}}),
        )
    }
    #[derive(Clone)]
    struct Commands {
        replies: Arc<Mutex<VecDeque<AppResult<ProcessOutput>>>>,
        calls: Arc<Mutex<Vec<Vec<String>>>>,
    }
    impl DeviceCommandPort for Commands {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            self.calls.lock().unwrap().push(args);
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected command")
        }
    }
    fn device(replies: Vec<AppResult<ProcessOutput>>) -> (AdbDevice<Commands>, Commands) {
        let commands = Commands {
            replies: Arc::new(Mutex::new(replies.into())),
            calls: Arc::default(),
        };
        (
            AdbDevice::with_commands(AndroidTools { adb: "adb".into() }, commands.clone()),
            commands,
        )
    }
    fn status_replies(persistent: &str) -> Vec<AppResult<ProcessOutput>> {
        vec![
            Ok(read_response(true, Some(COMPONENT))),
            Ok(output("uid=0(root) gid=0(root)")),
            Ok(output(persistent)),
            Ok(output(SHORT_COMPONENT)),
        ]
    }

    #[test]
    fn cleared_target_keeps_ordinary_default_home_and_still_requires_owner() {
        let mut status = HomeStatus {
            protocol_version: 1,
            package_name: XIAOXIN_PACKAGE.into(),
            device_owner: true,
            persistent_preferred: false,
            resolved_home: Some(COMPONENT.into()),
        };
        assert!(status.matches_target(false));
        assert!(!status.matches_target(true));
        status.resolved_home = None;
        assert!(status.matches_target(false));
        status.device_owner = false;
        assert!(!status.matches_target(false));
        status.device_owner = true;
        status.persistent_preferred = true;
        assert!(!status.matches_target(false));
    }

    #[tokio::test]
    async fn clear_uses_its_own_acknowledgement_and_separate_system_readback() {
        let mut replies = vec![Ok(response(json!({
            "status":"ok", "data": {
                "protocolVersion":1, "packageName":XIAOXIN_PACKAGE,
                "deviceOwner":true, "resolvedHome":COMPONENT, "policyCleared":true
            }
        })))];
        replies.extend(status_replies(EMPTY_POLICY));
        let (device, commands) = device(replies);
        assert!(matches!(
            clear_home(&device, "192.0.2.1").await.unwrap(),
            HomeWrite::Applied
        ));
        let status = query(&device, "192.0.2.1").await.unwrap();
        assert!(status.matches_target(false));
        assert_eq!(status.resolved_home.as_deref(), Some(COMPONENT));
        let calls = commands.calls.lock().unwrap();
        assert!(calls[0].iter().any(|arg| arg == "clearHome"));
        assert_eq!(
            calls
                .iter()
                .flatten()
                .filter(|arg| arg.as_str() == "clearHome")
                .count(),
            1
        );
        assert!(!calls.iter().flatten().any(|arg| arg == "setHome"));
    }

    #[tokio::test]
    async fn clear_rejects_setting_acknowledgement_and_never_retries_unknown_results() {
        for reply in [
            Err(AppError::Conflict("ADB 断开".into())),
            Ok(output("")),
            Ok(response(json!({"status":"failed","code":"UNKNOWN"}))),
            Ok(response(json!({"status":"ok","data":{
                "protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"deviceOwner":true,
                "resolvedHome":COMPONENT,"policyApplied":false
            }}))),
            Ok(response(json!({"status":"ok","data":{
                "protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"deviceOwner":true,
                "resolvedHome":COMPONENT,"policyApplied":true
            }}))),
        ] {
            let (device, commands) = device(vec![reply]);
            assert!(clear_home(&device, "192.0.2.1").await.is_err());
            let calls = commands.calls.lock().unwrap();
            assert_eq!(calls.len(), 1);
            assert!(calls[0].iter().any(|arg| arg == "clearHome"));
        }
        for code in [
            "NOT_DEVICE_OWNER",
            "FORBIDDEN",
            "NOT_READY",
            "UNSUPPORTED",
            "INVALID_REQUEST",
        ] {
            let (device, commands) =
                device(vec![Ok(response(json!({"status":"failed","code":code})))]);
            assert!(matches!(
                clear_home(&device, "192.0.2.1").await.unwrap(),
                HomeWrite::Rejected(_)
            ));
            assert_eq!(commands.calls.lock().unwrap().len(), 1);
        }
    }

    #[tokio::test]
    async fn clear_verification_polls_only_reads_and_does_not_require_another_default_home() {
        let mut replies = status_replies(PERSISTENT);
        replies.extend(status_replies(EMPTY_POLICY));
        let (device, commands) = device(replies);
        let status = wait_for_target(&device, "192.0.2.1", false, Duration::from_secs(2))
            .await
            .unwrap();
        assert!(status.matches_target(false));
        assert_eq!(status.resolved_home.as_deref(), Some(COMPONENT));
        let calls = commands.calls.lock().unwrap();
        assert_eq!(calls.len(), 8);
        assert!(
            !calls
                .iter()
                .flatten()
                .any(|arg| arg == "setHome" || arg == "clearHome")
        );
    }

    #[tokio::test]
    async fn clear_verification_keeps_unconfirmed_state_and_never_writes_after_failure() {
        let (present, commands) = device(status_replies(PERSISTENT));
        let status = wait_for_target(&present, "192.0.2.1", false, Duration::from_millis(10))
            .await
            .unwrap();
        assert!(!status.matches_target(false));
        assert!(
            !commands
                .calls
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .any(|arg| arg == "setHome" || arg == "clearHome")
        );
        let (disconnected, commands) = device(vec![Err(AppError::Conflict("ADB 离线".into()))]);
        assert!(
            wait_for_target(&disconnected, "192.0.2.1", false, Duration::from_secs(1))
                .await
                .is_err()
        );
        assert_eq!(commands.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn query_distinguishes_ordinary_home_and_verified_persistent_home() {
        for (rules, expected) in [
            (PERSISTENT.into(), true),
            (
                PERSISTENT.replace("persistent-preferred-activities", "preferred-activities"),
                false,
            ),
            (EMPTY_POLICY.into(), false),
        ] {
            let (device, commands) = device(status_replies(&rules));
            assert_eq!(
                query(&device, "192.0.2.1").await.unwrap().verified(),
                expected
            );
            let calls = commands.calls.lock().unwrap();
            assert_eq!(calls.len(), 4);
            assert!(calls[0].iter().any(|arg| arg == "readHome"));
            assert!(calls[2].ends_with(&[
                "cat".into(),
                "/data/system/users/0/package-restrictions.xml".into()
            ]));
            assert!(
                calls
                    .iter()
                    .flatten()
                    .all(|arg| !["su", "setHome", "set-home-activity"].contains(&arg.as_str()))
            );
        }
    }
    #[tokio::test]
    async fn query_blocks_missing_owner_old_app_and_zero_exit_error() {
        let cases = vec![
            read_response(false, Some(COMPONENT)),
            output("Could not find provider: chat.xiaoxin.app.workbench.home"),
            output("Error: SecurityException permission denial"),
            response(json!({"status":"failed","code":"NOT_READY"})),
        ];
        for reply in cases {
            let (device, commands) = device(vec![Ok(reply)]);
            assert!(query(&device, "192.0.2.1").await.is_err());
            assert_eq!(commands.calls.lock().unwrap().len(), 1);
        }
    }
    #[tokio::test]
    async fn system_permission_error_and_mismatched_resolver_are_not_success() {
        let (failed, _) = device(vec![
            Ok(read_response(true, Some(COMPONENT))),
            Ok(output("uid=0(root) gid=0(root)")),
            Ok(output("Permission denied")),
        ]);
        assert!(query(&failed, "192.0.2.1").await.is_err());
        let (mismatch, _) = device(vec![
            Ok(read_response(true, Some(COMPONENT))),
            Ok(output("uid=0(root) gid=0(root)")),
            Ok(output(PERSISTENT)),
            Ok(output("other.app/.Home")),
        ]);
        assert!(query(&mismatch, "192.0.2.1").await.is_err());
    }
    #[tokio::test]
    async fn missing_policy_read_permission_blocks_before_any_write() {
        let (device, commands) = device(vec![
            Ok(read_response(true, Some(COMPONENT))),
            Ok(output("uid=2000(shell) gid=2000(shell)")),
            Ok(output("uid=2000(shell) gid=2000(shell)")),
        ]);
        assert!(query(&device, "192.0.2.1").await.is_err());
        let calls = commands.calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert!(
            !calls
                .iter()
                .flatten()
                .any(|arg| arg == "setHome" || arg == "cat")
        );
    }

    #[tokio::test]
    async fn waits_for_async_policy_persistence_using_only_reads() {
        let mut replies = status_replies(EMPTY_POLICY);
        replies.extend(status_replies(PERSISTENT));
        let (device, commands) = device(replies);
        assert!(
            wait_verified(&device, "192.0.2.1", Duration::from_secs(2))
                .await
                .unwrap()
                .verified()
        );
        assert_eq!(commands.calls.lock().unwrap().len(), 8);
        assert!(
            !commands
                .calls
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .any(|arg| arg == "setHome")
        );
    }

    #[tokio::test]
    async fn unpersisted_policy_stays_failed_and_disconnected_verification_is_unknown() {
        let (absent, _) = device(status_replies(EMPTY_POLICY));
        assert!(
            !wait_verified(&absent, "192.0.2.1", Duration::from_millis(10))
                .await
                .unwrap()
                .verified()
        );
        let (disconnected, commands) = device(vec![Err(AppError::Conflict("ADB 离线".into()))]);
        assert!(
            wait_verified(&disconnected, "192.0.2.1", Duration::from_secs(1))
                .await
                .is_err()
        );
        assert_eq!(commands.calls.lock().unwrap().len(), 1);
    }
    #[tokio::test]
    async fn write_acknowledgement_requires_separate_system_readback() {
        for (rules, verified) in [(PERSISTENT, true), (EMPTY_POLICY, false)] {
            let mut replies = vec![Ok(response(
                json!({"status":"ok","data":{"protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"deviceOwner":true,"resolvedHome":COMPONENT,"policyApplied":true}}),
            ))];
            replies.extend(status_replies(rules));
            let (device, _) = device(replies);
            assert!(matches!(
                set_home(&device, "192.0.2.1").await.unwrap(),
                HomeWrite::Applied
            ));
            assert_eq!(
                query(&device, "192.0.2.1").await.unwrap().verified(),
                verified
            );
        }
    }
    #[tokio::test]
    async fn write_distinguishes_definite_rejection_and_unknown_result_without_retry() {
        let (rejected, _) = device(vec![Ok(response(
            json!({"status":"failed","code":"NOT_DEVICE_OWNER"}),
        ))]);
        assert!(matches!(
            set_home(&rejected, "192.0.2.1").await.unwrap(),
            HomeWrite::Rejected(_)
        ));
        for reply in [
            Err(AppError::Conflict("ADB 断开".into())),
            Ok(output("")),
            Ok(response(json!({"status":"failed","code":"INTERNAL_ERROR"}))),
        ] {
            let (device, commands) = device(vec![reply]);
            assert!(set_home(&device, "192.0.2.1").await.is_err());
            assert_eq!(commands.calls.lock().unwrap().len(), 1);
        }
    }
}
