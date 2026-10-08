use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::{
    model::{ScreenAsset, ScreenObservation},
    rules::{normalize_mac, valid_mac},
};
use crate::infrastructure::local_sqlite::screen_repository::now;
use regex::Regex;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub const XIAOXIN_PACKAGE: &str = "chat.xiaoxin.app";

#[derive(Clone, Debug)]
pub struct AndroidTools {
    pub adb: PathBuf,
}
impl AndroidTools {
    pub fn discover() -> AppResult<Self> {
        let adb_name = super::tool_bundle::executable_name("adb");
        if let Some(root) = super::tool_bundle::current()? {
            return Ok(Self { adb: root.join("android").join(&adb_name) });
        }
        if let Some(path) = std::env::var_os("INX_ADB_PATH").map(PathBuf::from) {
            if path.is_file() {
                return Ok(Self { adb: path });
            }
            return Err(AppError::InvalidConfig("配置的 ADB 工具不存在".into()));
        }
        if let Ok(exe) = std::env::current_exe() {
            let path = exe
                .parent()
                .unwrap_or(Path::new("."))
                .join("tools/android").join(&adb_name);
            if path.is_file() {
                return Ok(Self { adb: path });
            }
        }
        for variable in ["ANDROID_SDK_ROOT", "ANDROID_HOME"] {
            if let Some(root) = std::env::var_os(variable) {
                let path = PathBuf::from(root).join("platform-tools").join(&adb_name);
                if path.is_file() {
                    return Ok(Self { adb: path });
                }
            }
        }
        if let Some(path) = find_on_path(&adb_name) {
            return Ok(Self { adb: path });
        }
        Err(AppError::NotFound(
            "未找到 Android 管理工具，请配置 ADB 工具后重试".into(),
        ))
    }
}
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|path| path.join(name))
        .find(|path| path.is_file())
}

