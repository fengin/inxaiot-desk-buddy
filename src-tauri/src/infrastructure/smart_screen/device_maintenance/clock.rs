use super::{clock_settings, read, root_command};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::model::{ScreenAsset, ScreenObservation},
    infrastructure::smart_screen::device::{AdbDevice, DeviceCommandPort},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAX_OFFSET_SECONDS: u64 = 15;

#[derive(Debug, Serialize, Deserialize)]
struct HardwareClock {
    index: String,
    name: String,
}

impl HardwareClock {
    fn validate(&self) -> AppResult<()> {
        let number = self.index.strip_prefix("rtc").unwrap_or("");
        if number.is_empty()
            || number.len() > 3
            || !number.bytes().all(|c| c.is_ascii_digit())
            || !matches!(self.name.as_str(), "rk808-rtc" | "rtc-pcf85063")
        {
            return Err(AppError::Conflict(
                "硬件时钟记录无效，请重新检查设备".into(),
            ));
        }
        Ok(())
    }
    fn sys_path(&self, field: &str) -> String {
        format!("/sys/class/rtc/{}/{field}", self.index)
    }
    fn device_path(&self) -> String {
        format!("/dev/{}", self.index)
    }
}

fn expected_driver(screen: &ScreenAsset, observed: &ScreenObservation) -> AppResult<&'static str> {
    let firmware = observed.firmware.as_deref().unwrap_or("");
    match (screen.fields.size.as_str(), observed.sdk) {
        ("4", Some(27)) if firmware.starts_with("rockchip/px30_evb/px30_evb:8.1.0/") => {
            Ok("rk808-rtc")
        }
        ("10", Some(29)) if firmware.starts_with("STATION/ceres_c4/ceres-c3:10/") => {
            Ok("rtc-pcf85063")
        }
        _ => Err(AppError::Conflict(
            "当前固件的硬件时钟尚未验证，未修改时间；请先核对设备型号和固件".into(),
        )),
    }
}

async fn check_hardware<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    rtc: &HardwareClock,
) -> AppResult<()> {
    rtc.validate()?;
    let name = root_command(device, ip, mode, &["cat", &rtc.sys_path("name")]).await?;
    let boot_clock = root_command(device, ip, mode, &["cat", &rtc.sys_path("hctosys")]).await?;
    if name.trim() != rtc.name || boot_clock.trim() != "1" {
        return Err(AppError::Conflict(
            "启动使用的硬件时钟与检查记录不一致，未继续校时".into(),
        ));
    }
    // test 是 shell 内建命令，10 寸的 su 不能把它当独立程序直接执行。
    // 节点名已限制为 rtc 加数字，不接受外部命令片段。
    let command = format!("'test -c {}'", rtc.device_path());
    root_command(device, ip, mode, &["sh", "-c", &command]).await?;
    Ok(())
}

async fn hardware_seconds<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    rtc: &HardwareClock,
) -> AppResult<i64> {
    // 内核直接返回 UTC 秒数，避免旧版 Android date/hwclock 的本地时区解析差异。
    let value = root_command(device, ip, mode, &["cat", &rtc.sys_path("since_epoch")]).await?;
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|seconds| *seconds >= 0 && OffsetDateTime::from_unix_timestamp(*seconds).is_ok())
        .ok_or_else(|| AppError::Conflict("无法读取硬件时钟的实际时间".into()))
}

async fn system_seconds<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppResult<i64> {
    let value = read(device, ip, &["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"]).await?;
    OffsetDateTime::parse(value.trim(), &Rfc3339)
        .map(|time| time.unix_timestamp())
        .map_err(|_| AppError::Conflict("无法读取设备系统时间".into()))
}

