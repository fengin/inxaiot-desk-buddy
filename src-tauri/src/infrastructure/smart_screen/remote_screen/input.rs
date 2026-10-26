use super::super::device::{AdbDevice, AndroidTools};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::rules::{normalize_mac, valid_mac};
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use zeroize::{Zeroize, Zeroizing};

pub const MAX_TEXT_LENGTH: usize = 128;
pub const FRAME_MAX_AGE: Duration = Duration::from_secs(30);
pub type ControlFuture<'a, T> = Pin<Box<dyn Future<Output = AppResult<T>> + Send + 'a>>;

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ControlKey {
    Backspace,
    ClearText,
    Back,
    WakeUp,
}

// 凭据只驻留本次请求内存。故意不实现 Debug/Serialize，不能放入任务快照或日志。
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlAction {
    Tap { x: u32, y: u32 },
    Key { key: ControlKey },
    Text { text: String },
}
impl Drop for ControlAction {
    fn drop(&mut self) {
        if let Self::Text { text } = self {
            text.zeroize();
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlInput {
    pub frame_id: String,
    pub action: ControlAction,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlView {
    pub task_id: String,
    pub idle_timeout_ms: u64,
    pub frame_max_age_ms: u64,
    pub max_text_length: usize,
    pub text_input_behavior: &'static str,
    pub input_generation: u64,
    pub control_mode: &'static str,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionReceipt {
    pub status: String,
    pub message: String,
    pub control_active: bool,
    pub input_generation: u64,
    pub control_mode: &'static str,
}

#[derive(Clone)]
pub struct DeviceFacts {
    pub android_sdk: u32,
    pub awake: Option<bool>,
    pub serial: Option<String>,
    pub mac: Option<String>,
    pub foreground_package: String,
    pub width: u32,
    pub height: u32,
    pub orientation: u32,
}

#[derive(Clone, Copy)]
pub struct ControlSendContext {
    pub android_sdk: u32,
    pub deadline: tokio::time::Instant,
}

pub trait ControlPort: Send + Sync {
    fn probe<'a>(
        &'a self,
        ip: &'a str,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, DeviceFacts>;
    fn send<'a>(
        &'a self,
        ip: &'a str,
        action: &'a ControlAction,
        context: ControlSendContext,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, ()>;
}

pub struct NativeControl {
    pub tools: AndroidTools,
}
impl ControlPort for NativeControl {
    fn probe<'a>(
        &'a self,
        ip: &'a str,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, DeviceFacts> {
        Box::pin(async move {
            let device = AdbDevice::new(self.tools.clone());
            device.connect(ip, cancel.clone()).await?;
            let script = "printf '__REMOTE_SDK__\\n'; getprop ro.build.version.sdk; printf '__REMOTE_SERIAL__\\n'; getprop ro.serialno; printf '__REMOTE_MAC__\\n'; cat /sys/class/net/eth0/address 2>/dev/null; printf '__REMOTE_SIZE__\\n'; wm size; printf '__REMOTE_INPUT__\\n'; dumpsys input; printf '__REMOTE_POWER__\\n'; dumpsys power; printf '__REMOTE_ACTIVITY__\\n'; dumpsys activity activities; printf '__REMOTE_WINDOW__\\n'; dumpsys window";
            let output = device
                .shell(ip, &[script], cancel)
                .await
                .map_err(|_| AppError::Conflict("无法读取屏幕操作状态，请确认连接后重试".into()))?;
            parse_facts(&output)
        })
    }
    fn send<'a>(
        &'a self,
        ip: &'a str,
        action: &'a ControlAction,
        context: ControlSendContext,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, ()> {
        Box::pin(async move {
            let ip = ip
                .parse::<std::net::Ipv4Addr>()
                .map_err(|_| AppError::InvalidConfig("设备地址无效".into()))?;
            validate_action_for_sdk(action, context.android_sdk)?;
            if context.android_sdk >= 29
                && matches!(
                    action,
                    ControlAction::Text { .. }
                        | ControlAction::Key {
                            key: ControlKey::ClearText
                        }
                )
            {
                return helper::send(
                    &self.tools.adb,
                    &ip.to_string(),
                    action,
                    context.deadline,
                    cancel,
                )
                .await;
            }
            let script = encode_action(action)?;
            let remaining = context
                .deadline
                .saturating_duration_since(tokio::time::Instant::now());
            if remaining < Duration::from_secs(3) {
                return Err(AppError::InvalidConfig(
                    "本次操作准备超时，未发送输入，请刷新后重试".into(),
                ));
            }
            super::transport::run_stdin(
                &self.tools.adb,
                &[
                    "-s".into(),
                    format!("{ip}:5555"),
                    "shell".into(),
                    "-T".into(),
                ],
                script.as_bytes(),
                Duration::from_secs(10).min(remaining.saturating_sub(Duration::from_secs(2))),
                cancel,
            )
            .await
        })
    }
}

pub fn text_input_behavior(android_sdk: u32) -> AppResult<&'static str> {
    match android_sdk {
        24..=28 => Ok("insert"),
        29..=100 => Ok("replace"),
        _ => Err(AppError::Conflict(
            "无法确认当前系统的文字输入方式，暂不能辅助操作".into(),
        )),
    }
}

pub fn validate_action_for_sdk(action: &ControlAction, android_sdk: u32) -> AppResult<()> {
    validate_action(action)?;
    let behavior = text_input_behavior(android_sdk)?;
    match action {
        ControlAction::Key {
            key: ControlKey::Backspace,
        } if behavior != "insert" => Err(AppError::InvalidConfig("当前屏请使用清空输入框".into())),
        ControlAction::Key {
            key: ControlKey::ClearText,
        } if behavior != "replace" => Err(AppError::InvalidConfig(
            "当前屏请使用退格或屏端软键盘删除文字".into(),
        )),
        _ => Ok(()),
    }
}

pub(super) fn initial_control_mode(facts: &DeviceFacts) -> AppResult<&'static str> {
    match facts.awake {
        Some(false) => Ok("wake_only"),
        Some(true) if facts.foreground_package == super::super::device::XIAOXIN_PACKAGE => {
            Ok("full")
        }
        Some(true) => Err(AppError::Conflict(
            "小新未在当前前台显示，暂不能辅助操作".into(),
        )),
        None => Err(AppError::Conflict(
            "无法确认屏幕电源状态，请重新读取".into(),
        )),
    }
}

pub fn native_control() -> AppResult<Arc<dyn ControlPort>> {
    Ok(Arc::new(NativeControl {
        tools: AndroidTools::discover()?,
    }))
}

pub fn validate_action(action: &ControlAction) -> AppResult<()> {
    match action {
        ControlAction::Text { text }
            if text.is_empty()
                || text.len() > MAX_TEXT_LENGTH
                || !text.bytes().all(|byte| (0x20..=0x7e).contains(&byte)) =>
        {
            Err(AppError::InvalidConfig(
                "请输入1至128个英文、数字或常用半角符号；暂不支持中文、表情、制表符和换行".into(),
            ))
        }
        ControlAction::Tap { x, y } if *x >= 4096 || *y >= 4096 => {
            Err(AppError::InvalidConfig("点击位置超出屏幕范围".into()))
        }
        _ => Ok(()),
    }
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
pub(super) fn encode_action(action: &ControlAction) -> AppResult<Zeroizing<String>> {
    validate_action(action)?;
    let mut script = Zeroizing::new(String::from("set -e\n"));
    match action {
        ControlAction::Tap { x, y } => script.push_str(&format!("input tap {x} {y}\n")),
        ControlAction::Key { key } => script.push_str(&format!(
            "input keyevent {}\n",
            match key {
                ControlKey::Backspace => 67,
                ControlKey::ClearText =>
                    return Err(AppError::InvalidConfig("清空输入框需要受控文字通道".into())),
                ControlKey::Back => 4,
                ControlKey::WakeUp => 224,
            }
        )),
        ControlAction::Text { text } => {
            // Android input 会把同一参数内的 %s 替换为空格，因此在这一字符对之间拆分。
            // 每段仍是一个经过POSIX单引号保护的参数；不通过本机adb进程参数传递文字。
            let mut rest = text.as_str();
            while let Some(position) = rest.find("%s") {
                let (part, tail) = rest.split_at(position + 1);
                script.push_str("input text ");
                script.push_str(&quote(part));
                script.push('\n');
                rest = tail;
            }
            if !rest.is_empty() {
                script.push_str("input text ");
                script.push_str(&quote(rest));
                script.push('\n');
            }
        }
    }
    script.push_str("exit\n");
    Ok(script)
}

pub(super) fn trusted_serial(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 128
        && value != "0"
        && !value.eq_ignore_ascii_case("unknown")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte)))
    .then(|| value.to_string())
}
fn part<'a>(raw: &'a str, start: &str, end: &str) -> AppResult<&'a str> {
    raw.split_once(start)
        .and_then(|(_, value)| value.split_once(end).map(|(part, _)| part))
        .ok_or_else(|| AppError::Conflict("无法确认屏幕状态，请刷新后重试".into()))
}
pub(super) fn parse_facts(raw: &str) -> AppResult<DeviceFacts> {
    let android_sdk = part(raw, "__REMOTE_SDK__", "__REMOTE_SERIAL__")?
        .trim()
        .parse::<u32>()
        .map_err(|_| AppError::Conflict("未确认屏幕系统版本，请刷新后重试".into()))?;
    text_input_behavior(android_sdk)?;
    let serial = trusted_serial(part(raw, "__REMOTE_SERIAL__", "__REMOTE_MAC__")?);
    let mac = normalize_mac(part(raw, "__REMOTE_MAC__", "__REMOTE_SIZE__")?.trim());
    let mac = valid_mac(&mac).then_some(mac);
    let size = part(raw, "__REMOTE_SIZE__", "__REMOTE_INPUT__")?;
    let input = part(raw, "__REMOTE_INPUT__", "__REMOTE_POWER__")?;
    let power = part(raw, "__REMOTE_POWER__", "__REMOTE_ACTIVITY__")?;
    let power_values: Vec<_> = regex::Regex::new(r"(?m)^\s*mWakefulness\s*=\s*([A-Za-z]+)\s*$")
        .unwrap()
        .captures_iter(power)
        .map(|row| row[1].to_string())
        .collect();
    let awake = match power_values.as_slice() {
        [value] if value == "Awake" => Some(true),
        [value] if value == "Asleep" => Some(false),
        _ => None,
    };
    let dimensions =
        regex::Regex::new(r"(?m)^\s*(Physical|Override) size:\s*(\d+)x(\d+)\s*$").unwrap();
    let mut physical = None;
    let mut overridden = None;
    for row in dimensions.captures_iter(size) {
        let value = (
            row[2].parse::<u32>().unwrap_or(0),
            row[3].parse::<u32>().unwrap_or(0),
        );
        if &row[1] == "Override" {
            overridden = Some(value);
        } else {
            physical = Some(value);
        }
    }
    let (mut width, mut height) = overridden
        .or(physical)
        .ok_or_else(|| AppError::Conflict("未确认屏幕尺寸，请刷新后重试".into()))?;
    let rotations: std::collections::BTreeSet<_> =
        regex::Regex::new(r"SurfaceOrientation:\s*([0-3])\b")
            .unwrap()
            .captures_iter(input)
            .map(|row| row[1].parse::<u32>().unwrap())
            .collect();
    if rotations.len() != 1 || width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(AppError::Conflict("未确认屏幕方向，请刷新后重试".into()));
    }
    if rotations.first().unwrap() % 2 == 1 {
        std::mem::swap(&mut width, &mut height);
    }
    let activity = part(raw, "__REMOTE_ACTIVITY__", "__REMOTE_WINDOW__")?;
    let mut packages = std::collections::BTreeSet::new();
    for name in ["topResumedActivity", "mResumedActivity"] {
        let pattern =
            format!(r"(?m)^\s*{name}\s*[:=]\s*[^\r\n]*?\s([A-Za-z0-9_.]+)/[A-Za-z0-9_.$]+");
        packages = regex::Regex::new(&pattern)
            .unwrap()
            .captures_iter(activity)
            .map(|row| row[1].to_string())
            .collect();
        if regex::Regex::new(&format!(r"(?m)^\s*{name}\s*[:=]"))
            .unwrap()
            .is_match(activity)
        {
            break;
        }
    }
    let windows = raw
        .split_once("__REMOTE_WINDOW__")
        .map(|(_, value)| value)
        .unwrap_or("");
    let focuses: Vec<_> = regex::Regex::new(r"(?m)^\s*mCurrentFocus\s*=\s*([^\r\n]+)")
        .unwrap()
        .captures_iter(windows)
        .map(|row| row[1].to_string())
        .collect();
    let focus_package = if focuses.len() == 1 {
        regex::Regex::new(r"^Window\{[^\r\n]*?\s([A-Za-z0-9_.]+)/[A-Za-z0-9_.$]+")
            .unwrap()
            .captures(&focuses[0])
            .map(|row| row[1].to_string())
    } else {
        None
    };
    let foreground_package = if packages.len() == 1 && packages.first() == focus_package.as_ref() {
        packages.into_iter().next().unwrap()
    } else {
        String::new()
    };
    Ok(DeviceFacts {
        android_sdk,
        awake,
        serial,
        mac,
        foreground_package,
        width,
        height,
        orientation: *rotations.first().unwrap(),
    })
}

mod helper;

#[cfg(test)]
mod tests;