#[derive(Clone, Debug)]
pub struct ProcessOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}
pub trait DeviceCommandPort: Send + Sync {
    fn execute(
        &self,
        program: &Path,
        args: Vec<String>,
        limit: Duration,
        cancel: CancellationToken,
    ) -> impl std::future::Future<Output = AppResult<ProcessOutput>> + Send;
}
#[derive(Clone)]
pub struct NativeDeviceCommands;
impl DeviceCommandPort for NativeDeviceCommands {
    async fn execute(
        &self,
        program: &Path,
        args: Vec<String>,
        limit: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ProcessOutput> {
        run(program, &args, limit, cancel).await
    }
}
pub async fn run(
    program: &Path,
    args: &[String],
    limit: Duration,
    cancel: CancellationToken,
) -> AppResult<ProcessOutput> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let mut command = tokio::process::Command::new(program);
    command.args(args).kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let output = tokio::select! {
        _=cancel.cancelled()=>return Err(AppError::Cancelled),
        result=tokio::time::timeout(limit,command.output())=>match result {
            Err(_)=>return Err(AppError::timeout("等待设备命令")),
            Ok(Err(error))=>return Err(AppError::io("启动设备工具",&error)),
            Ok(Ok(output))=>output
        }
    };
    // 固定的读取命令应输出少量数据，拒绝异常的大结果。
    if output.stdout.len() + output.stderr.len() > 8 * 1024 * 1024 {
        return Err(AppError::Conflict(
            "设备返回数据过大，请单独采集诊断".into(),
        ));
    }
    Ok(ProcessOutput {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

#[derive(Clone)]
pub struct AdbDevice<P = NativeDeviceCommands> {
    pub tools: AndroidTools,
    commands: P,
}
impl AdbDevice<NativeDeviceCommands> {
    pub fn new(tools: AndroidTools) -> Self {
        Self {
            tools,
            commands: NativeDeviceCommands,
        }
    }
}
impl<P: DeviceCommandPort> AdbDevice<P> {
    pub fn with_commands(tools: AndroidTools, commands: P) -> Self {
        Self { tools, commands }
    }
    pub async fn adb(
        &self,
        ip: &str,
        args: &[&str],
        timeout: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ProcessOutput> {
        let ip = ip
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| AppError::InvalidConfig("设备 IP 必须是有效 IPv4 地址".into()))?;
        let mut all = vec!["-s".into(), format!("{ip}:5555")];
        all.extend(args.iter().map(|s| s.to_string()));
        self.commands
            .execute(&self.tools.adb, all, timeout, cancel)
            .await
    }
    pub async fn shell(
        &self,
        ip: &str,
        args: &[&str],
        cancel: CancellationToken,
    ) -> AppResult<String> {
        let mut command = vec!["shell"];
        command.extend(args);
        let result = self
            .adb(ip, &command, Duration::from_secs(20), cancel)
            .await?;
        if !result.success {
            return Err(AppError::Conflict(device_error(
                &result.stderr,
                &result.stdout,
            )));
        }
        Ok(result.stdout)
    }
    pub async fn connect(&self, ip: &str, cancel: CancellationToken) -> AppResult<()> {
        let ip = ip
            .parse::<std::net::Ipv4Addr>()
            .map_err(|_| AppError::InvalidConfig("设备 IP 必须是有效 IPv4 地址".into()))?
            .to_string();
        let _ = self
            .commands
            .execute(
                &self.tools.adb,
                vec!["connect".into(), format!("{ip}:5555")],
                Duration::from_secs(12),
                cancel.clone(),
            )
            .await?;
        let result = self
            .adb(&ip, &["get-state"], Duration::from_secs(10), cancel)
            .await?;
        if !result.success || result.stdout.trim() != "device" {
            return Err(AppError::Conflict(device_error(
                &result.stderr,
                &result.stdout,
            )));
        }
        Ok(())
    }
    pub async fn inspect(
        &self,
        screen: &ScreenAsset,
        action: &str,
        cancel: CancellationToken,
    ) -> AppResult<ScreenObservation> {
        let mut result = ScreenObservation {
            id: uuid::Uuid::now_v7().to_string(),
            screen_id: screen.id.clone(),
            operation_type: action.into(),
            observed_ip: screen.fields.ip.clone(),
            observed_at: now(),
            ..Default::default()
        };
        if ["ping", "inspect", "diagnostics"].contains(&action) {
            match ping_with(&self.commands, &screen.fields.ip, cancel.clone()).await {
                Ok(value) => result.ping = Some(if value { "online" } else { "offline" }.into()),
                Err(AppError::Cancelled) => return Err(AppError::Cancelled),
                Err(error) => result.errors.push(error.to_string()),
            }
        }
        if action == "ping" {
            return Ok(result);
        }
        if let Err(error) = self.connect(&screen.fields.ip, cancel.clone()).await {
            if matches!(error, AppError::Cancelled) {
                return Err(error);
            }
            result.errors.push(error.to_string());
            return Ok(result);
        }
        result.adb_available = true;
        // 4 寸使用已确认的 eth0；10 寸只接受唯一的有线网卡，不猜测多网卡身份。
        let mac_command = "if [ ! -d /sys/class/net/eth0 ] || [ -d /sys/class/net/eth0/wireless ] || [ -e /sys/class/net/eth0/phy80211 ]; then exit 2; fi; read t < /sys/class/net/eth0/type; if [ x$t != x1 ]; then exit 2; fi; cat /sys/class/net/eth0/address";
        let all_ethernet = r#"for p in /sys/class/net/eth*; do if [ -r "$p/address" ] && [ ! -d "$p/wireless" ] && [ ! -e "$p/phy80211" ]; then read t < "$p/type"; if [ x$t = x1 ]; then echo "${p##*/}=$(cat "$p/address")"; fi; fi; done"#;
        let command = if screen.fields.size == "10" {
            all_ethernet
        } else {
            mac_command
        };
        match self
            .shell(&screen.fields.ip, &[command], cancel.clone())
            .await
        {
            Ok(mac) if screen.fields.size == "10" => {
                result.mac_candidates = mac
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(String::from)
                    .collect();
                if let Ok((interface, address)) = unique_ethernet(&mac) {
                    result.observed_mac = Some(address);
                    result.mac_source = Some(format!("{interface}（有线网卡）"));
                } else {
                    result
                        .errors
                        .push("没有唯一、有效的有线网卡 MAC，请核实设备身份".into());
                }
            }
            Ok(mac) if valid_mac(mac.trim()) => {
                result.observed_mac = Some(mac.trim().into());
                result.mac_source = Some("eth0（有线网卡）".into());
                result.mac_candidates = vec![format!("eth0={}", mac.trim())];
            }
            Ok(_) => result
                .errors
                .push("有线网卡返回了无效 MAC，不能用于设备身份确认".into()),
            Err(error) => result.errors.push(format!("无法核对有线 MAC：{error}")),
        }
        if valid_mac(&screen.fields.mac)
            && result
                .observed_mac
                .as_deref()
                .is_some_and(|mac| normalize_mac(mac) != normalize_mac(&screen.fields.mac))
        {
            result
                .errors
                .push("实测 MAC 与当前确认资料不一致，请先核实设备身份".into());
        }
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        if action == "mac" {
            return Ok(result);
        }
        for (property, target) in [
            ("ro.build.version.release", "android"),
            ("ro.product.model", "model"),
            ("ro.build.fingerprint", "firmware"),
            ("ro.build.version.sdk", "sdk"),
            ("ro.product.cpu.abilist", "abis"),
            ("persist.adb.tcp.port", "port"),
            ("persist.sys.timezone", "timezone"),
        ] {
            match self
                .shell(&screen.fields.ip, &["getprop", property], cancel.clone())
                .await
            {
                Ok(value) => match target {
                    "android" => result.android = (!value.is_empty()).then_some(value),
                    "model" => result.device_model = Some(value),
                    "firmware" => result.firmware = Some(value),
                    "sdk" => result.sdk = value.parse().ok(),
                    "abis" => {
                        result.abis = value
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .map(String::from)
                            .collect()
                    }
                    "port" => result.persistent_adb = Some(value.trim() == "5555"),
                    "timezone" => result.timezone = (!value.is_empty()).then_some(value),
                    _ => {}
                },
                Err(error) => result.errors.push(error.to_string()),
            }
        }
        match self
            .shell(
                &screen.fields.ip,
                &["pm", "path", XIAOXIN_PACKAGE],
                cancel.clone(),
            )
            .await
        {
            Ok(path) if path.lines().any(|s| s.starts_with("package:")) => {
                result.app_installed = Some(true);
                result.package_id = Some(XIAOXIN_PACKAGE.into());
                match self
                    .shell(
                        &screen.fields.ip,
                        &["dumpsys", "package", XIAOXIN_PACKAGE],
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(text) => {
                        let (name, code) = parse_package_version(&text);
                        result.observed_app_version = name;
                        result.app_version_code = code;
                        if result.observed_app_version.is_none() || code.is_none() {
                            result
                                .errors
                                .push("应用已安装，但未取得完整版本信息".into());
                        }
                    }
                    Err(error) => result.errors.push(error.to_string()),
                }
                match self
                    .adb(
                        &screen.fields.ip,
                        &["shell", "pidof", XIAOXIN_PACKAGE],
                        Duration::from_secs(10),
                        cancel.clone(),
                    )
                    .await
                {
                    Ok(output) if output.success => {
                        result.app_running = Some(
                            output
                                .stdout
                                .split_whitespace()
                                .any(|part| part.parse::<u32>().is_ok()),
                        )
                    }
                    Ok(output) if output.stdout.is_empty() && output.stderr.is_empty() => {
                        result.app_running = Some(false)
                    }
                    Ok(_) => result.errors.push("未能确认小新进程状态".into()),
                    Err(error) => result.errors.push(error.to_string()),
                }
            }
            Ok(path) if path.is_empty() => result.app_installed = Some(false),
            Ok(_) => result
                .errors
                .push("小新安装状态返回异常，不能判为未安装".into()),
            Err(error) => result.errors.push(error.to_string()),
        }
        match self
            .shell(&screen.fields.ip, &["df", "-k", "/data"], cancel.clone())
            .await
        {
            Ok(text) => result.free_space_mb = parse_free_space(&text),
            Err(error) => result.errors.push(error.to_string()),
        }
        // 4 寸旧固件 date -u +%s 存在时区错误，统一解析 UTC 日历时间。
        match self
            .shell(
                &screen.fields.ip,
                &["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"],
                cancel.clone(),
            )
            .await
        {
            Ok(text) => match time::OffsetDateTime::parse(
                text.trim(),
                &time::format_description::well_known::Rfc3339,
            ) {
                Ok(value) => {
                    let computer = time::OffsetDateTime::now_utc();
                    result.clock_offset_seconds = Some(value.unix_timestamp() - computer.unix_timestamp());
                    result.device_time = value.format(&time::format_description::well_known::Rfc3339).ok();
                    result.computer_time = computer.format(&time::format_description::well_known::Rfc3339).ok();
                }
                Err(_) => result
                    .errors
                    .push("设备时间格式无法识别，未计算时间偏差".into()),
            },
            Err(error) => result.errors.push(error.to_string()),
        }
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        for setting in ["auto_time", "auto_time_zone"] {
            if let Ok(value) = self.shell(&screen.fields.ip, &["settings", "get", "global", setting], cancel.clone()).await {
                let value = match value.trim() { "1" => Some(true), "0" => Some(false), _ => None };
                if setting == "auto_time" { result.automatic_time = value; } else { result.automatic_timezone = value; }
            }
        }
        result.observed_at = now();
        Ok(result)
    }
}

pub async fn ping(ip: &str, cancel: CancellationToken) -> AppResult<bool> {
    ping_with(&NativeDeviceCommands, ip, cancel).await
}
pub async fn ping_with<P: DeviceCommandPort>(
    commands: &P,
    ip: &str,
    cancel: CancellationToken,
) -> AppResult<bool> {
    let ip = ip
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| AppError::InvalidConfig("设备 IP 必须是有效 IPv4 地址".into()))?
        .to_string();
    #[cfg(windows)]
    let system = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .ok_or_else(|| AppError::NotFound("Windows 系统目录不可用".into()))?;
    #[cfg(windows)]
    let (program, args) = (system.join("System32/ping.exe"),vec!["-n".into(), "1".into(), "-w".into(), "2000".into(), ip]);
    #[cfg(target_os = "macos")]
    let (program, args) = (PathBuf::from("/sbin/ping"),vec!["-c".into(), "1".into(), "-W".into(), "2000".into(), ip]);
    #[cfg(all(not(windows), not(target_os = "macos")))]
    let (program, args) = (PathBuf::from("/bin/ping"),vec!["-c".into(), "1".into(), "-W".into(), "2".into(), ip]);
    let output = commands
        .execute(
            &program,
            args,
            Duration::from_secs(5),
            cancel,
        )
        .await?;
    let text = format!("{} {}", output.stdout, output.stderr).to_lowercase();
    if [
        "general failure",
        "常规故障",
        "一般故障",
        "transmit failed",
        "传输失败",
    ]
    .iter()
    .any(|reason| text.contains(reason))
        || text.trim().is_empty()
    {
        return Err(AppError::Conflict(
            "本机 Ping 执行异常，不能据此判断设备离线".into(),
        ));
    }
    Ok(output.success && Regex::new(r"(?i)TTL=\d+").unwrap().is_match(&output.stdout))
}
fn device_error(stderr: &str, stdout: &str) -> String {
    let text = format!("{stderr} {stdout}").to_lowercase();
    if text.contains("unauthorized") {
        "设备尚未授权 ADB，请在屏端确认调试授权".into()
    } else if text.contains("offline") {
        "ADB 设备离线，请检查屏和网络".into()
    } else if text.contains("permission denied") || text.contains("not permitted") {
        "设备未授予此操作权限".into()
    } else {
        "设备管理连接或命令执行失败，请检查 ADB 端口和设备状态".into()
    }
}
pub fn parse_package_version(text: &str) -> (Option<String>, Option<u64>) {
    let active=text.split("Hidden system packages:").next().unwrap_or(text);
    let text=active.split_once("Packages:").map(|(_,section)|section).unwrap_or(active);
    let name = Regex::new(r"(?m)^\s*versionName=([^\r\n]+)")
        .unwrap()
        .captures(text)
        .map(|c| c[1].trim().to_string())
        .filter(|v| !v.is_empty() && v != "null");
    let code = Regex::new(r"\bversionCode=(\d+)")
        .unwrap()
        .captures(text)
        .and_then(|c| c[1].parse().ok());
    (name, code)
}
pub fn unique_ethernet(text: &str) -> AppResult<(String, String)> {
    let rows: Vec<_> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    if rows.len() != 1 {
        return Err(AppError::Conflict("有线网卡不唯一".into()));
    }
    let (interface, address) = rows[0]
        .trim()
        .split_once('=')
        .ok_or_else(|| AppError::Conflict("网卡信息格式不正确".into()))?;
    if !interface.starts_with("eth") || !valid_mac(address) {
        return Err(AppError::Conflict("网卡 MAC 无效".into()));
    }
    Ok((interface.into(), address.into()))
}
pub fn parse_free_space(text: &str) -> Option<u64> {
    text.lines().rev().find_map(|line| {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        if parts.len() >= 6 {
            parts[3].parse::<u64>().ok().map(|v| v / 1024)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FixtureCommands {
        mode: &'static str,
        calls: std::sync::Mutex<Vec<Vec<String>>>,
    }
    impl DeviceCommandPort for FixtureCommands {
        async fn execute(
            &self,
            program: &Path,
            args: Vec<String>,
            _limit: Duration,
            _cancel: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            self.calls.lock().unwrap().push(args.clone());
            let ok = |text: &str| ProcessOutput {
                success: true,
                stdout: text.into(),
                stderr: String::new(),
            };
            if program
                .file_name()
                .is_some_and(|s| ["ping.exe", "ping"].iter().any(|name| s.to_string_lossy().eq_ignore_ascii_case(name)))
            {
                return Ok(if self.mode == "ping_failure" {
                    ok("PING: transmit failed. General failure.")
                } else if self.mode == "offline" {
                    ProcessOutput {
                        success: false,
                        stdout: "Request timed out.".into(),
                        stderr: String::new(),
                    }
                } else {
                    ok("Reply from 192.0.2.1: bytes=32 time<1ms TTL=64")
                });
            }
            if args.first().is_some_and(|s| s == "connect") {
                return Ok(ok("connected"));
            }
            if args.iter().any(|s| s == "get-state") {
                return match self.mode {
                    "unauthorized" => Ok(ProcessOutput {
                        success: false,
                        stdout: String::new(),
                        stderr: "error: device unauthorized".into(),
                    }),
                    "timeout" => Err(AppError::timeout("读取 ADB 状态")),
                    _ => Ok(ok("device")),
                };
            }
            if args.iter().any(|s| s.contains("/sys/class/net/eth0")) {
                return Ok(ok("02:11:22:33:44:55"));
            }
            if args.iter().any(|s| s == "getprop") {
                return Ok(ok(match args.last().unwrap().as_str() {
                    "ro.build.version.release" => "8.1.0",
                    "ro.build.version.sdk" => "27",
                    "ro.product.cpu.abilist" => "arm64-v8a,armeabi-v7a",
                    "persist.adb.tcp.port" => "",
                    _ => "测试固件",
                }));
            }
            if args.iter().any(|s| s == "pm") {
                return Ok(ok(""));
            }
            if args.iter().any(|s| s == "df") {
                return Ok(ok("/dev/block/data 100000 1000 99000 1% /data"));
            }
            if args.iter().any(|s| s == "date") {
                return Ok(ok(&now()));
            }
            Err(AppError::InvalidConfig(
                "测试发现未列入只读范围的命令".into(),
            ))
        }
    }
    fn fixture(mode: &'static str) -> AdbDevice<FixtureCommands> {
        AdbDevice::with_commands(
            AndroidTools {
                adb: PathBuf::from("fixture-adb.exe"),
            },
            FixtureCommands {
                mode,
                calls: Default::default(),
            },
        )
    }
    fn screen() -> ScreenAsset {
        ScreenAsset {
            id: "fixture".into(),
            source: "local".into(),
            fields: crate::domain::smart_screen::model::ScreenFields {
                ip: "192.0.2.1".into(),
                size: "4".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn unauthorized_and_timeout_are_not_reported_as_ip_offline() {
        for mode in ["unauthorized", "timeout"] {
            let result = fixture(mode)
                .inspect(&screen(), "mac", CancellationToken::new())
                .await
                .unwrap();
            assert!(!result.adb_available);
            assert_eq!(result.ping, None);
            assert!(result.observed_mac.is_none());
            assert!(!result.errors.is_empty());
        }
    }
    #[tokio::test]
    async fn normal_offline_differs_from_ping_execution_failure() {
        assert!(
            !ping_with(
                &fixture("offline").commands,
                "192.0.2.1",
                CancellationToken::new()
            )
            .await
            .unwrap()
        );
        assert!(
            ping_with(
                &fixture("ping_failure").commands,
                "192.0.2.1",
                CancellationToken::new()
            )
            .await
            .is_err()
        );
    }
    #[tokio::test]
    async fn uninstalled_application_has_no_guessed_or_stale_version() {
        let device = fixture("uninstalled");
        let result = device
            .inspect(&screen(), "inspect", CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(result.app_installed, Some(false));
        assert_eq!(result.observed_app_version, None);
        assert_eq!(result.app_version_code, None);
        assert!(result.errors.is_empty());
        assert!(device.commands.calls.lock().unwrap().iter().all(|args| {
            !args
                .iter()
                .any(|s| ["install", "uninstall", "reboot", "setprop", "am"].contains(&s.as_str()))
        }));
    }
    #[test]
    fn parses_real_package_and_storage_outputs_without_assuming_success() {
        assert_eq!(
            parse_package_version("  versionCode=209 minSdk=23\n  versionName=2.0.9\n"),
            (Some("2.0.9".into()), Some(209))
        );
        assert_eq!(
            parse_package_version("Unable to find package"),
            (None, None)
        );
        assert_eq!(
            parse_free_space(
                "Filesystem 1K-blocks Used Available Use% Mounted on\n/dev/block/dm-0 100000 1000 99000 1% /data"
            ),
            Some(96)
        );
        assert_eq!(parse_free_space("Permission denied"), None);
        assert!(device_error("error: device unauthorized", "").contains("未授权"));
        assert!(unique_ethernet("eth0=02:11:22:33:44:55\neth1=02:11:22:33:44:56").is_err());
        assert_eq!(unique_ethernet("eth0=02:11:22:33:44:55").unwrap().0, "eth0");
    }
}
