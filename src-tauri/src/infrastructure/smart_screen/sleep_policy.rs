//! 系统接电保持唤醒策略。检查与核实只读，不经过小新配置组件，也不改变原息屏时长。
use super::device::{AdbDevice, DeviceCommandPort};
use crate::core::error::{AppError, AppResult};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

const SETTING: &str = "stay_on_while_plugged_in";
const SUPPORTED_POWER_SOURCES: u32 = 1 | 2 | 4;
const MAX_SETTING: u64 = i32::MAX as u64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SleepPolicyStatus {
    pub saved: Option<u32>,
    pub saved_raw: String,
    pub effective_mask: u32,
    pub is_powered: bool,
    pub plug_type: u32,
    pub stay_on: bool,
    pub device_admin_enforced: bool,
    pub device_admin_timeout_ms: u64,
    pub wakefulness: String,
    pub display: String,
    pub screen_off_timeout_ms: u64,
}

impl SleepPolicyStatus {
    /// 按当前供电状态判断，不因较大的息屏时长或其他应用的常亮窗口推断已关闭。
    /// 屏幕可能被手动关闭，已满足策略时不要求唤醒或重新写入。
    pub fn already_disabled(&self) -> bool {
        self.check_conditions().is_ok() && self.effective_mask & self.plug_type != 0 && self.stay_on
    }

    pub fn target_mask(&self) -> u32 {
        self.effective_mask | SUPPORTED_POWER_SOURCES
    }

    pub fn needs_wake(&self) -> bool {
        self.wakefulness != "Awake" || self.display != "ON"
    }

    pub fn preflight_message(&self) -> String {
        if let Err(error) = self.check_conditions() {
            return format!("无法关闭屏睡眠：{error}");
        }
        if self.already_disabled() {
            "已关闭自动休眠（接电保持亮屏），本次无需修改，也不唤醒屏幕".into()
        } else {
            "尚未关闭，将设置接电保持唤醒".into()
        }
    }

    pub fn check_conditions(&self) -> AppResult<()> {
        if parse_saved_mask(&self.saved_raw)? != self.saved
            || u64::from(self.effective_mask) > MAX_SETTING
            || self.screen_off_timeout_ms > MAX_SETTING
            || self.device_admin_timeout_ms > i64::MAX as u64
        {
            return Err(conflict("休眠策略检查数据不完整，请重新检查，未修改设置"));
        }
        if self.saved.is_some_and(|saved| saved != self.effective_mask) {
            return Err(conflict(
                "已保存的接电策略与系统生效值不一致，请稍后重新检查，未修改设置",
            ));
        }
        if !self.is_powered {
            return Err(conflict(
                "当前屏未接电，不能确认接电保持亮屏的生效条件，未修改设置",
            ));
        }
        if !matches!(self.plug_type, 1 | 2 | 4) {
            return Err(conflict("当前供电类型未能可靠确认，未修改设置"));
        }
        if self.device_admin_enforced {
            return Err(conflict(
                "设备管理策略限制自动熄屏时间，不能关闭屏睡眠；未修改设备所有者或管理策略",
            ));
        }
        if self.stay_on != (self.effective_mask & self.plug_type != 0) {
            return Err(conflict(
                "接电保持亮屏的设置与系统实际策略尚未一致，请重新检查，未修改设置",
            ));
        }
        if !valid_wakefulness(&self.wakefulness) || !valid_display(&self.display) {
            return Err(conflict("系统显示状态无法可靠确认，未修改设置"));
        }
        Ok(())
    }
}

pub async fn read<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    cancel: CancellationToken,
) -> AppResult<SleepPolicyStatus> {
    let saved = read_output(device, ip, &["settings", "get", "global", SETTING], &cancel).await?;
    let timeout = read_output(
        device,
        ip,
        &["settings", "get", "system", "screen_off_timeout"],
        &cancel,
    )
    .await?;
    let power = read_output(device, ip, &["dumpsys", "power"], &cancel).await?;
    parse_status(&saved, &timeout, &power)
}

