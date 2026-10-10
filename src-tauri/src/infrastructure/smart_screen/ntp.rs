use super::{
    device::{AdbDevice, AndroidTools, DeviceCommandPort},
    device_maintenance,
    leases::HeldScreenLeases,
    maintenance::MaintenancePlan,
    task_data,
    value_updates::reliable_identity,
};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::{
        model::{ResultState, ScreenAsset, ScreenObservation, ScreenTargetResult},
        ntp::{NtpConfig, NtpPatch, NtpRead},
        operation::{ScreenOperationInput, ScreenPlan, ScreenPreflight},
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

mod status;

fn stage_error(stage: &'static str, error: AppError) -> AppError {
    if matches!(error, AppError::Cancelled) {
        return error;
    }
    tracing::warn!(ntp_stage=stage,error=%crate::core::log_safety::safe_error(&error),"智能屏 NTP 阶段检查失败");
    let category = match error {
        AppError::Timeout { .. } => "等待设备响应超时",
        AppError::Io { .. } => "本机管理工具执行失败",
        AppError::NotFound(_) => "缺少所需设备管理工具",
        AppError::Conflict(_) => "设备连接或系统命令未完成",
        _ => "设备管理检查未完成",
    };
    AppError::Conflict(format!(
        "NTP {stage}失败：{category}；请检查设备连接和状态后重新操作"
    ))
}

pub(super) async fn check_status<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<()> {
    status::read(device, ip).await.map(|_| ())
}

pub async fn read_device<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
) -> AppResult<NtpConfig> {
    device
        .connect(ip, CancellationToken::new())
        .await
        .map_err(|error| stage_error("连接设备读取设置", error))?;
    async fn get<P: DeviceCommandPort>(
        device: &AdbDevice<P>,
        ip: &str,
        key: &str,
    ) -> AppResult<String> {
        device
            .shell(
                ip,
                &["settings", "get", "global", key],
                CancellationToken::new(),
            )
            .await
    }
    fn switch(value: &str) -> AppResult<bool> {
        match value.trim() {
            "1" => Ok(true),
            "0" | "null" | "" => Ok(false),
            _ => Err(AppError::Conflict(
                "系统自动时间设置返回无效内容，未修改配置".into(),
            )),
        }
    }
    let server = get(device, ip, "ntp_server")
        .await
        .map_err(|error| stage_error("读取原服务器地址", error))?;
    Ok(NtpConfig {
        server: if server.trim() == "null" {
            String::new()
        } else {
            server.trim().into()
        },
        auto_time: switch(
            &get(device, ip, "auto_time")
                .await
                .map_err(|error| stage_error("读取自动时间开关", error))?,
        )?,
        auto_time_zone: switch(
            &get(device, ip, "auto_time_zone")
                .await
                .map_err(|error| stage_error("读取自动时区开关", error))?,
        )?,
        time_zone: device
            .shell(
                ip,
                &["getprop", "persist.sys.timezone"],
                CancellationToken::new(),
            )
            .await
            .map_err(|error| stage_error("读取当前时区", error))?,
    })
}

pub async fn capability<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    observed: &ScreenObservation,
) -> AppResult<Value> {
    let firmware = observed.firmware.as_deref().unwrap_or("");
    let known = (screen.fields.size == "4"
        && observed.sdk == Some(27)
        && firmware.starts_with("rockchip/px30_evb/px30_evb:8.1.0/"))
        || (screen.fields.size == "10"
            && observed.sdk == Some(29)
            && firmware.starts_with("STATION/ceres_c4/ceres-c3:10/"));
    if !known {
        return Err(AppError::Conflict(
            "NTP 设置目前只支持已验证的 4 寸 Android 8.1 和 10 寸 Android 10 固件".into(),
        ));
    }
    let reboot = device_maintenance::capability(device, screen, observed, "reboot")
        .await
        .map_err(|error| stage_error("检查重启后管理连接恢复条件", error))?;
    let root = device_maintenance::root_mode(device, &screen.fields.ip)
        .await
        .map_err(|error| stage_error("检查系统管理权限", error))?;
    Ok(
        json!({"supported":true,"activation":"system_reboot","rebootRequired":true,"root":root,"bootId":reboot["bootId"]}),
    )
}

