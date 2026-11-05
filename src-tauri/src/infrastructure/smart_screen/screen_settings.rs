//! 一个批量任务内逐屏串行、跨屏并发，每屏持续持有占用；ADB与NTP共享一次重启。
use super::{
    device::{AdbDevice, AndroidTools, DeviceCommandPort},
    device_maintenance as maintenance_device,
    device_maintenance::clock,
    leases::HeldScreenLeases,
    maintenance::MaintenancePlan,
    ntp, sleep_policy, task_data,
    value_updates::reliable_identity,
};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::{
        model::*,
        ntp::{NtpConfig, NtpPatch},
        operation::*,
        settings::*,
    },
    formal::app_state::FormalAppState,
    infrastructure::local_sqlite::screen_repository::{ScreenRepository, now},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

mod execute;
pub use execute::{execute, execute_with_device, verify, verify_with_device};
const CODES: [&str; 4] = ["adb", "ntp", "time", "sleepOff"];

fn error(text: &str) -> AppError {
    AppError::Conflict(text.into())
}
fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}
async fn shell<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    args: &[&str],
    cancel: &CancellationToken,
) -> AppResult<String> {
    check_cancel(cancel)?;
    let value = device.shell(ip, args, cancel.clone()).await?;
    check_cancel(cancel)?;
    Ok(value)
}
fn item(
    state: &str,
    current: Value,
    target: Value,
    current_display: String,
    target_display: &str,
    reason: &str,
    reboot: bool,
) -> ScreenSettingItem {
    ScreenSettingItem {
        state: state.into(),
        current,
        target,
        current_display,
        target_display: target_display.into(),
        reason: reason.into(),
        reboot_required: reboot,
    }
}
fn problem(selected: bool, target: &str, cause: AppError) -> ScreenSettingItem {
    item(
        if selected { "blocked" } else { "not_selected" },
        Value::Null,
        Value::Null,
        "未确认".into(),
        target,
        &cause.to_string(),
        false,
    )
}

async fn adb_state<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
) -> AppResult<Value> {
    let ip = &screen.fields.ip;
    let cancel = CancellationToken::new();
    let persistent = shell(device, ip, &["getprop", "persist.adb.tcp.port"], &cancel).await?;
    let service = shell(device, ip, &["getprop", "service.adb.tcp.port"], &cancel).await?;
    for port in [&persistent, &service] {
        if !port.is_empty()
            && port
                .parse::<i32>()
                .ok()
                .filter(|v| (-1..=65535).contains(v))
                .is_none()
        {
            return Err(error("ADB端口状态格式无效"));
        }
    }
    let known_four = screen.fields.size == "4"
        && observed.sdk == Some(27)
        && observed.device_model.as_deref() == Some("px30_evb")
        && observed
            .firmware
            .as_deref()
            .is_some_and(|v| v.starts_with("rockchip/px30_evb/px30_evb:8.1.0/"));
    let write_supported = screen.fields.size == "10"
        && observed.sdk == Some(29)
        && observed
            .firmware
            .as_deref()
            .is_some_and(|v| v.starts_with("STATION/ceres_c4/ceres-c3:10/"));
    let firmware_boot = if known_four && persistent != "5555" {
        shell(device, ip, &["cat", "/vendor/build.prop"], &cancel)
            .await?
            .lines()
            .any(|v| v.trim() == "service.adb.tcp.port=5555")
    } else {
        false
    };
    Ok(
        json!({"persistentPort":persistent,"servicePort":service,"firmwareBootConfigured":firmware_boot,"writeSupported":write_supported,
        "satisfied":(known_four||write_supported)&&service=="5555" && (persistent=="5555" || firmware_boot),"bootId":maintenance_device::boot_id(device,ip).await?}),
    )
}

