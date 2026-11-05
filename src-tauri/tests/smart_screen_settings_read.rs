#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::{AppError, AppResult},
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::{ResultState, ScreenAsset, ScreenFields, ScreenTargetResult},
            ntp::NtpPatch,
            operation::{ScreenOperationInput, ScreenPlan},
            settings::ScreenSettingsPatch,
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::{
            screen_repository::{ScreenRepository, now},
            task_repository::CreateTask,
        },
        smart_screen::{
            device::{AdbDevice, AndroidTools, DeviceCommandPort, ProcessOutput},
            maintenance, screen_settings, task_data,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Clone)]
struct Screen {
    number: u8,
    size: &'static str,
    known_firmware: bool,
    persistent: &'static str,
    service: &'static str,
    firmware_boot: bool,
    server: &'static str,
    automatic: bool,
    source_event: bool,
    cache_age: u64,
}
impl Screen {
    fn satisfied(number: u8, size: &'static str) -> Self {
        Self {
            number,
            size,
            known_firmware: true,
            persistent: "5555",
            service: "5555",
            firmware_boot: true,
            server: "ntp.internal",
            automatic: true,
            source_event: true,
            cache_age: 100,
        }
    }
    fn ip(&self) -> String {
        format!("192.0.2.{}", self.number)
    }
    fn mac(&self) -> String {
        format!("02:11:22:33:44:{:02X}", self.number)
    }
    fn boot(&self) -> String {
        format!("00000000-0000-4000-8000-{:012}", self.number)
    }
    fn pid(&self) -> u64 {
        700 + u64::from(self.number)
    }
}