pub(super) async fn capability<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
    mode: &str,
) -> AppResult<Value> {
    let expected = expected_driver(screen, observed)?;
    let ip = &screen.fields.ip;
    let entries = root_command(device, ip, mode, &["ls", "/sys/class/rtc"]).await?;
    let mut candidates = Vec::new();
    for index in entries.split_whitespace() {
        let candidate = HardwareClock {
            index: index.into(),
            name: expected.into(),
        };
        candidate.validate()?;
        let name = root_command(device, ip, mode, &["cat", &candidate.sys_path("name")]).await?;
        if name.trim() == expected {
            candidates.push(candidate);
        }
    }
    if candidates.len() != 1 {
        return Err(AppError::Conflict(
            "未找到唯一且已验证的硬件时钟，未修改时间".into(),
        ));
    }
    let rtc = candidates.remove(0);
    check_hardware(device, ip, mode, &rtc).await?;
    let help = root_command(device, ip, mode, &["hwclock", "--help"]).await?;
    if !["--systohc", "--utc", "--rtc"]
        .iter()
        .all(|flag| help.contains(flag))
    {
        return Err(AppError::Conflict(
            "当前固件不支持已验证的硬件校时命令，未修改时间".into(),
        ));
    }
    Ok(json!({"rtc":rtc, "beforeHardwareSeconds":hardware_seconds(device, ip, mode, &rtc).await?}))
}

pub(super) async fn set_system<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    seconds: i64,
) -> AppResult<()> {
    // Android 8 toybox 对 @时间戳可能套用本地时区，显式使用 UTC，不修改设备时区。
    root_command(device, ip, mode, &["date", "-u", &format!("@{seconds}")]).await?;
    Ok(())
}

pub(super) async fn set_hardware<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    selected: &Value,
) -> AppResult<()> {
    let rtc: HardwareClock = serde_json::from_value(selected.clone())
        .map_err(|_| AppError::Conflict("缺少硬件时钟检查记录，未写入硬件时钟".into()))?;
    check_hardware(device, ip, mode, &rtc).await?;
    root_command(
        device,
        ip,
        mode,
        &["hwclock", "-w", "-u", "-f", &rtc.device_path()],
    )
    .await?;
    Ok(())
}

pub(super) struct Verification {
    pub succeeded: bool,
    pub message: String,
    pub readings: Value,
}