async fn same_boot_ntp(
    state: &FormalAppState,
    project: &str,
    screen: &ScreenAsset,
    server: &str,
    boot: &str,
    pid: Option<u64>,
    observed_mac: Option<&str>,
) -> AppResult<bool> {
    let Some(pid) = pid else { return Ok(false) };
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data d JOIN local_task t ON t.id=d.local_task_id AND t.operation_type='screen_settings',json_each(d.result_json,'$.targets') target WHERE d.local_project_id=? AND target.key=? AND json_extract(target.value,'$.evidence.screenSettings.items.ntp.evidence.targetServer')=? AND json_extract(target.value,'$.evidence.screenSettings.items.ntp.evidence.afterBootId')=? AND json_extract(target.value,'$.evidence.screenSettings.items.ntp.evidence.activationConfirmed')=1 AND json_extract(target.value,'$.evidence.screenSettings.items.ntp.evidence.systemServerPid')=? AND json_extract(target.value,'$.evidence.screenSettings.items.ntp.evidence.observedMac')=?")
        .bind(project).bind(&screen.id).bind(server).bind(boot).bind(pid as i64).bind(observed_mac.map(crate::domain::smart_screen::rules::normalize_mac))
        .fetch_one(state.local_store.pool()).await.map_err(|e|AppError::database("读取同次启动的组合授时证据",&e))?;
    Ok(count > 0)
}