pub async fn read(
    state: &FormalAppState,
    project: &str,
    ids: &[String],
) -> AppResult<Vec<NtpRead>> {
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AppError::InvalidConfig("请选择不同的智能屏".into()));
    }
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let device = AdbDevice::new(AndroidTools::discover()?);
    let mut rows = Vec::new();
    for id in ids {
        let result = async {
            let screen = repo.asset(project, id).await?;
            let observed = device
                .inspect(&screen, "inspect", CancellationToken::new())
                .await?;
            if !reliable_identity(&screen, &observed) {
                return Err(AppError::Conflict(
                    "尚未确认所选屏身份，不能读取该屏设置".into(),
                ));
            }
            Ok::<_, AppError>((
                read_device(&device, &screen.fields.ip).await?,
                screen,
                observed,
            ))
        }
        .await;
        let (config, capabilities, message) = match result {
            Ok((config, screen, observed)) => match capability(&device, &screen, &observed).await {
                Ok(capability) => (
                    Some(config),
                    Some(capability),
                    "已读取当前屏的 NTP 设置".into(),
                ),
                Err(error) => (
                    Some(config),
                    Some(json!({"supported":false})),
                    error.to_string(),
                ),
            },
            Err(error) => (None, None, error.to_string()),
        };
        rows.push(NtpRead {
            screen_id: id.clone(),
            read_at: now(),
            config,
            capabilities,
            message,
        });
    }
    Ok(rows)
}

pub async fn preflight(
    state: &FormalAppState,
    project: &str,
    input: ScreenOperationInput,
    patches: BTreeMap<String, NtpPatch>,
) -> AppResult<ScreenPreflight> {
    super::maintenance::preflight_with_ntp(state, project, input, Some(patches)).await
}

async fn authorize(
    state: &FormalAppState,
    plan: &ScreenPlan,
    held: Option<&HeldScreenLeases>,
) -> AppResult<()> {
    if let Some(held) = held {
        let context = super::write_context::open(state, &plan.project_id).await?;
        context.validate_plan(plan)?;
        held.valid().await?;
    }
    Ok(())
}
async fn checkpoint(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    result: &ScreenTargetResult,
) -> AppResult<()> {
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await
}
async fn same_boot_verified(
    state: &FormalAppState,
    project: &str,
    except: &str,
    screen: &ScreenAsset,
    server: &str,
    boot: &str,
    mac: Option<&str>,
    system_server_pid: Option<u64>,
) -> AppResult<bool> {
    let Some(pid) = system_server_pid else {
        return Ok(false);
    };
    let count:i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data d JOIN local_task t ON t.id=d.local_task_id AND t.domain_type='smart_screen' AND t.operation_type='ntp', json_each(d.result_json,'$.targets') target WHERE d.local_project_id=? AND d.local_task_id<>? AND target.key=? AND json_extract(target.value,'$.evidence.ntp.targetServer')=? AND json_extract(target.value,'$.evidence.ntp.afterBootId')=? AND json_extract(target.value,'$.evidence.ntp.activation')='succeeded' AND json_extract(target.value,'$.observation.observedMac')=? AND json_extract(target.value,'$.evidence.ntp.syncEvidence.systemServerPid')=?")
        .bind(project).bind(except).bind(&screen.id).bind(server).bind(boot).bind(mac).bind(pid as i64)
        .fetch_one(state.local_store.pool()).await.map_err(|e|AppError::database("核对同次启动的 NTP 生效记录",&e))?;
    Ok(count > 0)
}