pub(super) async fn verify<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    evidence: &Value,
) -> AppResult<Verification> {
    let capability = &evidence["capability"];
    let selected = &capability["clock"]["rtc"];
    let incomplete = if selected.is_null() {
        Some("原校时记录未包含硬件时钟，请重新执行校准时间")
    } else if !matches!(
        evidence["phase"].as_str(),
        Some("hardware_clock_requested" | "hardware_clock_set" | "clock_verified")
    ) {
        Some("本次尚未完成硬件校时，请重新执行校准时间")
    } else {
        None
    };
    if let Some(message) = incomplete {
        return Ok(Verification {
            succeeded: false,
            message: message.into(),
            readings: Value::Null,
        });
    }
    let mode = capability["root"].as_str().unwrap_or("");
    let rtc: HardwareClock = serde_json::from_value(selected.clone())
        .map_err(|_| AppError::Conflict("硬件时钟检查记录无效，请重新检查".into()))?;
    check_hardware(device, ip, mode, &rtc).await?;
    let system = system_seconds(device, ip).await?;
    let system_checked_at = OffsetDateTime::now_utc().unix_timestamp();
    let hardware = hardware_seconds(device, ip, mode, &rtc).await?;
    let hardware_checked_at = OffsetDateTime::now_utc().unix_timestamp();
    let system_offset = system - system_checked_at;
    let hardware_offset = hardware - hardware_checked_at;
    let settings = clock_settings(device, ip).await?;
    let readings = json!({
        "systemSeconds":system, "systemOffsetSeconds":system_offset, "systemCheckedAt":system_checked_at,
        "hardwareSeconds":hardware, "hardwareOffsetSeconds":hardware_offset, "hardwareCheckedAt":hardware_checked_at,
        "settings":settings, "rtc":rtc,
    });
    let problem = if settings != capability["clockSettings"] {
        Some("校时后的时区或自动校时设置与执行前不一致，请检查设备".into())
    } else if system_offset.unsigned_abs() > MAX_OFFSET_SECONDS {
        Some(format!(
            "系统时间偏差 {system_offset} 秒，校时未通过，请检查设备"
        ))
    } else if hardware_offset.unsigned_abs() > MAX_OFFSET_SECONDS {
        Some(format!(
            "系统时间已校准，但硬件时钟仍偏差 {hardware_offset} 秒；重启可能回退，请重新校准"
        ))
    } else {
        None
    };
    if let Some(message) = problem {
        return Ok(Verification {
            succeeded: false,
            message,
            readings,
        });
    }
    let mut message = format!(
        "系统时间与硬件时钟已校准，偏差分别为 {system_offset} 秒、{hardware_offset} 秒；时区和自动校时设置保留"
    );
    if rtc.name == "rk808-rtc" {
        message.push_str("；已测4寸固件完全断电后仍可能丢时");
    }
    Ok(Verification {
        succeeded: true,
        message,
        readings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::smart_screen::device::{AndroidTools, ProcessOutput};
    use std::{
        collections::BTreeMap,
        path::Path,
        sync::{Arc, Mutex},
        time::Duration,
    };
    use tokio_util::sync::CancellationToken;

    #[derive(Clone)]
    struct Commands(Arc<Mutex<SimulatedClock>>);
    struct SimulatedClock {
        drivers: BTreeMap<String, String>,
        system: i64,
        hardware: i64,
        boot_clock: bool,
        write_error: bool,
        read_error: bool,
        zone: String,
        calls: Vec<Vec<String>>,
    }
    impl Commands {
        fn new(driver: &str, index: &str) -> Self {
            Self(Arc::new(Mutex::new(SimulatedClock {
                drivers: [(index.into(), driver.into())].into(),
                system: OffsetDateTime::now_utc().unix_timestamp(),
                hardware: 1_500_000_000,
                boot_clock: true,
                write_error: false,
                read_error: false,
                zone: "Asia/Shanghai".into(),
                calls: Vec::new(),
            })))
        }
        fn device(&self) -> AdbDevice<Self> {
            AdbDevice::with_commands(
                AndroidTools {
                    adb: "unused".into(),
                },
                self.clone(),
            )
        }
        fn writes(&self) -> Vec<Vec<String>> {
            self.0
                .lock()
                .unwrap()
                .calls
                .iter()
                .filter(|args| {
                    args.first().is_some_and(|s| s == "hwclock") && args.iter().any(|s| s == "-w")
                        || args.iter().any(|s| s.starts_with('@'))
                })
                .cloned()
                .collect()
        }
    }
    impl DeviceCommandPort for Commands {
        async fn execute(
            &self,
            _: &Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            let mut c = self.0.lock().unwrap();
            let mut args = args[3..].to_vec();
            if args.starts_with(&["su".into(), "0".into()]) {
                args.drain(..2);
            }
            c.calls.push(args.clone());
            let parts: Vec<_> = args.iter().map(String::as_str).collect();
            let stdout = match parts.as_slice() {
                ["ls", "/sys/class/rtc"] => {
                    c.drivers.keys().cloned().collect::<Vec<_>>().join("\n")
                }
                ["cat", path] => {
                    let rest = path
                        .strip_prefix("/sys/class/rtc/")
                        .expect("仅访问硬件时钟目录");
                    let (index, field) = rest.split_once('/').unwrap();
                    match field {
                        "name" => c.drivers.get(index).cloned().unwrap_or_default(),
                        "hctosys" => if c.boot_clock { "1" } else { "0" }.into(),
                        "since_epoch" => {
                            if c.read_error {
                                return Err(AppError::Conflict("ADB 连接断开".into()));
                            }
                            c.hardware.to_string()
                        }
                        _ => panic!("未预期的文件 {path}"),
                    }
                }
                ["sh", "-c", command] if command.starts_with("'test -c /dev/rtc") => String::new(),
                ["hwclock", "--help"] => "--systohc --utc --rtc".into(),
                ["hwclock", "-w", "-u", "-f", _] => {
                    if c.write_error {
                        return Err(AppError::Conflict("写入失败".into()));
                    }
                    c.hardware = c.system;
                    String::new()
                }
                ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"] => {
                    OffsetDateTime::from_unix_timestamp(c.system)
                        .unwrap()
                        .format(&Rfc3339)
                        .unwrap()
                }
                ["date", "-u", value] if value.starts_with('@') => {
                    c.system = value[1..].parse().unwrap();
                    String::new()
                }
                ["getprop", "persist.sys.timezone"] => c.zone.clone(),
                ["settings", "get", "global", "auto_time" | "auto_time_zone"] => "1".into(),
                _ => panic!("未预期的命令 {args:?}"),
            };
            Ok(ProcessOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }
    fn screen(size: &str) -> (ScreenAsset, ScreenObservation) {
        let mut screen = ScreenAsset::default();
        screen.fields.size = size.into();
        screen.fields.ip = "192.0.2.1".into();
        let (sdk, firmware) = if size == "4" {
            (27, "rockchip/px30_evb/px30_evb:8.1.0/test")
        } else {
            (29, "STATION/ceres_c4/ceres-c3:10/test")
        };
        (
            screen,
            ScreenObservation {
                sdk: Some(sdk),
                firmware: Some(firmware.into()),
                ..Default::default()
            },
        )
    }
    fn evidence(clock: Value, phase: &str) -> Value {
        json!({"phase":phase, "capability":{"root":"shell", "clock":clock,
            "clockSettings":{"zone":"Asia/Shanghai", "autoTime":"1", "autoZone":"1"}}})
    }

    #[tokio::test]
    async fn both_firmwares_require_hardware_sync_and_verify_without_writing_again() {
        for (size, driver) in [("4", "rk808-rtc"), ("10", "rtc-pcf85063")] {
            let commands = Commands::new(driver, "rtc0");
            let device = commands.device();
            let (screen, observed) = screen(size);
            let cap = capability(&device, &screen, &observed, "shell")
                .await
                .unwrap();
            let record = evidence(cap.clone(), "hardware_clock_requested");
            set_system(
                &device,
                &screen.fields.ip,
                "shell",
                OffsetDateTime::now_utc().unix_timestamp(),
            )
            .await
            .unwrap();
            let stale = verify(&device, &screen.fields.ip, &record).await.unwrap();
            assert!(!stale.succeeded);
            assert!(stale.message.contains("硬件时钟仍偏差"));
            set_hardware(&device, &screen.fields.ip, "shell", &cap["rtc"])
                .await
                .unwrap();
            let writes = commands.writes();
            assert_eq!(writes.len(), 2);
            assert_eq!(writes[1], ["hwclock", "-w", "-u", "-f", "/dev/rtc0"]);
            // 未确认命令应答时也只回读实际状态，绝不重写时间。
            let completed = verify(&device, &screen.fields.ip, &record).await.unwrap();
            assert!(completed.succeeded);
            assert!(
                completed.readings["hardwareOffsetSeconds"]
                    .as_i64()
                    .unwrap()
                    .abs()
                    <= 15
            );
            assert_eq!(completed.message.contains("完全断电"), size == "4");
            assert_eq!(commands.writes(), writes);
        }
    }

    #[tokio::test]
    async fn uses_verified_driver_instead_of_assuming_rtc0_or_writing_all_clocks() {
        let commands = Commands::new("rtc-pcf85063", "rtc3");
        commands
            .0
            .lock()
            .unwrap()
            .drivers
            .insert("rtc0".into(), "sunxi-rtc".into());
        let (screen, observed) = screen("10");
        let cap = capability(&commands.device(), &screen, &observed, "su0")
            .await
            .unwrap();
        assert_eq!(cap["rtc"]["index"], "rtc3");
        set_hardware(&commands.device(), &screen.fields.ip, "su0", &cap["rtc"])
            .await
            .unwrap();
        assert_eq!(
            commands.writes(),
            [vec!["hwclock", "-w", "-u", "-f", "/dev/rtc3"]]
        );
    }

    #[tokio::test]
    async fn unknown_firmware_duplicate_driver_and_non_boot_clock_are_rejected_before_writes() {
        for scenario in [
            "unknown",
            "duplicate",
            "not_boot",
            "missing",
            "invalid_time",
        ] {
            let commands = Commands::new("rk808-rtc", "rtc0");
            let (screen, mut observed) = screen("4");
            match scenario {
                "unknown" => observed.firmware = Some("unverified/vendor".into()),
                "duplicate" => {
                    commands
                        .0
                        .lock()
                        .unwrap()
                        .drivers
                        .insert("rtc1".into(), "rk808-rtc".into());
                }
                "not_boot" => commands.0.lock().unwrap().boot_clock = false,
                "missing" => commands.0.lock().unwrap().drivers.clear(),
                _ => commands.0.lock().unwrap().hardware = i64::MAX,
            }
            assert!(
                capability(&commands.device(), &screen, &observed, "shell")
                    .await
                    .is_err(),
                "{scenario}"
            );
            assert!(commands.writes().is_empty());
        }
    }

    #[tokio::test]
    async fn clock_changes_after_preflight_and_untrusted_record_paths_never_write() {
        let commands = Commands::new("rtc-pcf85063", "rtc0");
        let (screen, observed) = screen("10");
        let cap = capability(&commands.device(), &screen, &observed, "shell")
            .await
            .unwrap();
        commands
            .0
            .lock()
            .unwrap()
            .drivers
            .insert("rtc0".into(), "sunxi-rtc".into());
        assert!(
            set_hardware(&commands.device(), &screen.fields.ip, "shell", &cap["rtc"])
                .await
                .is_err()
        );
        assert!(
            set_hardware(
                &commands.device(),
                &screen.fields.ip,
                "shell",
                &json!({"index":"rtc0/../../other", "name":"rtc-pcf85063"})
            )
            .await
            .is_err()
        );
        assert!(commands.writes().is_empty());
    }

    #[tokio::test]
    async fn write_failure_read_failure_wrong_system_and_changed_settings_do_not_pass() {
        let commands = Commands::new("rtc-pcf85063", "rtc0");
        let device = commands.device();
        let (screen, observed) = screen("10");
        let cap = capability(&device, &screen, &observed, "shell")
            .await
            .unwrap();
        let record = evidence(cap.clone(), "hardware_clock_requested");
        commands.0.lock().unwrap().write_error = true;
        assert!(
            set_hardware(&device, &screen.fields.ip, "shell", &cap["rtc"])
                .await
                .is_err()
        );
        assert!(
            !verify(&device, &screen.fields.ip, &record)
                .await
                .unwrap()
                .succeeded
        );
        commands.0.lock().unwrap().read_error = true;
        assert!(verify(&device, &screen.fields.ip, &record).await.is_err());
        {
            let mut c = commands.0.lock().unwrap();
            c.read_error = false;
            c.hardware = c.system;
            c.system -= 3600;
        }
        assert!(
            !verify(&device, &screen.fields.ip, &record)
                .await
                .unwrap()
                .succeeded
        );
        {
            let mut c = commands.0.lock().unwrap();
            c.system = c.hardware;
            c.zone = "UTC".into();
        }
        assert!(
            verify(&device, &screen.fields.ip, &record)
                .await
                .unwrap()
                .message
                .contains("时区")
        );
    }

    #[tokio::test]
    async fn interrupted_before_hardware_request_and_legacy_tasks_cannot_be_declared_successful() {
        let commands = Commands::new("rk808-rtc", "rtc0");
        let (screen, observed) = screen("4");
        let cap = capability(&commands.device(), &screen, &observed, "shell")
            .await
            .unwrap();
        for record in [
            evidence(cap, "clock_set"),
            json!({"phase":"clock_set", "capability":{}}),
        ] {
            assert!(
                !verify(&commands.device(), &screen.fields.ip, &record)
                    .await
                    .unwrap()
                    .succeeded
            );
        }
        assert!(commands.writes().is_empty());
    }
}
