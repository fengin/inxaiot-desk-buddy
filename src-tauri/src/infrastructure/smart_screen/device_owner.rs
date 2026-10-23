//! 只设置固定的小新设备所有者，不支持删除、替换或重置设备。
use super::device::{AdbDevice, DeviceCommandPort};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::app_config::AppOwnerRead;
use tokio_util::sync::CancellationToken;

pub const ADMIN: &str = "chat.xiaoxin.app/chat.xiaoxin.app.kiosk.XiaoXinDeviceAdminReceiver";

pub fn unknown(message: impl Into<String>) -> AppOwnerRead {
    AppOwnerRead { state: "unknown".into(), component: None, message: message.into() }
}

pub fn parse(text: &str) -> AppOwnerRead {
    let lines = text.lines().map(str::trim).collect::<Vec<_>>();
    let known_dump = lines.iter().any(|s| *s == "Current Device Policy Manager state:")
        && lines.iter().any(|s| s.starts_with("Enabled Device Admins") || s.starts_with("User ")
            || s.starts_with("Device policy cache:") || s.starts_with("Device Owner:"));
    let owners = lines.iter().enumerate().filter(|(_, line)| line.starts_with("Device Owner:")).collect::<Vec<_>>();
    if !known_dump || owners.len() > 1 { return unknown("系统设备所有者格式未确认，请重新读取"); }
    let Some((index, heading)) = owners.first() else {
        if lines.iter().any(|s| s.contains("Device Owner") || s.contains("mDeviceOwner")) {
            return unknown("系统设备所有者字段无法确认，请重新读取");
        }
        return AppOwnerRead { state: "none".into(), component: None, message: "系统未设置设备所有者".into() };
    };
    if matches!(heading.trim_start_matches("Device Owner:").trim(), "null" | "none") {
        return AppOwnerRead { state: "none".into(), component: None, message: "系统未设置设备所有者".into() };
    }
    let component = regex::Regex::new(r"admin=ComponentInfo\{([A-Za-z][A-Za-z0-9_.]*/[A-Za-z.][A-Za-z0-9_.$]*)\}").unwrap();
    let block = lines[*index..].iter().take(12).take_while(|line| !line.starts_with("Profile Owner") && !line.starts_with("Enabled Device Admins")).copied().collect::<Vec<_>>().join("\n");
    let Some(captures) = component.captures(&block) else { return unknown("设备所有者组件未确认，请重新读取") };
    if !regex::Regex::new(r"(?m)^User ID:\s*0\s*$").unwrap().is_match(&block) {
        return unknown("设备所有者所属用户未确认，未执行设置");
    }
    let raw = &captures[1];
    let (package, class) = raw.split_once('/').unwrap();
    let normalized = format!("{package}/{}", if class.starts_with('.') { format!("{package}{class}") } else { class.into() });
    let state = if normalized == ADMIN { "self" } else { "other" };
    AppOwnerRead { state: state.into(), component: Some(normalized), message: if state == "self" {
        "小新已是设备所有者，无需重复设置".into()
    } else { "设备已有其他所有者，不能替换或移除".into() } }
}

pub async fn read<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppOwnerRead {
    match device.shell(ip, &["dumpsys", "device_policy"], CancellationToken::new()).await {
        Ok(text) => parse(&text),
        Err(_) => unknown("设备所有者读取失败，请检查管理连接后重读"),
    }
}

pub fn require(owner: &AppOwnerRead, ensure: bool) -> AppResult<()> {
    match owner.state.as_str() {
        "self" => Ok(()),
        "none" if ensure => Ok(()),
        "none" => Err(AppError::Conflict("请先设置小新为设备所有者，或勾选本次设置".into())),
        "other" => Err(AppError::Conflict("设备已有其他所有者，未执行主屏或锁定操作".into())),
        _ => Err(AppError::Conflict("设备所有者未确认，未执行主屏或锁定操作".into())),
    }
}