pub async fn inspect<P: DeviceCommandPort>(
    state: &FormalAppState,
    project: &str,
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
    patch: &ScreenSettingsPatch,
) -> AppResult<ScreenSettingsRead> {
    patch.validate()?;
    if !reliable_identity(screen, observed) {
        return Err(error("未确认同一智能屏身份，不能生成设置计划"));
    }
    let mut items = BTreeMap::new();
    let adb = async {
        let mut current = adb_state(device, screen, observed).await?;
        let satisfied = current["satisfied"] == true;
        if patch.adb && !satisfied {
            if current["writeSupported"] != true {
                return Err(error("此固件未确认已保持ADB，也不支持已验证的端口写入"));
            }
            current["root"] =
                json!(maintenance_device::root_mode(device, &screen.fields.ip).await?);
        }
        Ok::<_, AppError>(item(
            if !patch.adb {
                "not_selected"
            } else if satisfied {
                "skip"
            } else {
                "ready"
            },
            current.clone(),
            json!({"port":"5555"}),
            format!(
                "持久端口{}，运行端口{}{}",
                current["persistentPort"].as_str().unwrap_or(""),
                current["servicePort"].as_str().unwrap_or(""),
                if current["firmwareBootConfigured"] == true {
                    "；厂家开机配置5555"
                } else {
                    ""
                }
            ),
            "重启后保持ADB 5555",
            if satisfied {
                "当前已保持，无需重写或单独重启"
            } else {
                "先保存并回读ADB，与NTP按需共用一次重启"
            },
            patch.adb && !satisfied,
        ))
    }
    .await;
    items.insert(
        "adb".into(),
        adb.unwrap_or_else(|cause| problem(patch.adb, "重启后保持ADB 5555", cause)),
    );
    let ntp_item = async {
        let before = ntp::read_device(device, &screen.fields.ip).await?;
        let boot = maintenance_device::boot_id(device, &screen.fields.ip).await?;
        let status = ntp::read_status(device, &screen.fields.ip).await?;
        let selected = patch.ntp.as_ref();
        let target = selected
            .map(NtpPatch::server)
            .unwrap_or(before.server.as_str());
        let direct = ntp::source_event(device, &screen.fields.ip, target, &status).await;
        let active = direct.is_some()
            || same_boot_ntp(
                state,
                project,
                screen,
                target,
                &boot,
                status["systemServerPid"].as_u64(),
                observed.observed_mac.as_deref(),
            )
            .await?;
        let sync_confirmed = active
            && ntp::fresh_sample(&status)
            && observed
                .clock_offset_seconds
                .is_some_and(|v| v.unsigned_abs() <= 15);
        let satisfied = selected.is_some_and(|v| v.matches(&before)) && sync_confirmed;
        let reboot = selected.is_some_and(|v| !v.matches(&before) || !active);
        let mut current = serde_json::to_value(&before).map_err(|_| error("无法生成授时快照"))?;
        current["activationConfirmed"] = json!(active);
        current["syncConfirmed"] = json!(sync_confirmed);
        current["status"] = status;
        current["bootId"] = json!(boot);
        current["sourceEvent"] = direct.unwrap_or(Value::Null);
        if selected.is_some() && !satisfied && reboot {
            let repair = patch.adb && items.get("adb").is_some_and(|v| v.state == "ready");
            current["capability"] =
                ntp::configuration_capability(device, screen, observed, repair).await?;
        }
        Ok::<_, AppError>(item(
            if selected.is_none() {
                "not_selected"
            } else if satisfied {
                "skip"
            } else {
                "ready"
            },
            current,
            json!({"server":target}),
            format!(
                "{}；自动时间{}；运行源{}",
                if before.server.is_empty() {
                    "固件默认"
                } else {
                    before.server.as_str()
                },
                if before.auto_time { "开启" } else { "关闭" },
                if active { "已确认" } else { "未确认" }
            ),
            if selected.is_none() {
                "不修改NTP"
            } else if target.is_empty() {
                "显式恢复固件默认NTP"
            } else {
                target
            },
            if selected.is_none() {
                "未选择NTP，保留原地址和开关"
            } else if satisfied {
                "设置与授时均已满足"
            } else {
                "保存值与系统实际授时分开核实"
            },
            reboot,
        ))
    }
    .await;
    items.insert(
        "ntp".into(),
        ntp_item.unwrap_or_else(|cause| problem(patch.ntp.is_some(), "指定NTP服务器", cause)),
    );
    let time = async {
        let mode = maintenance_device::root_mode(device, &screen.fields.ip).await?;
        let mut current = clock::inspect(device, screen, observed, &mode).await?;
        current["root"] = json!(mode);
        let satisfied = current["satisfied"] == true;
        if patch.time && !satisfied {
            maintenance_device::require_date_setting(device, &screen.fields.ip).await?;
        }
        Ok::<_, AppError>(item(
            if !patch.time {
                "not_selected"
            } else if satisfied {
                "skip"
            } else {
                "ready"
            },
            current.clone(),
            json!({"maximumOffsetSeconds":15}),
            format!(
                "系统偏差{}秒，硬件时钟偏差{}秒",
                current["systemOffsetSeconds"], current["hardwareOffsetSeconds"]
            ),
            "系统与启动RTC均在15秒内",
            if satisfied {
                "两处时间均已满足，无需校准"
            } else {
                "重启及NTP核实后重新读取，需要时复用双时钟校准"
            },
            false,
        ))
    }
    .await;
    items.insert(
        "time".into(),
        time.unwrap_or_else(|cause| problem(patch.time, "系统与硬件时钟校准", cause)),
    );
    let sleep = async {
        let status =
            sleep_policy::read(device, &screen.fields.ip, CancellationToken::new()).await?;
        if patch.sleep_off {
            status.check_conditions()?;
            if !status.already_disabled() {
                sleep_policy::check_write_access(
                    device,
                    &screen.fields.ip,
                    CancellationToken::new(),
                )
                .await?;
            }
        }
        Ok::<_, AppError>(item(
            if !patch.sleep_off {
                "not_selected"
            } else if status.already_disabled() {
                "skip"
            } else {
                "ready"
            },
            serde_json::to_value(&status).map_err(|_| error("无法生成休眠状态"))?,
            json!({"mask":status.target_mask()}),
            if status.already_disabled() {
                "已关闭自动休眠（接电保持亮屏）"
            } else {
                "尚未关闭自动休眠"
            }
            .into(),
            "接电保持亮屏",
            &status.preflight_message(),
            false,
        ))
    }
    .await;
    items.insert(
        "sleepOff".into(),
        sleep.unwrap_or_else(|cause| problem(patch.sleep_off, "接电保持亮屏", cause)),
    );
    let pending_count = items.values().filter(|v| v.state == "ready").count();
    let blocked_count = items
        .values()
        .filter(|v| v.state == "blocked" || v.state == "unknown")
        .count();
    Ok(ScreenSettingsRead {
        screen_id: screen.id.clone(),
        read_at: now(),
        items,
        pending_count,
        blocked_count,
        error: None,
    })
}