#[derive(Clone)]
struct ReadOnlyCommands {
    screens: BTreeMap<String, Screen>,
    calls: Arc<Mutex<Vec<Vec<String>>>>,
}
impl ReadOnlyCommands {
    fn new(screens: &[Screen]) -> Self {
        Self {
            screens: screens.iter().map(|s| (s.ip(), s.clone())).collect(),
            calls: Default::default(),
        }
    }
    fn device(&self) -> AdbDevice<Self> {
        AdbDevice::with_commands(
            AndroidTools {
                adb: "never-run-real-settings-adb".into(),
            },
            self.clone(),
        )
    }
    fn assert_read_only(&self) {
        assert!(
            self.calls
                .lock()
                .unwrap()
                .iter()
                .all(|args| !args.iter().any(|arg| matches!(
                    arg.as_str(),
                    "put"
                        | "delete"
                        | "setprop"
                        | "reboot"
                        | "input"
                        | "am"
                        | "content"
                        | "su"
                        | "root"
                        | "-w"
                ) || arg.starts_with('@')))
        );
    }
}
impl DeviceCommandPort for ReadOnlyCommands {
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
        let ping = program.file_name().is_some_and(|name| {
            matches!(
                name.to_string_lossy().to_ascii_lowercase().as_str(),
                "ping" | "ping.exe"
            )
        });
        if !ping {
            assert_eq!(program, Path::new("never-run-real-settings-adb"));
        }
        let ip = if ping {
            args.last().unwrap().clone()
        } else if args.first().is_some_and(|arg| arg == "connect") {
            args[1].strip_suffix(":5555").unwrap().to_string()
        } else {
            assert_eq!(args.first().map(String::as_str), Some("-s"));
            args[1].strip_suffix(":5555").unwrap().to_string()
        };
        let screen = self.screens.get(&ip).expect("只允许已声明的隔离屏");
        let command = if ping {
            vec!["ping"]
        } else if args.first().is_some_and(|arg| arg == "connect") {
            vec!["connect"]
        } else {
            args[2..].iter().map(String::as_str).collect::<Vec<_>>()
        };
        self.calls
            .lock()
            .unwrap()
            .push(command.iter().map(|part| part.to_string()).collect());
        let stdout=match command.as_slice() {
            ["ping"]=>format!("Reply from {ip}: bytes=32 time<1ms TTL=64"),
            ["connect"]=>"connected".into(), ["get-state"]=>"device".into(),
            ["shell",cmd] if cmd.contains("/sys/class/net/eth0/address")=>screen.mac(),
            ["shell",cmd] if cmd.contains("for p in /sys/class/net/eth*")=>format!("eth0={}",screen.mac()),
            ["shell","getprop",property]=>match *property {
                "ro.build.version.release"=>if screen.size=="4" {"8.1.0"}else{"10"}.into(),
                "ro.build.version.sdk"=>if screen.size=="4" {"27"}else{"29"}.into(),
                "ro.product.model"=>if screen.size=="4" {"px30_evb"}else{"Smart touch panel"}.into(),
                "ro.build.fingerprint"=>if !screen.known_firmware {"unknown/firmware"}else if screen.size=="4" {"rockchip/px30_evb/px30_evb:8.1.0/test"}else{"STATION/ceres_c4/ceres-c3:10/test"}.into(),
                "ro.product.cpu.abilist"=>"armeabi-v7a,arm64-v8a".into(),
                "persist.adb.tcp.port"=>screen.persistent.into(),"service.adb.tcp.port"=>screen.service.into(),
                "persist.sys.timezone"=>"Asia/Shanghai".into(),"sys.boot_completed"=>"1".into(),
                _=>panic!("未声明属性：{property}"),
            },
            ["shell","id"]=>"uid=0(root)".into(),["shell","id","-u"]=>"0".into(),
            ["shell","pm","path","chat.xiaoxin.app"]=>String::new(),
            ["shell","df","-k","/data"]=>"/dev/block/data 100000 1000 99000 1% /data".into(),
            ["shell","date","-u","+%Y-%m-%dT%H:%M:%SZ"]=>now(),
            ["shell","date","--help"]=>"date @UNIXTIME".into(),
            ["shell","hwclock","--help"]=>"--systohc --utc --rtc".into(),
            ["shell","ls","/sys/class/rtc"]=>"rtc0".into(),
            ["shell","cat","/sys/class/rtc/rtc0/name"]=>if screen.size=="4" {"rk808-rtc"}else{"rtc-pcf85063"}.into(),
            ["shell","cat","/sys/class/rtc/rtc0/hctosys"]=>"1".into(),
            ["shell","cat","/sys/class/rtc/rtc0/since_epoch"]=>time::OffsetDateTime::now_utc().unix_timestamp().to_string(),
            ["shell","sh","-c",cmd] if cmd.starts_with("'test -c /dev/rtc0")=>String::new(),
            ["shell","cat","/proc/sys/kernel/random/boot_id"]=>screen.boot(),
            ["shell","cat","/proc/uptime"]=>"3600.00 100.00".into(),
            ["shell","cat","/vendor/build.prop"]=>if screen.firmware_boot {"service.adb.tcp.port=5555\n"}else{""}.into(),
            ["shell","settings","get","global",key]=>match *key {
                "ntp_server"=>screen.server.into(),"auto_time"=>if screen.automatic {"1"}else{"0"}.into(),
                "auto_time_zone"=>"0".into(),"stay_on_while_plugged_in"=>"7".into(),
                _=>panic!("未声明设置：{key}"),
            },
            ["shell","settings","get","system","screen_off_timeout"]=>"60000".into(),
            ["shell","dumpsys","power"]=>"POWER MANAGER (dumpsys power)\n mWakefulness=Awake\n mIsPowered=true\n mPlugType=1\n mStayOnWhilePluggedInSetting=7\n mStayOn=true\n mMaximumScreenOffTimeoutFromDeviceAdmin=2147483647 (enforced=false)\nDisplay Power: state=ON\n".into(),
            ["shell","dumpsys","network_time_update_service"]=>format!("PollingIntervalMs: +1d0h0m0s0ms\nNTP cache age: {}\nNTP cache certainty: 5\n",screen.cache_age),
            ["shell","pidof","system_server"]=>screen.pid().to_string(),
            ["shell","logcat","-d","-b","events","-v","brief","-t","200","-s","ntp_success"]=>if screen.source_event {format!("I/ntp_success( {}): [{}/192.0.2.123,2,0]\n",screen.pid(),screen.server)}else{String::new()},
            _=>panic!("禁止未声明命令或真实副作用：{command:?}"),
        };
        Ok(ProcessOutput {
            success: true,
            stdout,
            stderr: String::new(),
        })
    }
}

