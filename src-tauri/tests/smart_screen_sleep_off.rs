#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::{
        ports::project_management::ProjectManagementPort, smart_screen::operations::validate_input,
    },
    core::error::{AppError, AppResult},
    domain::smart_screen::{
        model::ScreenFields,
        operation::{READ_ACTIONS, ScreenOperationInput, WRITE_ACTIONS, action_label},
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::{ScreenRepository, now},
        smart_screen::{
            device::{AdbDevice, AndroidTools, DeviceCommandPort, ProcessOutput},
            maintenance, task_data,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const FOUR_MAC_READ: &str = "if [ ! -d /sys/class/net/eth0 ] || [ -d /sys/class/net/eth0/wireless ] || [ -e /sys/class/net/eth0/phy80211 ]; then exit 2; fi; read t < /sys/class/net/eth0/type; if [ x$t != x1 ]; then exit 2; fi; cat /sys/class/net/eth0/address";
const TEN_MAC_READ: &str = r#"for p in /sys/class/net/eth*; do if [ -r "$p/address" ] && [ ! -d "$p/wireless" ] && [ ! -e "$p/phy80211" ]; then read t < "$p/type"; if [ x$t = x1 ]; then echo "${p##*/}=$(cat "$p/address")"; fi; fi; done"#;

#[derive(Clone)]
struct ScreenCase {
    number: u8,
    size: &'static str,
    mask: u32,
    powered: bool,
    asleep: bool,
    uid: u32,
    permission: bool,
}

impl ScreenCase {
    fn ip(&self) -> String {
        format!("192.0.2.{}", self.number)
    }

    fn mac(&self) -> String {
        format!("02:11:22:33:44:{:02X}", self.number)
    }

    fn power(&self) -> String {
        let admin_max = if self.size == "4" {
            i32::MAX as u64
        } else {
            i64::MAX as u64
        };
        format!(
            "POWER MANAGER (dumpsys power)\n  mWakefulness={}\n  mIsPowered={}\n  mPlugType={}\n  mStayOnWhilePluggedInSetting={}\n  mStayOn={}\n  mMaximumScreenOffTimeoutFromDeviceAdmin={} (enforced=false)\nDisplay Power: state={}\n",
            if self.asleep { "Asleep" } else { "Awake" },
            self.powered,
            if self.powered { 1 } else { 0 },
            self.mask,
            self.powered && self.mask & 1 != 0,
            admin_max,
            if self.asleep { "OFF" } else { "ON" },
        )
    }
}

#[derive(Clone, Debug)]
struct Call {
    ip: String,
    args: Vec<String>,
}

#[derive(Clone)]
struct MockCommands {
    screens: BTreeMap<String, ScreenCase>,
    calls: Arc<Mutex<Vec<Call>>>,
}

impl MockCommands {
    fn new(cases: &[ScreenCase]) -> Self {
        Self {
            screens: cases
                .iter()
                .map(|screen| (screen.ip(), screen.clone()))
                .collect(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn device(&self) -> AdbDevice<Self> {
        AdbDevice::with_commands(
            AndroidTools {
                adb: "fixture-adb-never-spawned".into(),
            },
            self.clone(),
        )
    }

    fn calls_for(&self, ip: &str) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| call.ip == ip)
            .map(|call| call.args.clone())
            .collect()
    }

    fn assert_read_only(&self) {
        for call in self.calls.lock().unwrap().iter() {
            assert!(
                !call.args.iter().any(|part| matches!(
                    part.as_str(),
                    "put" | "delete" | "root" | "su" | "reboot" | "input" | "content" | "am"
                )),
                "休眠检查不得产生写入、提权、唤醒、重启或APP管理调用：{call:?}"
            );
        }
    }
}

impl DeviceCommandPort for MockCommands {
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
        let is_ping = program.file_name().is_some_and(|name| {
            let name = name.to_string_lossy();
            name.eq_ignore_ascii_case("ping") || name.eq_ignore_ascii_case("ping.exe")
        });
        if !is_ping {
            assert_eq!(program, Path::new("fixture-adb-never-spawned"));
        }
        let ip = if is_ping {
            args.last().expect("Ping必须包含固定测试IP").clone()
        } else if args.first().is_some_and(|value| value == "connect") {
            assert_eq!(args.len(), 2);
            args[1]
                .strip_suffix(":5555")
                .expect("只允许测试5555端口")
                .to_string()
        } else {
            assert_eq!(args.first().map(String::as_str), Some("-s"));
            args[1]
                .strip_suffix(":5555")
                .expect("只允许测试5555端口")
                .to_string()
        };
        let screen = self.screens.get(&ip).expect("只允许明确列出的测试屏");
        self.calls.lock().unwrap().push(Call {
            ip: ip.clone(),
            args: args.clone(),
        });
        let stdout = if is_ping {
            format!("Reply from {ip}: bytes=32 time<1ms TTL=64")
        } else if args.first().is_some_and(|value| value == "connect") {
            "connected".into()
        } else {
            let arguments = args[2..].iter().map(String::as_str).collect::<Vec<_>>();
            match arguments.as_slice() {
                ["get-state"] => "device".into(),
                ["shell", FOUR_MAC_READ] if screen.size == "4" => screen.mac(),
                ["shell", TEN_MAC_READ] if screen.size == "10" => format!("eth0={}", screen.mac()),
                ["shell", "getprop", property] => match *property {
                    "ro.build.version.release" => {
                        if screen.size == "4" { "8.1.0" } else { "10" }.into()
                    }
                    "ro.product.model" => if screen.size == "4" {
                        "px30_evb"
                    } else {
                        "Smart touch panel"
                    }
                    .into(),
                    "ro.build.fingerprint" => if screen.size == "4" {
                        "rockchip/px30_evb/px30_evb:8.1.0/test"
                    } else {
                        "STATION/ceres_c4/ceres-c3:10/test"
                    }
                    .into(),
                    "ro.build.version.sdk" => if screen.size == "4" { "27" } else { "29" }.into(),
                    "ro.product.cpu.abilist" => "armeabi-v7a,arm64-v8a".into(),
                    "persist.adb.tcp.port" => "5555".into(),
                    "persist.sys.timezone" => "Asia/Shanghai".into(),
                    _ => panic!("休眠预检发出未知属性读取：{property}"),
                },
                // 系统维护不要求小新已经安装，也不应转入小新Provider。
                ["shell", "pm", "path", "chat.xiaoxin.app"] => String::new(),
                ["shell", "df", "-k", "/data"] => {
                    "/dev/block/data 100000 1000 99000 1% /data".into()
                }
                ["shell", "date", "-u", "+%Y-%m-%dT%H:%M:%SZ"] => now(),
                [
                    "shell",
                    "settings",
                    "get",
                    "global",
                    "auto_time" | "auto_time_zone",
                ] => "1".into(),
                [
                    "shell",
                    "settings",
                    "get",
                    "global",
                    "stay_on_while_plugged_in",
                ] => screen.mask.to_string(),
                ["shell", "settings", "get", "system", "screen_off_timeout"] => {
                    if screen.size == "4" {
                        "60000"
                    } else {
                        "2147483647"
                    }
                    .into()
                }
                ["shell", "dumpsys", "power"] => screen.power(),
                ["shell", "id", "-u"] => screen.uid.to_string(),
                ["shell", "dumpsys", "package", "com.android.shell"] => format!(
                    "Packages:\n  Package [com.android.shell] (fixture):\n    android.permission.WRITE_SECURE_SETTINGS: granted={}\n",
                    screen.permission
                ),
                _ => panic!("未声明的测试命令，禁止调用真实进程：{args:?}"),
            }
        };
        Ok(ProcessOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

fn input(ids: Vec<String>) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "sleep_off".into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: String::new(),
        reinstall: false,
        concurrency: 3,
        expected_targets: BTreeMap::new(),
        retry_of_operation_id: None,
    }
}

async fn assets(state: &FormalAppState, cases: &[ScreenCase]) -> (String, Vec<String>) {
    let project = Stage75Adapter::new(state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repository = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    for screen in cases {
        ids.push(
            repository
                .save_local(
                    &project,
                    &ScreenFields {
                        name: format!("隔离{}寸屏{}", screen.size, screen.number),
                        ip: screen.ip(),
                        mac: screen.mac(),
                        size: screen.size.into(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap(),
        );
    }
    (project, ids)
}

fn ready(number: u8, size: &'static str) -> ScreenCase {
    ScreenCase {
        number,
        size,
        mask: 0,
        powered: true,
        asleep: false,
        uid: 0,
        permission: false,
    }
}

#[tokio::test]
async fn mixed_screens_preflight_only_plans_not_disabled_targets_with_access() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [
        ready(11, "4"),
        ScreenCase {
            mask: 7,
            asleep: true,
            uid: 10001,
            ..ready(12, "10")
        },
        ScreenCase {
            powered: false,
            ..ready(13, "4")
        },
        ScreenCase {
            uid: 2000,
            permission: false,
            ..ready(14, "10")
        },
    ];
    let (project, ids) = assets(&state, &cases).await;
    let mock = MockCommands::new(&cases);
    let preview = maintenance::preflight_with_device_factory(
        &state,
        &project,
        input(ids.clone()),
        None,
        None,
        || Ok(mock.device()),
    )
    .await
    .unwrap();
    assert_eq!(
        preview
            .items
            .iter()
            .map(|item| item.screen_id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(
        preview
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        ["ready", "skip", "blocked", "blocked"]
    );
    assert!(preview.items[0].reason.contains("尚未关闭"));
    assert!(preview.items[1].reason.contains("已关闭"));
    assert!(preview.items[2].reason.contains("未接电"));
    assert!(preview.items[3].reason.contains("权限"));
    let (plan, _) = task_data::read_preflight_plan(state.local_store.pool(), &project, &preview.id)
        .await
        .unwrap();
    assert_eq!(plan.input.action, "sleep_off");
    assert_eq!(
        plan.targets
            .iter()
            .map(|screen| screen.id.as_str())
            .collect::<Vec<_>>(),
        [ids[0].as_str()]
    );
    assert_eq!(plan.detail["observations"].as_object().unwrap().len(), 4);
    assert!(
        preview
            .items
            .iter()
            .all(|item| item.observation.as_ref().unwrap().app_installed == Some(false))
    );
    for index in [1, 2] {
        assert!(
            !mock
                .calls_for(&cases[index].ip())
                .iter()
                .any(|args| args.iter().any(|value| value == "id")),
            "已关及无供电目标不应要求写权限"
        );
    }
    assert!(
        mock.calls_for(&cases[3].ip())
            .iter()
            .any(|args| args.ends_with(&[
                "dumpsys".into(),
                "package".into(),
                "com.android.shell".into()
            ]))
    );
    mock.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn mixed_android_versions_can_both_be_ready_without_installing_xiaoxin() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [
        ready(21, "4"),
        ScreenCase {
            uid: 2000,
            permission: true,
            ..ready(22, "10")
        },
    ];
    let (project, ids) = assets(&state, &cases).await;
    let mock = MockCommands::new(&cases);
    let preview = maintenance::preflight_with_device_factory(
        &state,
        &project,
        input(ids.clone()),
        None,
        None,
        || Ok(mock.device()),
    )
    .await
    .unwrap();
    assert!(preview.items.iter().all(|item| item.state == "ready"));
    let (plan, _) = task_data::read_preflight_plan(state.local_store.pool(), &project, &preview.id)
        .await
        .unwrap();
    assert_eq!(
        plan.targets
            .iter()
            .map(|screen| screen.id.clone())
            .collect::<Vec<_>>(),
        ids
    );
    assert_eq!(preview.items[0].observation.as_ref().unwrap().sdk, Some(27));
    assert_eq!(preview.items[1].observation.as_ref().unwrap().sdk, Some(29));
    for (item, screen) in preview.items.iter().zip(&cases) {
        assert_eq!(
            item.observation.as_ref().unwrap().observed_mac.as_deref(),
            Some(screen.mac().as_str())
        );
    }
    mock.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn all_already_disabled_screens_have_no_executable_preview_or_permission_reads() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [
        ScreenCase {
            mask: 7,
            uid: 10001,
            ..ready(31, "4")
        },
        ScreenCase {
            mask: 7,
            asleep: true,
            uid: 10001,
            ..ready(32, "10")
        },
    ];
    let (project, ids) = assets(&state, &cases).await;
    let mock = MockCommands::new(&cases);
    let preview = maintenance::preflight_with_device_factory(
        &state,
        &project,
        input(ids),
        None,
        None,
        || Ok(mock.device()),
    )
    .await
    .unwrap();
    assert!(
        preview
            .items
            .iter()
            .all(|item| item.state == "skip" && item.reason.contains("已关闭"))
    );
    assert!(
        task_data::read_preflight_plan(state.local_store.pool(), &project, &preview.id)
            .await
            .is_err()
    );
    let plans: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data WHERE local_project_id=?")
            .bind(&project)
            .fetch_one(state.local_store.pool())
            .await
            .unwrap();
    assert_eq!(plans, 0);
    assert!(
        state
            .task_repository
            .list_recent(&project, 100)
            .await
            .unwrap()
            .is_empty()
    );
    for screen in &cases {
        assert!(!mock.calls_for(&screen.ip()).iter().any(|args| {
            args.iter()
                .any(|value| value == "id" || value == "com.android.shell")
        }));
    }
    mock.assert_read_only();
    support::close(state).await;
}

#[test]
fn sleep_off_is_an_explicit_write_action_and_keeps_general_input_guards() {
    let checked = input(vec!["screen-1".into()]);
    assert!(WRITE_ACTIONS.contains(&"sleep_off"));
    assert!(!READ_ACTIONS.contains(&"sleep_off"));
    assert_eq!(action_label("sleep_off"), "关闭屏睡眠");
    assert!(validate_input(&checked).is_ok());
    for invalid in [
        ScreenOperationInput {
            action: "sleep-on".into(),
            ..checked.clone()
        },
        ScreenOperationInput {
            target_ids: Vec::new(),
            ..checked.clone()
        },
        ScreenOperationInput {
            target_ids: vec!["screen-1".into(), "screen-1".into()],
            ..checked.clone()
        },
        ScreenOperationInput {
            concurrency: 4,
            ..checked.clone()
        },
    ] {
        assert!(validate_input(&invalid).is_err());
    }
}