pub async fn read(
    state: &FormalAppState,
    project: &str,
    ids: &[String],
) -> AppResult<Vec<ScreenSettingsRead>> {
    read_with_device(
        state,
        project,
        ids,
        &AdbDevice::new(AndroidTools::discover()?),
    )
    .await
}
#[doc(hidden)]
pub async fn read_with_device<P: DeviceCommandPort>(
    state: &FormalAppState,
    project: &str,
    ids: &[String],
    device: &AdbDevice<P>,
) -> AppResult<Vec<ScreenSettingsRead>> {
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(error("请选择1至1000台不同智能屏"));
    }
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let snapshot = repo.snapshot(project).await?;
    let targets = ids
        .iter()
        .map(|id| {
            snapshot
                .screens
                .iter()
                .find(|v| &v.id == id)
                .cloned()
                .ok_or_else(|| error("所选屏已变化"))
        })
        .collect::<AppResult<Vec<_>>>()?;
    let mut rows = Vec::new();
    for screen in targets {
        let checked = async {
            let observed = device
                .inspect(&screen, "inspect", CancellationToken::new())
                .await?;
            inspect(
                state,
                project,
                device,
                &screen,
                &observed,
                &ScreenSettingsPatch::default(),
            )
            .await
        }
        .await;
        rows.push(checked.unwrap_or_else(|cause| {
            ScreenSettingsRead {
                screen_id: screen.id,
                read_at: now(),
                pending_count: 0,
                blocked_count: 3,
                error: Some(cause.to_string()),
                items: CODES
                    .iter()
                    .map(|code| {
                        (
                            (*code).into(),
                            problem(*code != "ntp", "未确认", error("未取得可靠的设备检查结果")),
                        )
                    })
                    .collect(),
            }
        }));
    }
    Ok(rows)
}

pub async fn preflight(
    state: &FormalAppState,
    project: &str,
    input: ScreenOperationInput,
    patches: BTreeMap<String, ScreenSettingsPatch>,
) -> AppResult<ScreenPreflight> {
    super::maintenance::preflight_settings_with_device_factory(
        state,
        project,
        input,
        None,
        None,
        Some(patches),
        || Ok(AdbDevice::new(AndroidTools::discover()?)),
    )
    .await
}

pub(super) async fn validate_retry_patches(
    state: &FormalAppState,
    project: &str,
    input: &ScreenOperationInput,
    patches: &BTreeMap<String, ScreenSettingsPatch>,
) -> AppResult<()> {
    let Some(task) = &input.retry_of_operation_id else {
        return Ok(());
    };
    let (previous, _) = task_data::read_plan(state.local_store.pool(), project, task).await?;
    let results = task_data::read_results(state.local_store.pool(), project, task).await?;
    let original: Option<BTreeMap<String, ScreenSettingsPatch>> =
        if previous.input.action == "screen_settings" {
            Some(
                serde_json::from_value(previous.detail["settings"].clone())
                    .map_err(|_| error("原设置选项缺失，不能猜测重试项目"))?,
            )
        } else {
            None
        };
    for (id, patch) in patches {
        let old = previous
            .targets
            .iter()
            .find(|v| &v.id == id || v.aliases.contains(id))
            .ok_or_else(|| error("重试不能增加原任务之外的屏"))?;
        for code in CODES {
            if !patch.selected(code) {
                continue;
            }
            if let Some(original) = &original {
                if !original.get(&old.id).is_some_and(|v| v.selected(code)) {
                    return Err(error("重试不能默认增加原任务未选择的设置，请另建设置任务"));
                }
                if let Some(result) = results.targets.get(&old.id) {
                    match result.evidence["screenSettings"]["items"][code]["state"].as_str() {
                        Some("succeeded" | "skipped") => {
                            return Err(error("原设置子项已经完成，不加入失败重试"));
                        }
                        Some("unknown" | "pending") => {
                            return Err(error("原设置结果尚未确认，请先只读核实原任务"));
                        }
                        _ => {}
                    }
                }
            } else {
                let allowed = match previous.input.action.as_str() {
                    "time" => "time",
                    "adb" => "adb",
                    "sleep_off" => "sleepOff",
                    _ => return Err(error("原独立操作不支持转换为设置重试")),
                };
                if code != allowed {
                    return Err(error("独立操作重试只能保留其原设置项，不带入默认三项"));
                }
            }
        }
    }
    Ok(())
}