async fn assets(
    state: &FormalAppState,
    screens: &[Screen],
    empty_mac: bool,
) -> (String, Vec<ScreenAsset>) {
    let project = Stage75Adapter::new(state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut rows = Vec::new();
    for screen in screens {
        let id = repo
            .save_local(
                &project,
                &ScreenFields {
                    name: format!("隔离{}寸屏{}", screen.size, screen.number),
                    ip: screen.ip(),
                    size: screen.size.into(),
                    mac: if empty_mac {
                        String::new()
                    } else {
                        screen.mac()
                    },
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        rows.push(repo.asset(&project, &id).await.unwrap());
    }
    (project, rows)
}
fn input(screens: &[ScreenAsset]) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "screen_settings".into(),
        target_ids: screens.iter().map(|s| s.id.clone()).collect(),
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
fn ntp_patch(adb: bool) -> ScreenSettingsPatch {
    ScreenSettingsPatch {
        adb,
        time: false,
        sleep_off: false,
        ntp: Some(NtpPatch {
            server: "ntp.internal".into(),
        }),
    }
}
async fn preflight(
    state: &FormalAppState,
    project: &str,
    screens: &[ScreenAsset],
    patch: ScreenSettingsPatch,
    commands: &ReadOnlyCommands,
) -> inxaiot_desk_buddy_lib::domain::smart_screen::operation::ScreenPreflight {
    maintenance::preflight_settings_with_device_factory(
        state,
        project,
        input(screens),
        None,
        None,
        Some(
            screens
                .iter()
                .map(|s| (s.id.clone(), patch.clone()))
                .collect(),
        ),
        || Ok(commands.device()),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn default_read_keeps_satisfied_screens_with_observed_identity_and_all_skip_creates_no_task()
{
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [Screen::satisfied(41, "4"), Screen::satisfied(42, "10")];
    let commands = ReadOnlyCommands::new(&cases);
    let (project, screens) = assets(&state, &cases, true).await;
    assert!(screens.iter().all(|s| s.fields.mac.is_empty()));
    let rows = screen_settings::read_with_device(
        &state,
        &project,
        &input(&screens).target_ids,
        &commands.device(),
    )
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert!(row.error.is_none());
        assert_eq!(row.pending_count, 0);
        assert_eq!(row.blocked_count, 0);
        for code in ["adb", "time", "sleepOff"] {
            assert_eq!(row.items[code].state, "skip");
        }
        assert_eq!(row.items["ntp"].state, "not_selected");
    }
    let checked = preflight(
        &state,
        &project,
        &screens,
        ScreenSettingsPatch::default(),
        &commands,
    )
    .await;
    assert_eq!(checked.items.len(), 2);
    assert!(
        checked
            .items
            .iter()
            .all(|item| item.state == "skip" && item.settings.is_some())
    );
    assert!(
        task_data::read_preflight_plan(state.local_store.pool(), &project, &checked.id)
            .await
            .is_err()
    );
    assert!(
        state
            .task_repository
            .list_recent(&project, 100)
            .await
            .unwrap()
            .is_empty()
    );
    commands.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn equal_ntp_address_requires_automatic_time_and_a_current_sync_sample() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [
        Screen::satisfied(51, "10"),
        Screen {
            automatic: false,
            ..Screen::satisfied(52, "10")
        },
        Screen {
            cache_age: 4_000_000,
            ..Screen::satisfied(53, "10")
        },
    ];
    let (project, screens) = assets(&state, &cases, false).await;
    let commands = ReadOnlyCommands::new(&cases);
    let checked = preflight(&state, &project, &screens, ntp_patch(false), &commands).await;
    assert_eq!(
        checked
            .items
            .iter()
            .map(|item| item.state.as_str())
            .collect::<Vec<_>>(),
        ["skip", "ready", "ready"]
    );
    let selected = checked
        .items
        .iter()
        .map(|item| &item.settings.as_ref().unwrap().items["ntp"])
        .collect::<Vec<_>>();
    assert!(!selected[0].reboot_required);
    assert!(selected[1].reboot_required);
    assert!(selected[2].reboot_required);
    assert_eq!(selected[0].current["syncConfirmed"], true);
    assert_eq!(selected[2].current["syncConfirmed"], false);
    commands.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn ntp_reboot_dependency_is_repairable_only_when_adb_is_selected() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [Screen {
        persistent: "0",
        automatic: false,
        ..Screen::satisfied(61, "10")
    }];
    let (project, screens) = assets(&state, &cases, false).await;
    let commands = ReadOnlyCommands::new(&cases);
    let blocked = preflight(&state, &project, &screens, ntp_patch(false), &commands).await;
    assert_eq!(blocked.items[0].state, "blocked");
    assert_eq!(
        blocked.items[0].settings.as_ref().unwrap().items["ntp"].state,
        "blocked"
    );
    let repaired = preflight(&state, &project, &screens, ntp_patch(true), &commands).await;
    assert_eq!(repaired.items[0].state, "ready");
    let row = repaired.items[0].settings.as_ref().unwrap();
    assert_eq!(row.items["adb"].state, "ready");
    assert_eq!(row.items["ntp"].state, "ready");
    assert!(row.items["adb"].reboot_required && row.items["ntp"].reboot_required);
    commands.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn unknown_firmware_does_not_count_matching_ports_as_a_verified_adb_setting() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [Screen {
        known_firmware: false,
        ..Screen::satisfied(71, "10")
    }];
    let (project, screens) = assets(&state, &cases, false).await;
    let commands = ReadOnlyCommands::new(&cases);
    let checked = preflight(
        &state,
        &project,
        &screens,
        ScreenSettingsPatch {
            adb: true,
            time: false,
            sleep_off: false,
            ntp: None,
        },
        &commands,
    )
    .await;
    assert_eq!(checked.items[0].state, "blocked");
    assert_eq!(
        checked.items[0].settings.as_ref().unwrap().items["adb"].state,
        "blocked"
    );
    commands.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn known_four_inch_uses_verified_factory_boot_port_without_writing_a_property() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [Screen {
        persistent: "",
        ..Screen::satisfied(81, "4")
    }];
    let (project, screens) = assets(&state, &cases, false).await;
    let commands = ReadOnlyCommands::new(&cases);
    let checked = preflight(
        &state,
        &project,
        &screens,
        ScreenSettingsPatch {
            adb: true,
            time: false,
            sleep_off: false,
            ntp: None,
        },
        &commands,
    )
    .await;
    assert_eq!(checked.items[0].state, "skip");
    let adb = &checked.items[0].settings.as_ref().unwrap().items["adb"];
    assert_eq!(adb.current["firmwareBootConfigured"], true);
    assert!(!adb.reboot_required);
    commands.assert_read_only();
    support::close(state).await;
}

#[tokio::test]
async fn same_boot_ntp_evidence_uses_the_observed_mac_when_the_asset_mac_is_empty() {
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let cases = [Screen {
        source_event: false,
        ..Screen::satisfied(91, "10")
    }];
    let (project, screens) = assets(&state, &cases, true).await;
    let screen = &screens[0];
    assert!(screen.fields.mac.is_empty());
    let commands = ReadOnlyCommands::new(&cases);
    let before = preflight(&state, &project, &screens, ntp_patch(false), &commands).await;
    assert_eq!(before.items[0].state, "ready");
    assert!(before.items[0].settings.as_ref().unwrap().items["ntp"].reboot_required);
    let id = uuid::Uuid::now_v7().to_string();
    let plan = ScreenPlan {
        project_id: project.clone(),
        input: input(&screens),
        targets: screens.clone(),
        business_project_id: None,
        data_source_id: None,
        operator: "隔离授时证据".into(),
        instance_id: "read-fixture".into(),
        created_at: now(),
        detail: json!({}),
    };
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
            name: "隔离授时证据".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: state
                .paths
                .project_task_log_path(&project, &id)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            targets: vec![("smart_screen".into(), screen.id.clone())],
        })
        .await
        .unwrap();
    state
        .task_repository
        .transition(&id, TaskState::Draft, TaskState::Checking, None, None)
        .await
        .unwrap();
    state
        .task_repository
        .transition(&id, TaskState::Checking, TaskState::Failed, None, None)
        .await
        .unwrap();
    task_data::save_target(state.local_store.pool(),&project,&id,&ScreenTargetResult {screen_id:screen.id.clone(),device:ResultState::Succeeded,
        evidence:json!({"screenSettings":{"items":{"ntp":{"evidence":{"targetServer":cases[0].server,"afterBootId":cases[0].boot(),
            "activationConfirmed":true,"systemServerPid":cases[0].pid(),"observedMac":inxaiot_desk_buddy_lib::domain::smart_screen::rules::normalize_mac(&cases[0].mac())}}}}}),..Default::default()}).await.unwrap();
    let after = preflight(&state, &project, &screens, ntp_patch(false), &commands).await;
    assert_eq!(after.items[0].state, "skip");
    let ntp = &after.items[0].settings.as_ref().unwrap().items["ntp"];
    assert_eq!(ntp.current["syncConfirmed"], true);
    assert!(!ntp.reboot_required);
    assert!(
        task_data::read_preflight_plan(state.local_store.pool(), &project, &after.id)
            .await
            .is_err()
    );
    commands.assert_read_only();
    support::close(state).await;
}