/// 使用设备已经具备的身份，不调用 su、adb root 或设备所有者设置。
pub async fn check_write_access<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    cancel: CancellationToken,
) -> AppResult<()> {
    let uid = read_output(device, ip, &["id", "-u"], &cancel).await?;
    match parse_unsigned(&uid, u32::MAX as u64)? {
        0 => Ok(()),
        2000 => {
            let permissions = read_output(
                device,
                ip,
                &["dumpsys", "package", "com.android.shell"],
                &cancel,
            )
            .await?;
            shell_can_write(&permissions)
        }
        _ => Err(conflict(
            "当前设备管理身份没有关闭屏睡眠的权限，未提权或修改设置",
        )),
    }
}

/// 至多写入一次固定全局键；调用方另行只读核实，并决定是否需要一次唤醒。
pub async fn set_mask<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    before: &SleepPolicyStatus,
    cancel: CancellationToken,
) -> AppResult<()> {
    set_mask_with_checkpoint(device, ip, before, cancel, || async { Ok(()) }).await
}

/// 生产调用在所有读取完成后、唯一写命令发出前再次检查共享租约。
pub async fn set_mask_with_checkpoint<P, F, Fut>(
    device: &AdbDevice<P>,
    ip: &str,
    before: &SleepPolicyStatus,
    cancel: CancellationToken,
    before_write: F,
) -> AppResult<()>
where
    P: DeviceCommandPort,
    F: Fn() -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<()>> + Send,
{
    check_cancelled(&cancel)?;
    before.check_conditions()?;
    if before.already_disabled() {
        return Ok(());
    }

    let current = read(device, ip, cancel.clone()).await?;
    current.check_conditions()?;
    if current.saved_raw != before.saved_raw
        || current.effective_mask != before.effective_mask
        || current.is_powered != before.is_powered
        || current.plug_type != before.plug_type
    {
        return Err(conflict(
            "屏幕供电或原接电设置已发生变化，请重新检查，未覆盖现值",
        ));
    }
    check_write_access(device, ip, cancel.clone()).await?;
    // 权限读取也有耗时；写入前再次核对原值，避免覆盖检查后的外部修改。
    let latest = read_output(device, ip, &["settings", "get", "global", SETTING], &cancel).await?;
    if latest != before.saved_raw {
        return Err(conflict(
            "原接电设置在检查后发生变化，请重新检查，未覆盖现值",
        ));
    }
    check_cancelled(&cancel)?;
    before_write().await?;
    check_cancelled(&cancel)?;
    let target = before.target_mask().to_string();
    let output = device
        .adb(
            ip,
            &["shell", "settings", "put", "global", SETTING, &target],
            Duration::from_secs(15),
            cancel.clone(),
        )
        .await?;
    check_cancelled(&cancel)?;
    if !output.success || !output.stdout.trim().is_empty() || !output.stderr.trim().is_empty() {
        return Err(conflict(
            "关闭屏睡眠的设置结果未确认，请只读核实；不会自动重复写入",
        ));
    }
    Ok(())
}

async fn read_output<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    args: &[&str],
    cancel: &CancellationToken,
) -> AppResult<String> {
    check_cancelled(cancel)?;
    let mut command = vec!["shell"];
    command.extend_from_slice(args);
    let output = device
        .adb(ip, &command, Duration::from_secs(15), cancel.clone())
        .await?;
    check_cancelled(cancel)?;
    if !output.success || !output.stderr.trim().is_empty() {
        return Err(conflict(
            "无法可靠读取设备的休眠策略或权限，请检查连接后重新核实",
        ));
    }
    Ok(output.stdout.trim().to_string())
}

