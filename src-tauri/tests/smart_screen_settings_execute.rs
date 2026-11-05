#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::{AppError, AppResult},
    domain::{
        common::task::TaskState,
        smart_screen::{model::*, ntp::NtpPatch, operation::*, settings::ScreenSettingsPatch},
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::{
            screen_repository::{ScreenRepository, now},
            task_repository::CreateTask,
        },
        smart_screen::{
            device::{AdbDevice, AndroidTools, DeviceCommandPort, ProcessOutput},
            maintenance::MaintenancePlan,
            screen_settings, task_data,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Sim {
    size: &'static str,
    persistent: String,
    service: String,
    server: String,
    active_server: String,
    automatic: bool,
    boot: u64,
    boot_completed: bool,
    system_offset: i64,
    hardware_offset: i64,
    mask: u32,
    fail_auto_time: bool,
    date_supported: bool,
    fail_hardware: bool,
    fail_sleep: bool,
    replace_after_boot: bool,
    app_present: bool,
    app_running: bool,
    stop_app_on_boot: bool,
    cancel_after_adb: Option<CancellationToken>,
    calls: Vec<Vec<String>>,
}
impl Default for Sim {
    fn default() -> Self {
        Self {
            size: "10",
            persistent: "0".into(),
            service: "5555".into(),
            server: "old.test".into(),
            active_server: "old.test".into(),
            automatic: false,
            boot: 1,
            boot_completed: true,
            system_offset: 0,
            hardware_offset: -120,
            mask: 0,
            fail_auto_time: false,
            date_supported: true,
            fail_hardware: false,
            fail_sleep: false,
            replace_after_boot: false,
            app_present: false,
            app_running: false,
            stop_app_on_boot: false,
            cancel_after_adb: None,
            calls: vec![],
        }
    }
}
impl Sim {
    fn mac(&self) -> &str {
        if self.replace_after_boot && self.boot > 1 {
            "02:11:22:33:44:99"
        } else {
            "02:11:22:33:44:10"
        }
    }
    fn boot_id(&self) -> String {
        format!("00000000-0000-4000-8000-{:012}", self.boot)
    }
    fn power(&self) -> String {
        format!(
            "POWER MANAGER (dumpsys power)\n mWakefulness=Awake\n mIsPowered=true\n mPlugType=1\n mStayOnWhilePluggedInSetting={}\n mStayOn={}\n mMaximumScreenOffTimeoutFromDeviceAdmin=2147483647 (enforced=false)\nDisplay Power: state=ON\n",
            self.mask,
            self.mask & 1 != 0
        )
    }
}
type SavedContext = (sqlx::SqlitePool, String, String, String);
#[derive(Clone)]
struct Fake {
    sim: Arc<Mutex<Sim>>,
    context: Arc<Mutex<Option<SavedContext>>>,
    reboot_snapshot: Arc<Mutex<Option<ScreenTargetResult>>>,
}
impl Fake {
    fn new(sim: Sim) -> Self {
        Self {
            sim: Arc::new(Mutex::new(sim)),
            context: Arc::new(Mutex::new(None)),
            reboot_snapshot: Arc::new(Mutex::new(None)),
        }
    }
    fn device(&self) -> AdbDevice<Self> {
        AdbDevice::with_commands(
            AndroidTools {
                adb: "never-run-real-adb".into(),
            },
            self.clone(),
        )
    }
    fn writes(&self) -> Vec<Vec<String>> {
        self.sim
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|v| {
                v.iter().any(|s| {
                    matches!(
                        s.as_str(),
                        "setprop" | "put" | "delete" | "reboot" | "input"
                    )
                }) || v.iter().any(|s| s.starts_with('@'))
                    || v.iter().any(|s| s == "-w")
            })
            .cloned()
            .collect()
    }
    fn clear_calls(&self) {
        self.sim.lock().unwrap().calls.clear();
    }
}
impl DeviceCommandPort for Fake {
    async fn execute(
        &self,
        program: &Path,
        args: Vec<String>,
        _: Duration,
        cancel: CancellationToken,
    ) -> AppResult<ProcessOutput> {
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let ping = program
            .file_name()
            .is_some_and(|v| v.to_string_lossy().to_ascii_lowercase().starts_with("ping"));
        if !ping {
            assert_eq!(program, Path::new("never-run-real-adb"));
        }
        let command = if ping {
            vec!["ping"]
        } else if args.first().is_some_and(|v| v == "connect") {
            vec!["connect"]
        } else {
            assert_eq!(&args[..2], &["-s", "192.0.2.10:5555"]);
            args[2..].iter().map(String::as_str).collect::<Vec<_>>()
        };
        if command == ["reboot"] {
            let context = self.context.lock().unwrap().clone();
            if let Some((pool, project, id, screen)) = context {
                let results = task_data::read_results(&pool, &project, &id).await?;
                *self.reboot_snapshot.lock().unwrap() = results.targets.get(&screen).cloned();
            }
        }
        let mut s = self.sim.lock().unwrap();
        s.calls.push(command.iter().map(|v| (*v).into()).collect());
        let now_seconds = time::OffsetDateTime::now_utc().unix_timestamp();
        let stdout = match command.as_slice() {
            ["ping"] => "Reply from 192.0.2.10: bytes=32 time<1ms TTL=64".into(),
            ["connect"] => "connected".into(),
            ["get-state"] => "device".into(),
            ["reboot"] => {
                s.boot += 1;
                s.service = s.persistent.clone();
                s.active_server = s.server.clone();
                s.system_offset = 0;
                if s.stop_app_on_boot {
                    s.app_running = false;
                }
                String::new()
            }
            ["shell", cmd] if cmd.contains("/sys/class/net/eth0/address") => s.mac().into(),
            ["shell", cmd] if cmd.contains("for p in /sys/class/net/eth*") => {
                format!("eth0={}", s.mac())
            }
            ["shell", "getprop", property] => match *property {
                "ro.build.version.release" => if s.size == "4" { "8.1.0" } else { "10" }.into(),
                "ro.build.version.sdk" => if s.size == "4" { "27" } else { "29" }.into(),
                "ro.product.model" => if s.size == "4" {
                    "px30_evb"
                } else {
                    "Smart touch panel"
                }
                .into(),
                "ro.build.fingerprint" => if s.size == "4" {
                    "rockchip/px30_evb/px30_evb:8.1.0/test"
                } else {
                    "STATION/ceres_c4/ceres-c3:10/test"
                }
                .into(),
                "ro.product.cpu.abilist" => "armeabi-v7a,arm64-v8a".into(),
                "persist.sys.timezone" => "Asia/Shanghai".into(),
                "persist.adb.tcp.port" => s.persistent.clone(),
                "service.adb.tcp.port" => s.service.clone(),
                "sys.boot_completed" => if s.boot_completed { "1" } else { "0" }.into(),
                _ => panic!("未声明属性 {property}"),
            },
            ["shell", "id"] => "uid=0(root)".into(),
            ["shell", "id", "-u"] => "0".into(),
            ["shell", "pm", "path", "chat.xiaoxin.app"] => if s.app_present {
                "package:/system/app/xiaoxin.apk"
            } else {
                ""
            }
            .into(),
            ["shell", "dumpsys", "package", "chat.xiaoxin.app"] => {
                "versionName=2.1.3\nversionCode=25038 minSdk=24".into()
            }
            ["shell", "pidof", "chat.xiaoxin.app"] => if s.app_running { "222" } else { "" }.into(),
            ["shell", "pidof", "system_server"] => format!("{}", 100 + s.boot),
            ["shell", "df", "-k", "/data"] => "/dev/block/data 100000 1000 99000 1% /data".into(),
            ["shell", "date", "-u", "+%Y-%m-%dT%H:%M:%SZ"] => {
                time::OffsetDateTime::from_unix_timestamp(now_seconds + s.system_offset)
                    .unwrap()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            }
            ["shell", "date", "--help"] => if s.date_supported {
                "date @UNIXTIME"
            } else {
                "unsupported date"
            }
            .into(),
            ["shell", "date", "-u", seconds] if seconds.starts_with('@') => {
                s.system_offset = seconds[1..].parse::<i64>().unwrap() - now_seconds;
                String::new()
            }
            ["shell", "hwclock", "--help"] => "--systohc --utc --rtc".into(),
            ["shell", "hwclock", "-w", "-u", "-f", "/dev/rtc0"] => {
                if s.fail_hardware {
                    return Err(AppError::Conflict("模拟硬件时钟写失败".into()));
                }
                s.hardware_offset = s.system_offset;
                String::new()
            }
            ["shell", "ls", "/sys/class/rtc"] => "rtc0".into(),
            ["shell", "cat", "/sys/class/rtc/rtc0/name"] => if s.size == "4" {
                "rk808-rtc"
            } else {
                "rtc-pcf85063"
            }
            .into(),
            ["shell", "cat", "/sys/class/rtc/rtc0/hctosys"] => "1".into(),
            ["shell", "cat", "/sys/class/rtc/rtc0/since_epoch"] => {
                format!("{}", now_seconds + s.hardware_offset)
            }
            ["shell", "sh", "-c", cmd] if cmd.contains("test -c /dev/rtc0") => String::new(),
            ["shell", "cat", "/proc/sys/kernel/random/boot_id"] => s.boot_id(),
            ["shell", "cat", "/proc/uptime"] => "3600.00 100.00".into(),
            ["shell", "cat", "/vendor/build.prop"] => "service.adb.tcp.port=5555\n".into(),
            ["shell", "settings", "get", "global", key] => match *key {
                "auto_time" => if s.automatic { "1" } else { "0" }.into(),
                "auto_time_zone" => "0".into(),
                "ntp_server" => {
                    if s.server.is_empty() {
                        "null".into()
                    } else {
                        s.server.clone()
                    }
                }
                "stay_on_while_plugged_in" => s.mask.to_string(),
                _ => panic!("未声明设置{key}"),
            },
            ["shell", "settings", "get", "system", "screen_off_timeout"] => "60000".into(),
            ["shell", "dumpsys", "power"] => s.power(),
            ["shell", "dumpsys", "network_time_update_service"] => {
                "PollingIntervalMs: +1d0h0m0s0ms\nNTP cache age: 100\nNTP cache certainty: 5\n"
                    .into()
            }
            ["shell", "logcat", ..] => format!(
                "I/ntp_success( {}): [{}/192.0.2.123,2,0]\n",
                100 + s.boot,
                s.active_server
            ),
            ["shell", "setprop", "persist.adb.tcp.port", value] => {
                s.persistent = (*value).into();
                if let Some(token) = &s.cancel_after_adb {
                    token.cancel();
                }
                String::new()
            }
            ["shell", "settings", "put", "global", "ntp_server", value] => {
                s.server = (*value).into();
                String::new()
            }
            ["shell", "settings", "delete", "global", "ntp_server"] => {
                s.server.clear();
                String::new()
            }
            ["shell", "settings", "put", "global", "auto_time", "1"] => {
                if s.fail_auto_time {
                    return Err(AppError::Conflict("模拟自动校时保存失败".into()));
                }
                s.automatic = true;
                String::new()
            }
            [
                "shell",
                "settings",
                "put",
                "global",
                "stay_on_while_plugged_in",
                value,
            ] => {
                if s.fail_sleep {
                    return Err(AppError::Conflict("模拟休眠设置写失败".into()));
                }
                s.mask = value.parse().unwrap();
                String::new()
            }
            ["shell", "sync"] => String::new(),
            _ => panic!("未声明命令，禁止启动实际进程：{command:?}"),
        };
        Ok(ProcessOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

struct Context {
    state: Arc<FormalAppState>,
    _directory: tempfile::TempDir,
    project: String,
    id: String,
    plan: ScreenPlan,
    data: MaintenancePlan,
    screen: ScreenAsset,
}
async fn setup(fake: &Fake, patch: ScreenSettingsPatch) -> Context {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let size = fake.sim.lock().unwrap().size;
    let sid = repo
        .save_local(
            &project,
            &ScreenFields {
                name: "隔离执行测试屏".into(),
                ip: "192.0.2.10".into(),
                mac: "02:11:22:33:44:10".into(),
                size: size.into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let screen = repo.asset(&project, &sid).await.unwrap();
    let observed = fake
        .device()
        .inspect(&screen, "inspect", CancellationToken::new())
        .await
        .unwrap();
    let data = MaintenancePlan {
        apk: None,
        observations: BTreeMap::from([(sid.clone(), observed)]),
        request_ids: BTreeMap::new(),
        app_config: None,
        ntp: None,
        settings: Some(BTreeMap::from([(sid.clone(), patch)])),
    };
    let plan = ScreenPlan {
        project_id: project.clone(),
        input: ScreenOperationInput {
            action: "screen_settings".into(),
            target_ids: vec![sid.clone()],
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: String::new(),
            reinstall: false,
            concurrency: 1,
            expected_targets: BTreeMap::new(),
            retry_of_operation_id: None,
        },
        targets: vec![screen.clone()],
        business_project_id: None,
        data_source_id: None,
        operator: "本机测试".into(),
        instance_id: "isolated-test".into(),
        created_at: now(),
        detail: serde_json::to_value(&data).unwrap(),
    };
    let id = uuid::Uuid::now_v7().to_string();
    task_data::save_plan(state.local_store.pool(), &id, &plan)
        .await
        .unwrap();
    state
        .task_repository
        .create(CreateTask {
            id: id.clone(),
            local_project_id: project.clone(),
            remote_operation_record_id: None,
            domain_type: "smart_screen".into(),
            operation_type: "screen_settings".into(),
            name: "统一设置隔离验证".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: directory
                .path()
                .join("task.jsonl")
                .to_string_lossy()
                .into_owned(),
            targets: vec![("smart_screen".into(), sid.clone())],
        })
        .await
        .unwrap();
    for (from, to) in [
        (TaskState::Draft, TaskState::Checking),
        (TaskState::Checking, TaskState::Ready),
        (TaskState::Ready, TaskState::Queued),
        (TaskState::Queued, TaskState::Running),
    ] {
        state
            .task_repository
            .transition(&id, from, to, None, None)
            .await
            .unwrap();
    }
    *fake.context.lock().unwrap() = Some((
        state.local_store.pool().clone(),
        project.clone(),
        id.clone(),
        sid,
    ));
    fake.clear_calls();
    Context {
        state,
        _directory: directory,
        project,
        id,
        plan,
        data,
        screen,
    }
}
impl Context {
    async fn execute(&self, fake: &Fake, cancel: CancellationToken) -> ScreenTargetResult {
        let mut result = ScreenTargetResult {
            screen_id: self.screen.id.clone(),
            device: ResultState::Pending,
            ..Default::default()
        };
        screen_settings::execute_with_device(
            &self.state,
            &self.id,
            &self.plan,
            &self.data,
            &self.screen,
            None,
            &mut result,
            cancel,
            &fake.device(),
        )
        .await
        .unwrap();
        result
    }
    async fn verify(&self, fake: &Fake, result: &mut ScreenTargetResult) {
        screen_settings::verify_with_device(
            &self.state,
            &self.id,
            &self.plan,
            &self.screen,
            result,
            &fake.device(),
        )
        .await
        .unwrap();
    }
    async fn close(self) {
        support::close(self.state).await;
    }
}
fn full_patch() -> ScreenSettingsPatch {
    ScreenSettingsPatch {
        ntp: Some(NtpPatch {
            server: "ntp.test".into(),
        }),
        ..Default::default()
    }
}
fn child<'a>(result: &'a ScreenTargetResult, key: &str) -> &'a Value {
    &result.evidence["screenSettings"]["items"][key]
}
fn count_reboots(fake: &Fake) -> usize {
    fake.writes()
        .iter()
        .filter(|v| v.as_slice() == ["reboot"])
        .count()
}

#[tokio::test]
async fn each_reboot_capable_option_alone_uses_only_one_reboot() {
    for adb_only in [true, false] {
        let fake = Fake::new(Sim {
            persistent: if adb_only { "0" } else { "5555" }.into(),
            ..Sim::default()
        });
        let patch = ScreenSettingsPatch {
            adb: adb_only,
            time: false,
            sleep_off: false,
            ntp: if adb_only {
                None
            } else {
                Some(NtpPatch {
                    server: "ntp.test".into(),
                })
            },
        };
        let c = setup(&fake, patch).await;
        let result = c.execute(&fake, CancellationToken::new()).await;
        assert_eq!(result.device, ResultState::Succeeded, "{}", result.message);
        assert_eq!(count_reboots(&fake), 1);
        let writes = fake.writes();
        assert_eq!(writes.iter().flatten().any(|v| v == "setprop"), adb_only);
        assert_eq!(
            writes.iter().flatten().any(|v| v == "ntp_server"),
            !adb_only
        );
        assert!(
            !writes
                .iter()
                .flatten()
                .any(|v| v == "hwclock" || v == "stay_on_while_plugged_in")
        );
        c.close().await;
    }
}

#[tokio::test]
async fn unsupported_date_is_blocked_before_any_requested_setting_write() {
    let fake = Fake::new(Sim {
        date_supported: false,
        ..Sim::default()
    });
    let c = setup(
        &fake,
        ScreenSettingsPatch {
            adb: false,
            time: true,
            sleep_off: true,
            ntp: None,
        },
    )
    .await;
    let mut result = ScreenTargetResult {
        screen_id: c.screen.id.clone(),
        ..Default::default()
    };
    assert!(
        screen_settings::execute_with_device(
            &c.state,
            &c.id,
            &c.plan,
            &c.data,
            &c.screen,
            None,
            &mut result,
            CancellationToken::new(),
            &fake.device()
        )
        .await
        .is_err()
    );
    assert!(fake.writes().is_empty());
    c.close().await;
}

#[tokio::test]
async fn rtc_failure_stops_sleep_and_read_only_verification_does_not_rewrite_clock() {
    let fake = Fake::new(Sim {
        fail_hardware: true,
        ..Sim::default()
    });
    let c = setup(
        &fake,
        ScreenSettingsPatch {
            adb: false,
            time: true,
            sleep_off: true,
            ntp: None,
        },
    )
    .await;
    let mut result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(child(&result, "sleepOff")["state"], "not_started");
    assert!(
        !fake
            .writes()
            .iter()
            .flatten()
            .any(|v| v == "stay_on_while_plugged_in")
    );
    fake.clear_calls();
    c.verify(&fake, &mut result).await;
    assert_eq!(result.device, ResultState::Failed);
    assert!(fake.writes().is_empty());
    c.close().await;
}

#[tokio::test]
async fn sleep_failure_recovery_does_not_repeat_write_or_wake() {
    let fake = Fake::new(Sim {
        fail_sleep: true,
        ..Sim::default()
    });
    let c = setup(
        &fake,
        ScreenSettingsPatch {
            adb: false,
            time: false,
            sleep_off: true,
            ntp: None,
        },
    )
    .await;
    let mut result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(fake.writes().len(), 1);
    assert_eq!(child(&result, "sleepOff")["state"], "unknown");
    fake.clear_calls();
    c.verify(&fake, &mut result).await;
    assert_eq!(result.device, ResultState::Failed);
    assert!(fake.writes().is_empty());
    c.close().await;
}

#[tokio::test]
async fn adb_and_ntp_are_saved_before_one_reboot_then_rtc_and_sleep_are_set() {
    let fake = Fake::new(Sim::default());
    let c = setup(&fake, full_patch()).await;
    let result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(result.device, ResultState::Succeeded, "{}", result.message);
    for key in ["adb", "ntp", "time", "sleepOff"] {
        assert_eq!(
            child(&result, key)["state"],
            "succeeded",
            "{key}: {}",
            result.evidence
        );
    }
    let writes = fake.writes();
    assert_eq!(count_reboots(&fake), 1);
    let position = |key: &str| {
        writes
            .iter()
            .position(|v| v.iter().any(|s| s == key))
            .unwrap()
    };
    assert!(position("setprop") < position("ntp_server"));
    assert!(position("auto_time") < position("reboot"));
    assert!(position("reboot") < position("hwclock"));
    assert!(position("hwclock") < position("stay_on_while_plugged_in"));
    assert_ne!(
        result.evidence["screenSettings"]["reboot"]["beforeBootId"],
        result.evidence["screenSettings"]["reboot"]["afterBootId"]
    );
    assert_eq!(fake.sim.lock().unwrap().mask, 7);
    c.close().await;
}

#[tokio::test]
async fn already_satisfied_four_and_ten_have_zero_writes_or_reboots() {
    for size in ["4", "10"] {
        let fake = Fake::new(Sim {
            size,
            persistent: if size == "4" {
                String::new()
            } else {
                "5555".into()
            },
            hardware_offset: 0,
            mask: 7,
            ..Sim::default()
        });
        let c = setup(&fake, ScreenSettingsPatch::default()).await;
        let result = c.execute(&fake, CancellationToken::new()).await;
        assert_eq!(result.device, ResultState::Skipped, "{}", result.message);
        assert!(fake.writes().is_empty(), "{:?}", fake.writes());
        assert_eq!(child(&result, "ntp")["state"], "not_selected");
        c.close().await;
    }
}

#[tokio::test]
async fn ntp_save_failure_keeps_adb_evidence_and_recovery_never_finishes_unstarted_steps() {
    let fake = Fake::new(Sim {
        fail_auto_time: true,
        ..Sim::default()
    });
    let c = setup(&fake, full_patch()).await;
    let mut result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(fake.sim.lock().unwrap().persistent, "5555");
    assert_eq!(fake.sim.lock().unwrap().server, "ntp.test");
    assert_eq!(count_reboots(&fake), 0);
    assert_eq!(child(&result, "time")["state"], "not_started");
    assert_eq!(child(&result, "sleepOff")["state"], "not_started");
    fake.clear_calls();
    c.verify(&fake, &mut result).await;
    assert_eq!(result.device, ResultState::Failed);
    assert!(fake.writes().is_empty());
    assert_eq!(child(&result, "adb")["state"], "failed");
    assert_eq!(child(&result, "ntp")["state"], "failed");
    c.close().await;
}

#[tokio::test]
async fn cancellation_after_adb_write_does_not_start_ntp_or_reboot() {
    let cancel = CancellationToken::new();
    let fake = Fake::new(Sim {
        cancel_after_adb: Some(cancel.clone()),
        ..Sim::default()
    });
    let c = setup(&fake, full_patch()).await;
    let mut result = c.execute(&fake, cancel).await;
    assert_eq!(fake.writes().len(), 1);
    assert_eq!(count_reboots(&fake), 0);
    assert_eq!(child(&result, "ntp")["state"], "not_started");
    fake.clear_calls();
    c.verify(&fake, &mut result).await;
    assert!(fake.writes().is_empty());
    assert!(!matches!(
        result.device,
        ResultState::Pending | ResultState::Unknown
    ));
    c.close().await;
}

#[tokio::test]
async fn lost_checkpoint_after_adb_save_prevents_ntp_and_reboot() {
    let fake = Fake::new(Sim::default());
    let c = setup(&fake, full_patch()).await;
    sqlx::query("CREATE TRIGGER stop_after_adb BEFORE UPDATE ON local_screen_task_data WHEN instr(NEW.result_json,'screen_settings_adb_saved')>0 BEGIN SELECT RAISE(ABORT,'模拟检查点不可用'); END").execute(c.state.local_store.pool()).await.unwrap();
    let result = c.execute(&fake, CancellationToken::new()).await;
    assert!(matches!(
        result.device,
        ResultState::Unknown | ResultState::Failed
    ));
    assert_eq!(fake.writes().len(), 1);
    assert_eq!(count_reboots(&fake), 0);
    c.close().await;
}

#[tokio::test]
async fn reboot_completed_before_result_saved_is_only_read_back_and_not_replayed() {
    let fake = Fake::new(Sim::default());
    let c = setup(&fake, full_patch()).await;
    assert_eq!(
        c.execute(&fake, CancellationToken::new()).await.device,
        ResultState::Succeeded
    );
    let mut interrupted = fake
        .reboot_snapshot
        .lock()
        .unwrap()
        .clone()
        .expect("重启发出前必须已保存完整快照");
    assert!(interrupted.evidence["screenSettings"]["reboot"]["afterBootId"].is_null());
    // 重建进程恰在共享重启完成后停止的设备条件，后面的时间与睡眠从未执行。
    {
        let mut s = fake.sim.lock().unwrap();
        s.hardware_offset = -120;
        s.mask = 0;
    }
    task_data::save_target(c.state.local_store.pool(), &c.project, &c.id, &interrupted)
        .await
        .unwrap();
    fake.clear_calls();
    c.verify(&fake, &mut interrupted).await;
    assert!(fake.writes().is_empty());
    assert_eq!(child(&interrupted, "adb")["state"], "succeeded");
    assert_eq!(child(&interrupted, "ntp")["state"], "succeeded");
    assert_eq!(child(&interrupted, "time")["state"], "not_started");
    assert_eq!(child(&interrupted, "sleepOff")["state"], "not_started");
    assert!(!matches!(
        interrupted.device,
        ResultState::Pending | ResultState::Unknown | ResultState::Succeeded
    ));
    assert!(!interrupted.evidence["screenSettings"]["reboot"]["afterBootId"].is_null());
    c.close().await;
}

#[tokio::test]
async fn replacement_device_after_reboot_never_receives_time_or_sleep_writes() {
    let fake = Fake::new(Sim {
        replace_after_boot: true,
        ..Sim::default()
    });
    let c = setup(&fake, full_patch()).await;
    let result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(count_reboots(&fake), 1);
    assert_ne!(result.device, ResultState::Succeeded);
    assert!(
        !fake
            .writes()
            .iter()
            .flatten()
            .any(|v| v == "hwclock" || v == "stay_on_while_plugged_in")
    );
    c.close().await;
}

#[tokio::test]
async fn ntp_only_recovery_keeps_unconfirmed_shared_reboot_unknown_even_with_fresh_time_source() {
    let fake = Fake::new(Sim {
        persistent: "5555".into(),
        ..Sim::default()
    });
    let c = setup(
        &fake,
        ScreenSettingsPatch {
            adb: false,
            time: false,
            sleep_off: false,
            ntp: Some(NtpPatch {
                server: "ntp.test".into(),
            }),
        },
    )
    .await;
    assert_eq!(
        c.execute(&fake, CancellationToken::new()).await.device,
        ResultState::Succeeded
    );
    let mut interrupted = fake.reboot_snapshot.lock().unwrap().clone().unwrap();
    // 请求已持久化，但新进程未证实重启发生；即使运行源现在符合目标也不能抹去未知的重启结果。
    {
        let mut s = fake.sim.lock().unwrap();
        s.boot = 1;
        s.active_server = "ntp.test".into();
    }
    task_data::save_target(c.state.local_store.pool(), &c.project, &c.id, &interrupted)
        .await
        .unwrap();
    fake.clear_calls();
    c.verify(&fake, &mut interrupted).await;
    assert_eq!(
        interrupted.device,
        ResultState::Unknown,
        "{}",
        interrupted.evidence
    );
    assert!(interrupted.evidence["screenSettings"]["reboot"]["afterBootId"].is_null());
    assert!(fake.writes().is_empty());
    c.close().await;
}

#[tokio::test]
async fn app_recovery_failure_retains_ntp_result_and_stops_later_settings() {
    let fake = Fake::new(Sim {
        app_present: true,
        app_running: true,
        stop_app_on_boot: true,
        ..Sim::default()
    });
    let c = setup(&fake, full_patch()).await;
    let result = c.execute(&fake, CancellationToken::new()).await;
    assert_eq!(count_reboots(&fake), 1);
    assert_eq!(result.device, ResultState::Failed, "{}", result.message);
    assert_eq!(child(&result, "adb")["state"], "failed");
    assert_eq!(child(&result, "ntp")["state"], "succeeded");
    assert_eq!(child(&result, "time")["state"], "not_started");
    assert_eq!(child(&result, "sleepOff")["state"], "not_started");
    c.close().await;
}

#[tokio::test]
async fn recovery_requires_boot_completed_even_when_boot_id_and_both_settings_match() {
    let fake = Fake::new(Sim::default());
    let c = setup(
        &fake,
        ScreenSettingsPatch {
            adb: true,
            time: false,
            sleep_off: false,
            ntp: Some(NtpPatch {
                server: "ntp.test".into(),
            }),
        },
    )
    .await;
    assert_eq!(
        c.execute(&fake, CancellationToken::new()).await.device,
        ResultState::Succeeded
    );
    let mut interrupted = fake.reboot_snapshot.lock().unwrap().clone().unwrap();
    fake.sim.lock().unwrap().boot_completed = false;
    task_data::save_target(c.state.local_store.pool(), &c.project, &c.id, &interrupted)
        .await
        .unwrap();
    fake.clear_calls();
    c.verify(&fake, &mut interrupted).await;
    assert_eq!(
        interrupted.device,
        ResultState::Unknown,
        "{}",
        interrupted.evidence
    );
    assert!(interrupted.evidence["screenSettings"]["reboot"]["afterBootId"].is_null());
    assert!(fake.writes().is_empty());
    c.close().await;
}