pub async fn ensure<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<()> {
    let current = read(device, ip).await;
    require(&current, true)?;
    if current.state == "self" { return Ok(()) }
    // 唯一获准的系统写命令；响应后仍由调用者独立读取确认，不据输出猜成功。
    device.shell(ip, &["dpm", "set-device-owner", "--user", "0", ADMIN], CancellationToken::new()).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::device::{AndroidTools, ProcessOutput};
    use std::{path::Path, sync::{Arc, Mutex}, time::Duration};

    #[derive(Clone)]
    struct Commands { policy: String, calls: Arc<Mutex<Vec<Vec<String>>>> }
    impl DeviceCommandPort for Commands {
        async fn execute(&self, _: &Path, args: Vec<String>, _: Duration, _: CancellationToken) -> AppResult<ProcessOutput> {
            let stdout = if args.ends_with(&["dumpsys".into(), "device_policy".into()]) { self.policy.clone() }
                else { assert_eq!(&args[3..], &["dpm", "set-device-owner", "--user", "0", ADMIN]); "Success".into() };
            self.calls.lock().unwrap().push(args);
            Ok(ProcessOutput { success: true, stdout, stderr: String::new() })
        }
    }
    #[tokio::test]
    async fn ensure_is_idempotent_and_only_writes_fixed_admin_after_confirmed_none() {
        let prefix = "Current Device Policy Manager state:\nEnabled Device Admins (User 0):\n";
        for (policy, permitted, writes) in [
            (prefix.to_string(), true, 1),
            (format!("{prefix}Device Owner:\nadmin=ComponentInfo{{{ADMIN}}}\nUser ID: 0"), true, 0),
            (format!("{prefix}Device Owner:\nadmin=ComponentInfo{{other.app/.Admin}}\nUser ID: 0"), false, 0),
            ("permission denied".into(), false, 0),
        ] {
            let commands = Commands { policy, calls: Arc::default() };
            let device = AdbDevice::with_commands(AndroidTools { adb: "mock-never-executed".into() }, commands.clone());
            assert_eq!(ensure(&device, "192.0.2.1").await.is_ok(), permitted);
            let calls = commands.calls.lock().unwrap();
            assert_eq!(calls.iter().filter(|args| args.iter().any(|arg| arg == "dpm")).count(), writes);
            assert!(calls.iter().all(|args| !args.iter().any(|arg| arg.contains("remove") || arg.contains("wipe"))));
        }
    }
    #[test]
    fn owner_four_states_ignore_active_admins_and_profile_owners() {
        let prefix = "Current Device Policy Manager state:\nEnabled Device Admins (User 0):\n  admin=ComponentInfo{chat.xiaoxin.app/.kiosk.XiaoXinDeviceAdminReceiver}\n";
        assert_eq!(parse(prefix).state, "none");
        assert_eq!(parse(&format!("{prefix}Profile Owner (User 10):\nadmin=ComponentInfo{{other.app/.Admin}}\n")).state, "none");
        assert_eq!(parse(&format!("{prefix}Device Owner:\nadmin=ComponentInfo{{chat.xiaoxin.app/.kiosk.XiaoXinDeviceAdminReceiver}}\nUser ID: 0")).state, "self");
        assert_eq!(parse(&format!("{prefix}Device Owner:\nadmin=ComponentInfo{{other.app/.Admin}}\nUser ID: 0")).state, "other");
        for bad in ["", "permission denied", "Current Device Policy Manager state:", "Device Owner:\nadmin=broken"] { assert_eq!(parse(bad).state, "unknown"); }
    }
    #[test]
    fn only_none_with_explicit_intent_or_self_permit_dependent_actions() {
        for state in ["none", "other", "unknown"] {
            let owner = AppOwnerRead { state: state.into(), component: None, message: String::new() };
            assert!(require(&owner, false).is_err());
            assert_eq!(require(&owner, true).is_ok(), state == "none");
        }
    }
}