fn parse_status(saved: &str, timeout: &str, power: &str) -> AppResult<SleepPolicyStatus> {
    if !power
        .trim_start()
        .starts_with("POWER MANAGER (dumpsys power)")
    {
        return Err(conflict("系统电源状态格式未确认，未修改休眠设置"));
    }
    let admin = field(power, "mMaximumScreenOffTimeoutFromDeviceAdmin")?;
    let admin_pattern = Regex::new(r"^([0-9]+)\s+\(enforced=(true|false)\)$").unwrap();
    let admin = admin_pattern
        .captures(&admin)
        .ok_or_else(|| conflict("设备管理休眠约束无法可靠解析，未修改设置"))?;
    let display = unique_value(
        power
            .lines()
            .filter_map(|line| line.trim().strip_prefix("Display Power: state="))
            .map(|value| value.trim().to_string()),
    )?;
    let wakefulness = field(power, "mWakefulness")?;
    if !valid_wakefulness(&wakefulness) || !valid_display(&display) {
        return Err(conflict("系统显示状态无法可靠解析，未修改设置"));
    }
    Ok(SleepPolicyStatus {
        saved: parse_saved_mask(saved)?,
        saved_raw: saved.trim().to_string(),
        effective_mask: parse_unsigned(&field(power, "mStayOnWhilePluggedInSetting")?, MAX_SETTING)?
            as u32,
        is_powered: parse_bool(&field(power, "mIsPowered")?)?,
        plug_type: parse_unsigned(&field(power, "mPlugType")?, u32::MAX as u64)? as u32,
        stay_on: parse_bool(&field(power, "mStayOn")?)?,
        device_admin_enforced: parse_bool(&admin[2])?,
        device_admin_timeout_ms: parse_unsigned(&admin[1], i64::MAX as u64)?,
        wakefulness,
        display,
        screen_off_timeout_ms: parse_unsigned(timeout, MAX_SETTING)?,
    })
}

fn field(text: &str, name: &str) -> AppResult<String> {
    let prefix = format!("{name}=");
    unique_value(
        text.lines()
            .filter_map(|line| line.trim().strip_prefix(prefix.as_str()))
            .map(|value| value.trim().to_string()),
    )
}

fn unique_value(values: impl Iterator<Item = String>) -> AppResult<String> {
    let values = values.collect::<BTreeSet<_>>();
    if values.len() != 1 {
        return Err(conflict(
            "系统休眠状态缺少关键字段或返回相互矛盾的值，未修改设置",
        ));
    }
    Ok(values.into_iter().next().unwrap())
}

fn parse_saved_mask(raw: &str) -> AppResult<Option<u32>> {
    if raw.trim() == "null" {
        Ok(None)
    } else {
        Ok(Some(parse_unsigned(raw, MAX_SETTING)? as u32))
    }
}

fn parse_unsigned(raw: &str, maximum: u64) -> AppResult<u64> {
    let text = raw.trim();
    if text.is_empty() || !text.bytes().all(|character| character.is_ascii_digit()) {
        return Err(conflict("系统休眠状态的整数值无效，未修改设置"));
    }
    text.parse::<u64>()
        .ok()
        .filter(|value| *value <= maximum)
        .ok_or_else(|| conflict("系统休眠状态的整数值超出范围，未修改设置"))
}

fn parse_bool(text: &str) -> AppResult<bool> {
    match text {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(conflict("系统休眠状态的布尔值无效，未修改设置")),
    }
}

fn valid_wakefulness(value: &str) -> bool {
    matches!(value, "Awake" | "Asleep" | "Dreaming" | "Dozing")
}

fn valid_display(value: &str) -> bool {
    matches!(
        value,
        "ON" | "OFF" | "DOZE" | "DOZE_SUSPEND" | "ON_SUSPEND" | "VR"
    )
}