pub async fn execute(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
) -> AppResult<()> {
    let patch = data
        .ntp
        .as_ref()
        .and_then(|p| p.get(&screen.id))
        .ok_or_else(|| AppError::Conflict("缺少本次 NTP 设置内容".into()))?;
    patch.validate()?;
    let original = data
        .observations
        .get(&screen.id)
        .ok_or_else(|| AppError::Conflict("缺少原设备身份".into()))?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let current = repo.asset(&plan.project_id, &screen.id).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || super::value_updates::critical_draft(&repo.snapshot(&plan.project_id).await?, &current)
    {
        return Err(AppError::Conflict("屏资料已变化，未设置 NTP 服务器".into()));
    }
    if screen.source == "platform" {
        let context = super::write_context::open(state, &plan.project_id).await?;
        let current = super::platform_write::record(&context.read, &screen.id)
            .await?
            .ok_or_else(|| AppError::Conflict("平台屏已不存在".into()))?;
        if current.business.as_deref() != Some(&context.business)
            || !crate::application::smart_screen::operations::same_target(screen, &current.asset)
        {
            return Err(AppError::Conflict(
                "平台屏身份已变化，未设置 NTP 服务器".into(),
            ));
        }
    }
    let device = AdbDevice::new(AndroidTools::discover()?);
    let progress = super::installation_progress::ScreenOperationProgress {
        state,
        task: id,
        screen,
    };
    progress
        .report("读取 NTP 设置", 5, "正在重新确认设备身份和系统设置", true)
        .await?;
    let observed = device
        .inspect(screen, "inspect", cancel.clone())
        .await
        .map_err(|error| stage_error("重新确认设备身份", error))?;
    if !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(AppError::Conflict(
            "屏身份与检查时不一致，未设置 NTP 服务器".into(),
        ));
    }
    let cap = capability(&device, screen, &observed).await?;
    let before = read_device(&device, &screen.fields.ip).await?;
    let status = status::read(&device, &screen.fields.ip).await?;
    let direct_source =
        status::source_event(&device, &screen.fields.ip, patch.server(), &status).await;
    let source_verified = direct_source.is_some()
        || same_boot_verified(
            state,
            &plan.project_id,
            id,
            screen,
            patch.server(),
            cap["bootId"].as_str().unwrap_or(""),
            observed.observed_mac.as_deref(),
            status["systemServerPid"].as_u64(),
        )
        .await?;
    result.observation = Some(observed);
    result.evidence = json!({"phase":"ntp_checked","capability":cap,"ntp":{"before":before,"targetServer":patch.server(),"save":"pending","activation":"pending","sync":"pending","rebootRequired":false,"beforeStatus":status,"beforeSourceEvent":direct_source}});
    checkpoint(state, id, plan, result).await?;
    if !patch.activation_required(&before, source_verified) {
        result.evidence["ntp"]["save"] = json!("unchanged");
        result.evidence["ntp"]["after"] = json!(before);
        // 相同设置不重复写入或重启；运行中的时间源必须有独立证据。
        result.evidence["ntp"]["activation"] = json!("pending");
        return wait_sync(state, id, plan, data, screen, result).await;
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    authorize(state, plan, held).await?;
    let mode = cap["root"].as_str().unwrap_or("");
    let unchanged = patch.matches(&before);
    if !unchanged {
        progress
            .report(
                "保存 NTP 设置",
                25,
                "正在保存服务器地址并开启自动设置时间",
                true,
            )
            .await?;
        result.device = ResultState::Unknown;
        result.evidence["phase"] = json!("ntp_save_requested");
        result.evidence["ntp"]["save"] = json!("unknown");
        checkpoint(state, id, plan, result).await?;
        if before.server != patch.server() {
            authorize(state, plan, held).await?;
            set_server(&device, &screen.fields.ip, mode, patch).await?;
        }
        authorize(state, plan, held).await?;
        if !before.auto_time {
            device_maintenance::root_command(
                &device,
                &screen.fields.ip,
                mode,
                &["settings", "put", "global", "auto_time", "1"],
            )
            .await?;
        }
    }
    let after = read_device(&device, &screen.fields.ip).await?;
    result.evidence["ntp"]["after"] = json!(after);
    if !patch.matches(&after)
        || before.auto_time_zone != after.auto_time_zone
        || before.time_zone != after.time_zone
    {
        result.device = ResultState::Failed;
        result.evidence["ntp"]["save"] = json!("failed");
        result.evidence["ntp"]["activation"] = json!("not_started");
        result.evidence["ntp"]["sync"] = json!("not_started");
        result.message = "NTP 设置回读不符合本次目标或原时区设置已变化，未重启屏".into();
        return checkpoint(state, id, plan, result).await;
    }
    result.evidence["phase"] = json!("ntp_saved");
    result.evidence["ntp"]["save"] = json!(if unchanged { "unchanged" } else { "succeeded" });
    result.evidence["ntp"]["rebootRequired"] = json!(true);
    checkpoint(state, id, plan, result).await?;
    authorize(state, plan, held).await?;
    progress
        .report(
            "使 NTP 设置生效",
            45,
            "正在重启屏，使系统授时服务重新读取地址",
            true,
        )
        .await?;
    device_maintenance::root_command(&device, &screen.fields.ip, mode, &["sync"]).await?;
    result.evidence["phase"] = json!("ntp_reboot_requested");
    result.device = ResultState::Unknown;
    result.evidence["ntp"]["activation"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    // 结果保存可能跨越其他客户端接手，发送真正的重启命令前再次核对占用。
    authorize(state, plan, held).await?;
    let _ = device
        .adb(
            &screen.fields.ip,
            &["reboot"],
            Duration::from_secs(20),
            CancellationToken::new(),
        )
        .await;
    device_maintenance::wait_boot(&device, screen, &cap).await?;
    result.evidence["phase"] = json!("ntp_rebooted");
    checkpoint(state, id, plan, result).await?;
    progress
        .report(
            "验证自动授时",
            70,
            "正在核对系统的新 NTP 样本与当前时间",
            true,
        )
        .await?;
    wait_sync(state, id, plan, data, screen, result).await
}

async fn set_server<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    mode: &str,
    patch: &NtpPatch,
) -> AppResult<()> {
    patch.validate()?;
    if patch.server().is_empty() {
        device_maintenance::root_command(
            device,
            ip,
            mode,
            &["settings", "delete", "global", "ntp_server"],
        )
        .await?;
    } else {
        device_maintenance::root_command(
            device,
            ip,
            mode,
            &["settings", "put", "global", "ntp_server", patch.server()],
        )
        .await?;
    }
    Ok(())
}