fn shell_can_write(text: &str) -> AppResult<()> {
    if !text.contains("Package [com.android.shell]") {
        return Err(conflict("无法核实设备管理账号的设置权限，未修改设置"));
    }
    let pattern = Regex::new(r"^granted=(true|false)(?:,.*)?$").unwrap();
    let grants = text
        .lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("android.permission.WRITE_SECURE_SETTINGS:")
        })
        .map(|value| {
            pattern
                .captures(value.trim())
                .map(|capture| capture[1].to_string())
                .ok_or_else(|| conflict("设备管理账号的设置权限格式异常，未修改设置"))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let grant = unique_value(grants.into_iter())?;
    if grant != "true" {
        return Err(conflict(
            "当前设备管理账号没有修改系统全局设置的权限，未提权或修改设置",
        ));
    }
    Ok(())
}

fn check_cancelled(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn conflict(message: &str) -> AppError {
    AppError::Conflict(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::smart_screen::device::{AndroidTools, ProcessOutput};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const IP: &str = "192.0.2.74";
    const GRANTED: &str = "Packages:\n  Package [com.android.shell] (test):\n    android.permission.WRITE_SECURE_SETTINGS: granted=true\n";

    fn power(mask: u32, stay_on: bool, admin_timeout: u64) -> String {
        format!(
            "POWER MANAGER (dumpsys power)\n  mWakefulness=Awake\n  mIsPowered=true\n  mPlugType=1\n  mStayOnWhilePluggedInSetting={mask}\n  mStayOn={stay_on}\n  mMaximumScreenOffTimeoutFromDeviceAdmin={admin_timeout} (enforced=false)\nDisplay Power: state=ON\n"
        )
    }

    struct FakeState {
        saved: String,
        timeout: String,
        power: String,
        uid: String,
        permissions: String,
        commands: Vec<Vec<String>>,
        global_reads: usize,
        replace_on_global_read: Option<(usize, String)>,
        cancel_on_uid: Option<CancellationToken>,
        write_output: ProcessOutput,
    }

    #[derive(Clone)]
    struct FakeCommands(Arc<Mutex<FakeState>>);

    impl FakeCommands {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(FakeState {
                saved: "0".into(),
                timeout: "60000".into(),
                power: power(0, false, i32::MAX as u64),
                uid: "0".into(),
                permissions: GRANTED.into(),
                commands: Vec::new(),
                global_reads: 0,
                replace_on_global_read: None,
                cancel_on_uid: None,
                write_output: ProcessOutput {
                    success: true,
                    stdout: String::new(),
                    stderr: String::new(),
                },
            })))
        }

        fn device(&self) -> AdbDevice<Self> {
            AdbDevice::with_commands(
                AndroidTools {
                    adb: PathBuf::from("test-adb"),
                },
                self.clone(),
            )
        }

        fn writes(&self) -> Vec<Vec<String>> {
            self.0
                .lock()
                .unwrap()
                .commands
                .iter()
                .filter(|command| {
                    command.get(3).is_some_and(|value| value == "settings")
                        && command.get(4).is_some_and(|value| value == "put")
                })
                .cloned()
                .collect()
        }
    }

    impl DeviceCommandPort for FakeCommands {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            cancel: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            check_cancelled(&cancel)?;
            let mut state = self.0.lock().unwrap();
            state.commands.push(args.clone());
            let command = args[3..].iter().map(String::as_str).collect::<Vec<_>>();
            let stdout = match command.as_slice() {
                ["settings", "get", "global", SETTING] => {
                    state.global_reads += 1;
                    if let Some((index, replacement)) = &state.replace_on_global_read {
                        if state.global_reads == *index {
                            state.saved = replacement.clone();
                        }
                    }
                    state.saved.clone()
                }
                ["settings", "get", "system", "screen_off_timeout"] => state.timeout.clone(),
                ["dumpsys", "power"] => state.power.clone(),
                ["id", "-u"] => {
                    if let Some(token) = &state.cancel_on_uid {
                        token.cancel();
                    }
                    state.uid.clone()
                }
                ["dumpsys", "package", "com.android.shell"] => state.permissions.clone(),
                ["settings", "put", "global", SETTING, value] => {
                    state.saved = (*value).into();
                    return Ok(state.write_output.clone());
                }
                _ => panic!("休眠策略发出了未授权测试命令：{command:?}"),
            };
            Ok(ProcessOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }

    #[test]
    fn android_81_and_10_states_keep_timeout_separate_from_sleep_policy() {
        for (timeout, admin_max) in [("60000", i32::MAX as u64), ("2147483647", i64::MAX as u64)] {
            let status = parse_status("0", timeout, &power(0, false, admin_max)).unwrap();
            assert!(!status.already_disabled());
            assert_eq!(status.target_mask(), 7);
            assert_eq!(
                status.screen_off_timeout_ms,
                timeout.parse::<u64>().unwrap()
            );
            assert!(status.preflight_message().contains("尚未关闭"));
        }
    }

    #[test]
    fn android_81_and_10_power_reads_accept_crlf_and_mixed_line_endings() {
        for (saved, timeout, admin_max) in [
            (0, "60000", i32::MAX as u64),
            (7, "2147483647", i64::MAX as u64),
        ] {
            let lf = power(saved, saved != 0, admin_max);
            let mixed = lf
                .lines()
                .enumerate()
                .map(|(index, line)| {
                    format!("{line}{}", if index % 2 == 0 { "\r\n" } else { "\n" })
                })
                .collect::<String>();
            for text in [lf.replace('\n', "\r\n"), mixed] {
                let status = parse_status(&format!("{saved}\r\n"), timeout, &text).unwrap();
                status.check_conditions().unwrap();
                assert_eq!(status.effective_mask, saved);
                assert_eq!(status.device_admin_timeout_ms, admin_max);
                assert_eq!(status.wakefulness, "Awake");
                assert_eq!(status.display, "ON");
                assert_eq!(status.already_disabled(), saved != 0);
            }
        }
    }

    #[test]
    fn repeated_crlf_fields_must_be_consistent_and_never_hide_invalid_values() {
        let base = power(7, true, i64::MAX as u64).replace('\n', "\r\n");
        let same = format!("{base}  mIsPowered=true\nDisplay Power: state=ON\r\n");
        assert!(
            parse_status("7", "2147483647", &same)
                .unwrap()
                .already_disabled()
        );
        for extra in [
            "  mIsPowered=false\r\n",
            "  mIsPowered=unknown\n",
            "Display Power: state=OFF\r\n",
            "Display Power: state=UNKNOWN\n",
        ] {
            assert!(parse_status("7", "2147483647", &format!("{base}{extra}")).is_err());
        }
    }

    #[test]
    fn shell_permission_crlf_and_mixed_lines_preserve_grant_validation() {
        let crlf = GRANTED.replace('\n', "\r\n");
        for text in [
            crlf.clone(),
            format!("{crlf}    android.permission.WRITE_SECURE_SETTINGS: granted=true\n"),
            format!(
                "{crlf}    android.permission.WRITE_SECURE_SETTINGS: granted=true, flags=[ SYSTEM_FIXED ]\r\n"
            ),
        ] {
            shell_can_write(&text).unwrap();
        }
        for suffix in [
            "    android.permission.WRITE_SECURE_SETTINGS: granted=false\r\n",
            "    android.permission.WRITE_SECURE_SETTINGS: granted=unknown\n",
        ] {
            assert!(shell_can_write(&format!("{crlf}{suffix}")).is_err());
        }
    }

    #[test]
    fn current_power_source_is_enough_and_manual_screen_off_does_not_require_rewrite() {
        let text = power(1, true, i32::MAX as u64)
            .replace("mWakefulness=Awake", "mWakefulness=Asleep")
            .replace("state=ON", "state=OFF");
        let status = parse_status("1", "60000", &text).unwrap();
        assert!(status.already_disabled());
        assert!(status.needs_wake());
        assert!(status.preflight_message().contains("不唤醒"));
    }

    #[test]
    fn absent_setting_uses_effective_default_and_preserves_other_bits() {
        let status = parse_status("null", "60000", &power(8, false, i64::MAX as u64)).unwrap();
        assert_eq!(status.saved, None);
        assert_eq!(status.target_mask(), 15);
        assert!(status.check_conditions().is_ok());
    }

    #[test]
    fn no_power_admin_constraints_and_inconsistent_runtime_are_not_writable() {
        let cases = [
            power(0, false, i32::MAX as u64)
                .replace("mIsPowered=true", "mIsPowered=false")
                .replace("mPlugType=1", "mPlugType=0"),
            power(0, false, 60000).replace("enforced=false", "enforced=true"),
            power(0, false, i32::MAX as u64).replace("mPlugType=1", "mPlugType=8"),
            power(7, false, i32::MAX as u64),
            power(0, true, i32::MAX as u64),
        ];
        for text in cases {
            let raw = if text.contains("mStayOnWhilePluggedInSetting=7") {
                "7"
            } else {
                "0"
            };
            let status = parse_status(raw, "60000", &text).unwrap();
            assert!(status.check_conditions().is_err());
            assert!(!status.already_disabled());
        }
        let status = parse_status("0", "60000", &power(7, true, i32::MAX as u64)).unwrap();
        assert!(status.check_conditions().is_err());
        assert!(!status.already_disabled());
    }

    #[test]
    fn malformed_missing_or_conflicting_output_never_defaults_to_safe_values() {
        let base = power(0, false, i32::MAX as u64);
        let malformed = [
            String::new(),
            base.replace("mIsPowered=true", "mIsPowered=unknown"),
            base.replace("mStayOn=false", "mStayOn=0"),
            base.replace("mPlugType=1", "mPlugType=not-a-number"),
            base.replace(
                "mStayOnWhilePluggedInSetting=0",
                "mStayOnWhilePluggedInSetting=-1",
            ),
            base.replace(" (enforced=false)", ""),
            base.replace("mWakefulness=Awake", "mWakefulness=Unknown"),
            base.replace("state=ON", "state=UNKNOWN"),
            base.replace("  mStayOn=false\n", ""),
            format!("{base}  mStayOn=true\n"),
            format!("{base}Display Power: state=OFF\n"),
        ];
        for text in malformed {
            assert!(
                parse_status("0", "60000", &text).is_err(),
                "异常状态应拒绝：{text}"
            );
        }
        for saved in ["", "-1", "garbage", "2147483648", "0\n7"] {
            assert!(parse_status(saved, "60000", &base).is_err());
        }
    }

    #[tokio::test]
    async fn already_disabled_skips_permission_checks_and_all_device_commands() {
        let fake = FakeCommands::new();
        let status = parse_status("1", "60000", &power(1, true, i32::MAX as u64)).unwrap();
        set_mask(&fake.device(), IP, &status, CancellationToken::new())
            .await
            .unwrap();
        assert!(fake.0.lock().unwrap().commands.is_empty());
    }

    #[tokio::test]
    async fn shell_permission_is_read_only_and_conflicting_or_missing_grants_are_rejected() {
        for (permissions, allowed) in [
            (GRANTED.to_string(), true),
            (
                format!("{GRANTED}    android.permission.WRITE_SECURE_SETTINGS: granted=true\n"),
                true,
            ),
            (GRANTED.replace("granted=true", "granted=false"), false),
            (
                format!("{GRANTED}    android.permission.WRITE_SECURE_SETTINGS: granted=false\n"),
                false,
            ),
            ("Package [com.android.shell] (test):\n".into(), false),
            (
                GRANTED.replace("Package [com.android.shell]", "Package [other.package]"),
                false,
            ),
        ] {
            let fake = FakeCommands::new();
            {
                let mut state = fake.0.lock().unwrap();
                state.uid = "2000".into();
                state.permissions = permissions;
            }
            let result = check_write_access(&fake.device(), IP, CancellationToken::new()).await;
            assert_eq!(result.is_ok(), allowed);
            assert!(fake.writes().is_empty());
            assert_eq!(fake.0.lock().unwrap().commands.len(), 2);
        }
    }

    #[tokio::test]
    async fn root_passes_without_elevation_and_other_uids_are_denied() {
        for (uid, allowed) in [
            ("0", true),
            ("10001", false),
            ("uid=0(root)", false),
            ("", false),
        ] {
            let fake = FakeCommands::new();
            fake.0.lock().unwrap().uid = uid.into();
            assert_eq!(
                check_write_access(&fake.device(), IP, CancellationToken::new())
                    .await
                    .is_ok(),
                allowed
            );
            assert_eq!(fake.0.lock().unwrap().commands.len(), 1);
            assert!(fake.writes().is_empty());
        }
    }

    #[tokio::test]
    async fn cancellation_before_read_or_after_permissions_never_writes() {
        let fake = FakeCommands::new();
        let token = CancellationToken::new();
        token.cancel();
        assert!(matches!(
            read(&fake.device(), IP, token.clone()).await,
            Err(AppError::Cancelled)
        ));
        let status = parse_status("0", "60000", &power(0, false, i32::MAX as u64)).unwrap();
        assert!(matches!(
            set_mask(&fake.device(), IP, &status, token).await,
            Err(AppError::Cancelled)
        ));
        assert!(fake.0.lock().unwrap().commands.is_empty());
        let token = CancellationToken::new();
        fake.0.lock().unwrap().cancel_on_uid = Some(token.clone());
        assert!(matches!(
            set_mask(&fake.device(), IP, &status, token).await,
            Err(AppError::Cancelled)
        ));
        assert!(fake.writes().is_empty());
    }

    #[tokio::test]
    async fn changed_original_value_before_write_is_not_overwritten() {
        for at in [1, 2] {
            let fake = FakeCommands::new();
            fake.0.lock().unwrap().replace_on_global_read = Some((at, "8".into()));
            let status = parse_status("0", "60000", &power(0, false, i32::MAX as u64)).unwrap();
            assert!(
                set_mask(&fake.device(), IP, &status, CancellationToken::new())
                    .await
                    .is_err()
            );
            assert!(fake.writes().is_empty());
        }
    }

    #[tokio::test]
    async fn only_one_fixed_global_key_is_written_and_long_timeout_is_untouched() {
        for saved in ["0", "null", "8"] {
            let fake = FakeCommands::new();
            let effective = if saved == "8" { 8 } else { 0 };
            {
                let mut state = fake.0.lock().unwrap();
                state.saved = saved.into();
                state.timeout = "2147483647".into();
                state.power = power(effective, false, i64::MAX as u64);
                state.uid = "2000".into();
            }
            let status = read(&fake.device(), IP, CancellationToken::new())
                .await
                .unwrap();
            set_mask(&fake.device(), IP, &status, CancellationToken::new())
                .await
                .unwrap();
            let writes = fake.writes();
            assert_eq!(writes.len(), 1);
            assert_eq!(
                writes[0][3..],
                [
                    "settings",
                    "put",
                    "global",
                    SETTING,
                    &status.target_mask().to_string()
                ]
            );
            let state = fake.0.lock().unwrap();
            assert_eq!(state.timeout, "2147483647");
            assert!(
                !state
                    .commands
                    .iter()
                    .any(|args| args.iter().any(|value| matches!(
                        value.as_str(),
                        "su" | "root" | "reboot" | "input" | "content"
                    )))
            );
        }
    }

    #[tokio::test]
    async fn ambiguous_write_response_is_not_retried_or_reported_as_confirmed() {
        let fake = FakeCommands::new();
        fake.0.lock().unwrap().write_output.stdout = "Permission denial".into();
        let status = parse_status("0", "60000", &power(0, false, i32::MAX as u64)).unwrap();
        assert!(
            set_mask(&fake.device(), IP, &status, CancellationToken::new())
                .await
                .is_err()
        );
        assert_eq!(fake.writes().len(), 1);
    }

    #[tokio::test]
    async fn lost_final_write_checkpoint_never_writes_settings() {
        let fake = FakeCommands::new();
        let status = parse_status("0", "60000", &power(0, false, i32::MAX as u64)).unwrap();
        let result = set_mask_with_checkpoint(
            &fake.device(),
            IP,
            &status,
            CancellationToken::new(),
            || async { Err(AppError::Conflict("目标租约已失效".into())) },
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("租约已失效"));
        assert!(fake.writes().is_empty());
    }

    #[tokio::test]
    async fn cancellation_in_final_write_checkpoint_never_writes_settings() {
        let fake = FakeCommands::new();
        let status = parse_status("0", "60000", &power(0, false, i32::MAX as u64)).unwrap();
        let token = CancellationToken::new();
        let result = set_mask_with_checkpoint(&fake.device(), IP, &status, token.clone(), || {
            token.cancel();
            async { Ok(()) }
        })
        .await;
        assert!(matches!(result, Err(AppError::Cancelled)));
        assert!(fake.writes().is_empty());
    }
}