async fn wait_sync(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        // 等待只轻量读取系统 NTP 状态，不为每次轮询生成完整设备观测历史。
        let status = status::read(&device, &screen.fields.ip).await?;
        let clock = device
            .shell(
                &screen.fields.ip,
                &["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"],
                CancellationToken::new(),
            )
            .await?;
        let close = time::OffsetDateTime::parse(
            clock.trim(),
            &time::format_description::well_known::Rfc3339,
        )
        .ok()
        .is_some_and(|clock| {
            (clock.unix_timestamp() - time::OffsetDateTime::now_utc().unix_timestamp()).abs() <= 15
        });
        if (status::fresh_sample(&status) && close) || tokio::time::Instant::now() >= deadline {
            return verify(state, id, plan, data, screen, result).await;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

/// 恢复只读取设备、配置和授时证据，不重复保存、校时或重启。
pub async fn verify(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let patch = data
        .ntp
        .as_ref()
        .and_then(|p| p.get(&screen.id))
        .ok_or_else(|| AppError::Conflict("原 NTP 设置目标缺失".into()))?;
    let original = data
        .observations
        .get(&screen.id)
        .ok_or_else(|| AppError::Conflict("原设备身份缺失".into()))?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    let mut observed = device
        .inspect(screen, "inspect", CancellationToken::new())
        .await?;
    if !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(AppError::Conflict(
            "尚未重新确认原设备身份，保留待核实记录".into(),
        ));
    }
    let after = read_device(&device, &screen.fields.ip).await?;
    let boot = device_maintenance::boot_id(&device, &screen.fields.ip).await?;
    let status = status::read(&device, &screen.fields.ip).await?;
    result.evidence["ntp"]["after"] = json!(after);
    result.evidence["ntp"]["afterBootId"] = json!(boot);
    observed.task_id = Some(id.into());
    observed.operation_type = "ntp".into();
    ScreenRepository::new(state.local_store.pool().clone())
        .append_observation(&plan.project_id, &observed)
        .await?;
    let offset = observed.clock_offset_seconds;
    result.observation = Some(observed);
    if !patch.matches(&after) {
        result.device = ResultState::Failed;
        result.evidence["ntp"]["save"] = json!("failed");
        result.evidence["ntp"]["activation"] = json!("failed");
        result.evidence["ntp"]["sync"] = json!("not_started");
        result.message = "当前 NTP 地址或自动时间设置不符合原目标，未重放旧设置".into();
        return checkpoint(state, id, plan, result).await;
    }
    let before: NtpConfig = serde_json::from_value(result.evidence["ntp"]["before"].clone())
        .map_err(|_| AppError::Conflict("原 NTP 设置快照缺失，未重复修改".into()))?;
    if before.auto_time_zone != after.auto_time_zone || before.time_zone != after.time_zone {
        result.device = ResultState::Failed;
        result.evidence["ntp"]["sync"] = json!("unknown");
        result.message =
            "NTP 地址已保存，但时区设置与原记录不一致，请独立核对；未重复修改或重启".into();
        return checkpoint(state, id, plan, result).await;
    }
    let rebooted = result.evidence["capability"]["bootId"]
        .as_str()
        .is_some_and(|id| id != boot)
        && device
            .shell(
                &screen.fields.ip,
                &["getprop", "sys.boot_completed"],
                CancellationToken::new(),
            )
            .await?
            == "1";
    if !rebooted && result.evidence["phase"] == "ntp_reboot_requested" {
        return Err(AppError::Conflict(
            "NTP 配置已保存，尚未确认本次系统重启完成；保留待核实记录，不再次重启".into(),
        ));
    }
    let unchanged = result.evidence["ntp"]["save"] == "unchanged";
    let previously_active = if !rebooted && unchanged {
        same_boot_verified(
            state,
            &plan.project_id,
            id,
            screen,
            patch.server(),
            &boot,
            original.observed_mac.as_deref(),
            status["systemServerPid"].as_u64(),
        )
        .await?
    } else {
        false
    };
    let persisted_before_activation = matches!(
        result.evidence["phase"].as_str(),
        Some("ntp_saved" | "ntp_reboot_requested" | "ntp_rebooted" | "ntp_verified")
    ) && matches!(
        result.evidence["ntp"]["save"].as_str(),
        Some("succeeded" | "unchanged")
    );
    let source_event =
        status::source_event(&device, &screen.fields.ip, patch.server(), &status).await;
    let active =
        source_event.is_some() || (rebooted && persisted_before_activation) || previously_active;
    result.evidence["ntp"]["save"] = json!(if unchanged { "unchanged" } else { "succeeded" });
    result.evidence["ntp"]["activation"] = json!(if active { "succeeded" } else { "unknown" });
    let synchronized =
        active && status::fresh_sample(&status) && offset.is_some_and(|offset| offset.abs() <= 15);
    result.evidence["ntp"]["syncEvidence"] = json!({"server":if synchronized && !patch.server().is_empty(){Some(patch.server())}else{None},"sourceType":if synchronized{if patch.server().is_empty(){"firmware_default"}else{"configured_server"}}else{"unconfirmed"},"clockOffsetSeconds":offset,"systemServerPid":status["systemServerPid"],"status":status,"sourceEvent":source_event,"activationConfirmedBy":if source_event.is_some(){"ntp_success_event"}else if active && rebooted{"new_boot_loaded_settings"}else if previously_active{"same_boot_verified_settings"}else{"unconfirmed"},"sourceConfirmedBy":if !synchronized{"unconfirmed"}else if source_event.is_some(){"ntp_success_event"}else if active && rebooted{"new_boot_loaded_settings"}else if previously_active{"same_boot_verified_settings"}else{"unconfirmed"}});
    result.evidence["ntp"]["sync"] = json!(if synchronized { "succeeded" } else { "unknown" });
    result.device = if synchronized {
        ResultState::Succeeded
    } else {
        ResultState::Failed
    };
    result.message = if synchronized {
        "NTP 设置已生效，已确认系统 NTP 样本及当前时间偏差不超过 15 秒"
    } else if active {
        "NTP 配置已生效，自动授时未确认；请检查服务器、UDP 123 及网络，未重复设置或重启"
    } else {
        "NTP 配置已保存，尚未确认运行中的系统授时服务已使用该设置；未重复设置或重启"
    }
    .into();
    if synchronized {
        result.evidence["phase"] = json!("ntp_verified");
    }
    checkpoint(state, id, plan, result).await
}

#[cfg(test)]
mod tests {
    use super::super::device::ProcessOutput;
    use super::*;
    use std::{
        path::Path,
        sync::{Arc, Mutex},
    };

    #[derive(Clone, Default)]
    struct Commands {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        server: Arc<Mutex<String>>,
    }
    impl DeviceCommandPort for Commands {
        async fn execute(
            &self,
            _program: &Path,
            args: Vec<String>,
            _limit: Duration,
            _cancel: CancellationToken,
        ) -> AppResult<ProcessOutput> {
            self.calls.lock().unwrap().push(args.clone());
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let stdout = if let Some(index) = args.iter().position(|arg| *arg == "shell") {
                match &args[index + 1..] {
                    ["settings", "get", "global", "ntp_server"] => {
                        self.server.lock().unwrap().clone()
                    }
                    ["settings", "get", "global", "auto_time"] => "1".into(),
                    ["settings", "get", "global", "auto_time_zone"] => "0".into(),
                    ["getprop", "persist.sys.timezone"] => "Asia/Shanghai".into(),
                    ["settings", "delete", "global", "ntp_server"] => {
                        *self.server.lock().unwrap() = "null".into();
                        "Deleted 1 rows".into()
                    }
                    ["settings", "put", "global", "ntp_server", value] => {
                        *self.server.lock().unwrap() = value.to_string();
                        String::new()
                    }
                    _ => return Err(AppError::Conflict("非预期设备命令".into())),
                }
            } else if args.contains(&"get-state") {
                "device".into()
            } else if args.first() == Some(&"connect") {
                "connected".into()
            } else {
                return Err(AppError::Conflict("非预期 ADB 命令".into()));
            };
            Ok(ProcessOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }
    fn device(commands: Commands) -> AdbDevice<Commands> {
        AdbDevice::with_commands(
            AndroidTools {
                adb: "adb.exe".into(),
            },
            commands,
        )
    }
    #[test]
    fn stage_error_keeps_fixed_phase_but_never_copies_device_output_and_preserves_cancel() {
        let error = stage_error(
            "读取系统授时服务状态",
            AppError::Conflict("token=private-test-token stderr from device".into()),
        )
        .to_string();
        assert!(error.contains("读取系统授时服务状态"));
        assert!(error.contains("设备连接或系统命令未完成"));
        assert!(!error.contains("private-test-token"));
        assert!(!error.contains("stderr"));
        assert!(matches!(
            stage_error("读取原服务器地址", AppError::Cancelled),
            AppError::Cancelled
        ));
    }
    #[tokio::test]
    async fn unset_server_is_empty_and_reading_never_writes_or_restarts() {
        let commands = Commands::default();
        *commands.server.lock().unwrap() = "null".into();
        let read = read_device(&device(commands.clone()), "192.0.2.4")
            .await
            .unwrap();
        assert!(read.server.is_empty());
        assert!(read.auto_time);
        assert!(!read.auto_time_zone);
        assert_eq!(read.time_zone, "Asia/Shanghai");
        assert!(commands.calls.lock().unwrap().iter().all(|args| {
            !args
                .iter()
                .any(|arg| ["put", "delete", "reboot"].contains(&arg.as_str()))
        }));
    }
    #[tokio::test]
    async fn empty_patch_deletes_custom_key_instead_of_writing_empty_and_never_changes_timezone() {
        let commands = Commands::default();
        *commands.server.lock().unwrap() = "old.internal".into();
        let device = device(commands.clone());
        set_server(
            &device,
            "192.0.2.4",
            "shell",
            &NtpPatch { server: "".into() },
        )
        .await
        .unwrap();
        assert_eq!(*commands.server.lock().unwrap(), "null");
        let calls = commands.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            &calls[0][3..],
            ["settings", "delete", "global", "ntp_server"]
        );
        assert!(
            !calls[0]
                .iter()
                .any(|arg| ["auto_time_zone", "persist.sys.timezone"].contains(&arg.as_str()))
        );
    }
    #[tokio::test]
    async fn invalid_address_is_rejected_before_device_command() {
        let commands = Commands::default();
        assert!(
            set_server(
                &device(commands.clone()),
                "192.0.2.4",
                "shell",
                &NtpPatch {
                    server: "ntp;reboot".into()
                }
            )
            .await
            .is_err()
        );
        assert!(commands.calls.lock().unwrap().is_empty());
    }
}
