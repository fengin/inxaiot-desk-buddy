use super::device::{AdbDevice, AndroidTools, DeviceCommandPort};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::app_config::{AppConfigPatch, AppConfigRead, AppHomeRead};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::local_sqlite::screen_repository::{ScreenRepository, now};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;

const URI: &str = "content://chat.xiaoxin.app.workbench.config";

/// 配置草稿与资产草稿分开，复用本机任务 JSON 表；草稿不创建操作锁。
pub async fn load_draft(state: &FormalAppState, project: &str) -> AppResult<Option<Value>> {
    use sha2::{Digest, Sha256};
    let id = format!("app-config-draft:{project}");
    let row:Option<(String,String)>=sqlx::query_as("SELECT plan_json,plan_sha256 FROM local_screen_task_data WHERE local_task_id=? AND local_project_id=?")
        .bind(id).bind(project).fetch_optional(state.local_store.pool()).await.map_err(|e|AppError::database("读取小新配置草稿",&e))?;
    row.map(|(text, hash)| {
        if format!("{:x}", Sha256::digest(text.as_bytes())) != hash {
            return Err(AppError::Conflict(
                "本机配置草稿已损坏，请重新读取屏端配置".into(),
            ));
        }
        serde_json::from_str(&text).map_err(|_| AppError::Conflict("本机配置草稿格式无效".into()))
    })
    .transpose()
}
pub async fn save_draft(
    state: &FormalAppState,
    project: &str,
    draft: Option<Value>,
) -> AppResult<()> {
    use sha2::{Digest, Sha256};
    let id = format!("app-config-draft:{project}");
    let Some(mut draft) = draft else {
        sqlx::query(
            "DELETE FROM local_screen_task_data WHERE local_task_id=? AND local_project_id=?",
        )
        .bind(id)
        .bind(project)
        .execute(state.local_store.pool())
        .await
        .map_err(|e| AppError::database("清理已提交配置草稿", &e))?;
        return Ok(());
    };
    let object = draft
        .as_object()
        .ok_or_else(|| AppError::InvalidConfig("配置草稿格式无效".into()))?;
    if object.keys().any(|key| {
        ![
            "environment",
            "switchEnvironment",
            "edits",
            "readyMode",
            "homeMode",
            "names",
            "targetIds",
            "includedIds",
            "rows",
            "savedAt",
        ]
        .contains(&key.as_str())
    }) || !draft["targetIds"].is_array()
        || !draft["rows"].is_array()
    {
        return Err(AppError::InvalidConfig("配置草稿字段无效".into()));
    }
    if draft["targetIds"].as_array().unwrap().len() > 1000
        || draft["rows"].as_array().unwrap().len() > 1000
    {
        return Err(AppError::InvalidConfig("配置草稿目标过多".into()));
    }
    if draft
        .get("homeMode")
        .is_some_and(|mode| !matches!(mode.as_str(), Some("keep" | "on" | "off")))
    {
        return Err(AppError::InvalidConfig(
            "主屏设置草稿无效，请重新选择".into(),
        ));
    }
    draft["savedAt"] = json!(now());
    let text = serde_json::to_string(&draft)
        .map_err(|_| AppError::InvalidConfig("配置草稿无法保存".into()))?;
    if text.len() > 4 * 1024 * 1024 {
        return Err(AppError::InvalidConfig(
            "配置草稿内容过长，请减少本次目标".into(),
        ));
    }
    let hash = format!("{:x}", Sha256::digest(text.as_bytes()));
    sqlx::query("INSERT INTO local_screen_task_data(local_task_id,local_project_id,plan_json,plan_sha256,result_json,updated_at) VALUES(?,?,?,?,'{}',?) ON CONFLICT(local_task_id) DO UPDATE SET plan_json=excluded.plan_json,plan_sha256=excluded.plan_sha256,updated_at=excluded.updated_at WHERE local_screen_task_data.local_project_id=excluded.local_project_id")
        .bind(id).bind(project).bind(text).bind(hash).bind(now()).execute(state.local_store.pool()).await.map_err(|e|AppError::database("保存本机小新配置草稿",&e))?;
    Ok(())
}

pub async fn call<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    method: &str,
    request: Option<&Value>,
) -> AppResult<Value> {
    if !["getCapabilities", "readConfig", "saveConfig"].contains(&method) {
        return Err(AppError::InvalidConfig("不支持的小新配置调用".into()));
    }
    let mut args = vec!["shell", "content", "call", "--uri", URI, "--method", method];
    let payload;
    if let Some(request) = request {
        let raw = serde_json::to_vec(request)
            .map_err(|_| AppError::InvalidConfig("配置内容无法编码".into()))?;
        if raw.len() > 16 * 1024 {
            return Err(AppError::InvalidConfig("本次配置内容过长".into()));
        }
        payload = format!("payload:s:{}", STANDARD.encode(raw));
        args.extend(["--extra", &payload]);
    }
    let output = device
        .adb(ip, &args, Duration::from_secs(20), CancellationToken::new())
        .await?;
    if output
        .stdout
        .contains("Could not find provider: chat.xiaoxin.app.workbench.config")
        || output
            .stderr
            .contains("Could not find provider: chat.xiaoxin.app.workbench.config")
    {
        return Err(AppError::Conflict(
            "当前小新未提供远程配置功能，请先升级到支持配置管理的版本".into(),
        ));
    }
    if !output.success {
        return Err(AppError::Conflict(
            "小新配置调用未完成，请检查 ADB 连接和应用版本；保存结果需重新读取确认".into(),
        ));
    }
    parse_response(&output.stdout)
}
pub fn parse_response(text: &str) -> AppResult<Value> {
    let malformed =
        || AppError::Conflict("未收到有效的小新配置结果，请确认应用支持远程配置并已启动".into());
    if text.len() > 90 * 1024 {
        return Err(malformed());
    }
    let encoded = text
        .strip_prefix("Result: Bundle[{result=")
        .and_then(|s| s.strip_suffix("}]"))
        .ok_or_else(malformed)?;
    let bytes = STANDARD.decode(encoded).map_err(|_| malformed())?;
    if bytes.len() > 64 * 1024 {
        return Err(malformed());
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
    if !matches!(
        value["status"].as_str(),
        Some("ok" | "saved" | "partial" | "failed" | "unchanged")
    ) {
        return Err(malformed());
    }
    Ok(value)
}
fn accepted(response: Value) -> AppResult<Value> {
    if response["status"] == "ok" {
        return Ok(response["data"].clone());
    }
    let message = match response["code"].as_str() {
        Some("NOT_READY") => "小新尚未就绪，请启动应用或稍后重新读取",
        Some("FORBIDDEN") => "设备拒绝当前管理身份，不能读取或修改配置",
        Some("UNSUPPORTED") => "当前小新不支持此配置，请先升级应用",
        _ => "未能读取小新配置，请检查连接和应用状态",
    };
    Err(AppError::Conflict(message.into()))
}
pub async fn read_device<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    patch: Option<&AppConfigPatch>,
) -> AppResult<(Value, Value)> {
    device.connect(ip, CancellationToken::new()).await?;
    let cap = accepted(call(device, ip, "getCapabilities", None).await?)?;
    if cap["protocolVersion"] != 1 || cap["packageName"] != "chat.xiaoxin.app" {
        return Err(AppError::Conflict(
            "当前小新不支持此配置，请先升级应用".into(),
        ));
    }
    if cap["ready"] != true {
        return Err(AppError::Conflict(
            "小新尚未就绪，请启动应用后重新读取".into(),
        ));
    }
    if let Some(patch) = patch {
        let supported = cap["supportedFields"]
            .as_array()
            .ok_or_else(|| AppError::Conflict("小新配置能力返回不完整".into()))?;
        if patch
            .fields()
            .iter()
            .any(|f| !supported.contains(&json!(f)))
        {
            return Err(AppError::Conflict(
                "当前小新不支持所选字段，请先升级应用".into(),
            ));
        }
    }
    let config = accepted(call(device, ip, "readConfig", None).await?)?;
    // 不保存设备返回的任意私有字段。
    if config.get("customDeviceName").is_none()
        || !["test", "pre", "prod"]
            .contains(&config["environments"]["current"].as_str().unwrap_or(""))
    {
        return Err(AppError::Conflict("小新配置返回不完整".into()));
    }
    let mut safe = json!({"customDeviceName":config["customDeviceName"],"environments":{"current":config["environments"]["current"]}});
    for env in ["test", "pre", "prod"] {
        let source = &config["environments"][env];
        if !source.is_object() {
            return Err(AppError::Conflict("小新环境配置返回不完整".into()));
        }
        let mut fields = serde_json::Map::new();
        for key in [
            "otaUrl",
            "wsUrl",
            "h5Url",
            "h5ReadyCheckEnabled",
            "otaWsUrl",
            "otaH5Url",
            "effectiveWsUrl",
            "effectiveH5Url",
            "wsSource",
            "h5Source",
        ] {
            fields.insert(
                key.into(),
                source
                    .get(key)
                    .cloned()
                    .ok_or_else(|| AppError::Conflict("小新环境配置字段不完整".into()))?,
            );
        }
        safe["environments"][env] = Value::Object(fields);
    }
    Ok((cap, safe))
}
pub async fn read(
    state: &FormalAppState,
    project: &str,
    ids: &[String],
) -> AppResult<Vec<AppConfigRead>> {
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AppError::InvalidConfig("请选择不同的智能屏".into()));
    }
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let device = AdbDevice::new(AndroidTools::discover()?);
    let mut rows = Vec::new();
    for id in ids {
        let screen = repo.asset(project, id).await?;
        let result = read_device(&device, &screen.fields.ip, None).await;
        let (config, capabilities, message) = match result {
            Ok((cap, data)) => (Some(data), Some(cap), "已从当前屏读取".into()),
            Err(error) => (None, None, error.to_string()),
        };
        // 主屏能力独立于普通配置；旧应用或无系统策略读取权限不阻断已有配置管理。
        let home = if config.is_some() {
            Some(read_home(&device, &screen.fields.ip).await)
        } else {
            None
        };
        rows.push(AppConfigRead {
            screen_id: id.clone(),
            read_at: now(),
            config,
            capabilities,
            home,
            message,
        });
    }
    Ok(rows)
}

pub async fn read_home<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppHomeRead {
    match super::home::query(device, ip).await {
        Ok(status) => {
            let runtime = super::home::query_runtime(device, ip, CancellationToken::new()).await;
            let active = runtime.as_ref().ok().map(|runtime| runtime.verified());
            let message = match runtime {
                Err(error) => format!("已读取持久主屏规则，运行状态尚待核实：{error}"),
                Ok(_) if status.persistent_preferred && !status.verified() => {
                    "持久主屏规则已设置，但当前主屏解析尚未一致".into()
                }
                Ok(_) if status.persistent_preferred && active != Some(true) => {
                    "持久主屏规则已设置，小新尚未作为系统主屏在前台运行，可重新激活".into()
                }
                Ok(_) => "已读取持久主屏规则与系统主屏运行状态".into(),
            };
            AppHomeRead {
                supported: true,
                enabled: Some(status.persistent_preferred),
                active,
                message,
            }
        }
        Err(error) => AppHomeRead {
            supported: false,
            enabled: None,
            active: None,
            message: error.to_string(),
        },
    }
}

use super::{
    device_maintenance, leases::HeldScreenLeases, maintenance::MaintenancePlan, task_data,
    value_updates::reliable_identity,
};
use crate::domain::smart_screen::{
    model::{ResultState, ScreenAsset, ScreenTargetResult},
    operation::ScreenPlan,
};

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

async fn apply_home(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    original: &crate::domain::smart_screen::model::ScreenObservation,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    device: &AdbDevice,
    enabled: Option<bool>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    let Some(enabled) = enabled else {
        return Ok(());
    };
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    authorize(state, plan, held).await?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let current = repo.asset(&plan.project_id, &screen.id).await?;
    let observed = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || super::value_updates::critical_draft(&repo.snapshot(&plan.project_id).await?, &current)
        || !reliable_identity(screen, &observed)
        || observed.observed_mac != original.observed_mac
    {
        return Err(AppError::Conflict(
            "屏身份已变化，未继续修改主屏策略；已有配置保存结果保留".into(),
        ));
    }
    let status = super::home::query(device, &screen.fields.ip).await?;
    if status.matches_target(enabled) {
        result.evidence["home"]["save"] = json!("unchanged");
        result.evidence["home"]["readback"] = json!("succeeded");
        result.evidence["home"]["after"] = json!(status);
        return checkpoint(state, id, plan, result).await;
    }
    authorize(state, plan, held).await?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    super::installation_progress::ScreenOperationProgress {
        state,
        task: id,
        screen,
    }
    .report(
        "设置主屏应用",
        45,
        if enabled {
            "正在固定小新为主屏应用"
        } else {
            "正在解除小新的持久主屏关联"
        },
        true,
    )
    .await?;
    result.device = ResultState::Unknown;
    result.evidence["phase"] = json!("config_home_requested");
    result.evidence["home"]["save"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    match super::home::set_enabled(device, &screen.fields.ip, enabled).await? {
        super::home::HomeWrite::Applied => {
            result.evidence["home"]["save"] = json!("applied");
            result.evidence["phase"] = json!("config_home_applied");
        }
        super::home::HomeWrite::Rejected(message) => {
            result.device = ResultState::Failed;
            result.evidence["home"]["save"] = json!("rejected");
            result.evidence["home"]["readback"] = json!("not_required");
            return Err(AppError::Conflict(format!(
                "主屏策略未修改：{message}；普通配置的已有保存结果保留"
            )));
        }
    }
    checkpoint(state, id, plan, result).await
}

fn home_request_satisfied(
    enabled: bool,
    policy: Option<&super::home::HomeStatus>,
    runtime: Option<&super::home::HomeRuntimeStatus>,
) -> bool {
    policy.is_some_and(|status| status.matches_target(enabled))
        && (!enabled || runtime.is_some_and(|status| status.verified()))
}

/// 主屏激活动作只在正常执行中发送一次，结果核实与中断恢复只能读取。
async fn activate_home(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    original: &crate::domain::smart_screen::model::ScreenObservation,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    device: &AdbDevice,
    enabled: Option<bool>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    if enabled != Some(true) {
        return Ok(());
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    result.device = ResultState::Unknown;
    result.evidence["home"]["readback"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    let policy =
        super::home::wait_for_target(device, &screen.fields.ip, true, Duration::from_secs(20))
            .await?;
    let matched = policy.matches_target(true);
    result.evidence["home"]["after"] = json!(policy);
    result.evidence["home"]["readback"] = json!(if matched { "succeeded" } else { "failed" });
    checkpoint(state, id, plan, result).await?;
    if !matched {
        result.device = ResultState::Failed;
        result.message = "持久主屏规则尚未确认，未激活系统主屏；已有普通配置结果保留".into();
        checkpoint(state, id, plan, result).await?;
        return Err(AppError::Conflict(result.message.clone()));
    }
    result.evidence["home"]["runtimeReadback"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    let runtime = super::home::query_runtime(device, &screen.fields.ip, cancel.clone()).await?;
    result.evidence["home"]["runtimeAfter"] = json!(runtime);
    if runtime.verified() {
        result.evidence["home"]["activation"] = json!("unchanged");
        result.evidence["home"]["runtimeReadback"] = json!("succeeded");
        return checkpoint(state, id, plan, result).await;
    }
    // 运行状态与策略读取可能耗时，副作用前重新确认当前资产、设备身份及写租约。
    authorize(state, plan, held).await?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let current = repo.asset(&plan.project_id, &screen.id).await?;
    let observed = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || super::value_updates::critical_draft(&repo.snapshot(&plan.project_id).await?, &current)
        || !reliable_identity(screen, &observed)
        || observed.observed_mac != original.observed_mac
    {
        return Err(AppError::Conflict(
            "屏身份已变化，未激活系统主屏；已有配置和主屏策略结果保留".into(),
        ));
    }
    authorize(state, plan, held).await?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    super::installation_progress::ScreenOperationProgress {
        state,
        task: id,
        screen,
    }
    .report(
        "激活系统主屏",
        75,
        "正在切换到系统主屏，使小新作为主屏应用运行",
        true,
    )
    .await?;
    result.evidence["phase"] = json!("config_home_activation_requested");
    result.evidence["home"]["activation"] = json!("unknown");
    result.evidence["home"]["runtimeReadback"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    authorize(state, plan, held).await?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    super::home::activate_home(device, &screen.fields.ip, cancel.clone()).await?;
    result.evidence["home"]["activation"] = json!("activated");
    result.evidence["phase"] = json!("config_home_activated");
    checkpoint(state, id, plan, result).await
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
        .app_config
        .as_ref()
        .and_then(|p| p.get(&screen.id))
        .ok_or_else(|| AppError::Conflict("缺少本次配置修改内容".into()))?;
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
        return Err(AppError::Conflict("屏资料已变化，未修改配置".into()));
    }
    if screen.source == "platform" {
        let ctx = super::write_context::open(state, &plan.project_id).await?;
        let current = super::platform_write::record(&ctx.read, &screen.id)
            .await?
            .ok_or_else(|| AppError::Conflict("平台屏已不存在".into()))?;
        if current.business.as_deref() != Some(&ctx.business)
            || !crate::application::smart_screen::operations::same_target(screen, &current.asset)
        {
            return Err(AppError::Conflict("平台屏身份已变化，未修改配置".into()));
        }
    }
    let device = AdbDevice::new(AndroidTools::discover()?);
    let progress = super::installation_progress::ScreenOperationProgress {
        state,
        task: id,
        screen,
    };
    let observed = progress
        .wait(
            "读取当前配置",
            5,
            "正在重新确认设备与小新配置",
            device.inspect(screen, "inspect", cancel.clone()),
        )
        .await?;
    if !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(AppError::Conflict(
            "屏身份与检查时不一致，未修改配置".into(),
        ));
    }
    let (_, before) = read_device(&device, &screen.fields.ip, Some(patch)).await?;
    let home_before = if patch.home_enabled.is_some() {
        Some(super::home::query(&device, &screen.fields.ip).await?)
    } else {
        None
    };
    // 显式开启时，任何写入前先确认固件的运行态可以可靠读取。
    let runtime_before = if patch.home_enabled == Some(true) {
        Some(super::home::query_runtime(&device, &screen.fields.ip, cancel.clone()).await?)
    } else {
        None
    };
    result.observation = Some(observed.clone());
    result.evidence = json!({"phase":"config_checked","config":{"before":before,"fields":patch.fields(),"save":"pending","restart":"not_required","readback":"pending"}});
    if let Some(enabled) = patch.home_enabled {
        result.evidence["home"] = json!({"component":super::home::COMPONENT,"targetEnabled":enabled,"before":home_before,"save":"pending","readback":"pending","activation":if enabled { "pending" } else { "not_required" },"runtimeReadback":if enabled { "pending" } else { "not_required" },"runtimeBefore":runtime_before});
    }
    if patch.matches(&before) {
        let no_fields = patch.fields().is_empty();
        result.evidence["config"]["save"] = json!(if no_fields {
            "not_required"
        } else {
            "unchanged"
        });
        result.evidence["config"]["readback"] = json!(if no_fields {
            "not_required"
        } else {
            "succeeded"
        });
        result.evidence["config"]["after"] = before.clone();
        if patch.home_enabled.is_some_and(|enabled| {
            !home_request_satisfied(enabled, home_before.as_ref(), runtime_before.as_ref())
        }) {
            result.device = ResultState::Unknown;
            checkpoint(state, id, plan, result).await?;
            apply_home(
                state,
                id,
                plan,
                screen,
                original,
                held,
                result,
                &device,
                patch.home_enabled,
                &cancel,
            )
            .await?;
            activate_home(
                state,
                id,
                plan,
                screen,
                original,
                held,
                result,
                &device,
                patch.home_enabled,
                &cancel,
            )
            .await?;
            return verify(state, id, plan, data, screen, result).await;
        }
        if home_before.is_some() {
            result.evidence["home"]["after"] = json!(home_before);
            result.evidence["home"]["save"] = json!("unchanged");
            result.evidence["home"]["readback"] = json!("succeeded");
            if patch.home_enabled == Some(true) {
                result.evidence["home"]["activation"] = json!("unchanged");
                result.evidence["home"]["runtimeReadback"] = json!("succeeded");
                result.evidence["home"]["runtimeAfter"] = json!(runtime_before);
            }
        }
        result.device = ResultState::Skipped;
        result.message = if patch.home_enabled == Some(true) {
            "所选配置、持久主屏规则和系统主屏运行状态已符合要求，未重复操作"
        } else if patch.home_enabled.is_some() {
            "所选配置和主屏设置已符合要求，未重复保存或重启"
        } else {
            "配置已经符合本次设置，未重复保存或重启"
        }
        .into();
        return checkpoint(state, id, plan, result).await;
    }
    let restart = patch.restart_required(&before);
    let capability = if restart {
        Some(device_maintenance::capability(&device, screen, &observed, "restart").await?)
    } else {
        None
    };
    result.evidence["config"]["restartRequired"] = json!(restart);
    result.evidence["config"]["beforePid"] = capability
        .as_ref()
        .map(|v| v["pid"].clone())
        .unwrap_or(Value::Null);
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    authorize(state, plan, held).await?;
    progress
        .report("保存配置", 25, "正在保存明确选择的字段，其他配置保留", true)
        .await?;
    result.device = ResultState::Unknown;
    result.evidence["phase"] = json!("config_save_requested");
    result.evidence["config"]["save"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    let request = json!({"protocolVersion":1,"requestId":data.request_ids.get(&screen.id),"set":patch.set,"clear":patch.clear});
    let saved = progress
        .wait(
            "保存配置",
            25,
            "正在等待小新确认保存结果",
            call(&device, &screen.fields.ip, "saveConfig", Some(&request)),
        )
        .await?;
    if matches!(
        saved["code"].as_str(),
        Some("TIMEOUT" | "STORAGE_ERROR" | "INVALID_RESPONSE" | "RESPONSE_TOO_LARGE")
    ) {
        return Err(AppError::Conflict(
            "小新尚未确认配置保存结果，请重新读取核实；未重复保存或重启".into(),
        ));
    }
    let status = saved["status"].as_str().unwrap_or("failed");
    if status == "ok"
        || (status == "failed" && saved.get("code").is_none() && !saved["failedFields"].is_array())
    {
        return Err(AppError::Conflict(
            "小新未返回完整的保存结果，请重新读取核实".into(),
        ));
    }
    if matches!(status, "saved" | "partial" | "unchanged")
        && (!saved["savedFields"].is_array()
            || !saved["failedFields"].is_array()
            || !saved["restartRequired"].is_boolean())
    {
        return Err(AppError::Conflict(
            "小新保存结果不完整，请重新读取核实".into(),
        ));
    }
    let fields = patch.fields();
    for key in ["savedFields", "failedFields"] {
        if let Some(items) = saved[key].as_array() {
            if items.iter().any(|f| {
                !f.as_str()
                    .is_some_and(|s| fields.iter().any(|v| v == s) || s == "h5ReadyRecord")
            }) {
                return Err(AppError::Conflict(
                    "小新保存结果字段不完整，请重新读取核实".into(),
                ));
            }
        }
    }
    result.evidence["config"]["save"] = json!(status);
    result.evidence["config"]["savedFields"] = saved["savedFields"].clone();
    result.evidence["config"]["failedFields"] = saved["failedFields"].clone();
    result.evidence["config"]["restartRequired"] = saved
        .get("restartRequired")
        .cloned()
        .unwrap_or(json!(false));
    result.evidence["phase"] = json!("config_saved");
    checkpoint(state, id, plan, result).await?;
    if !["saved", "unchanged"].contains(&status) {
        result.device = ResultState::Failed;
        result.message = if status == "partial" {
            "配置仅部分保存，未重启；请重新读取并检查未保存字段"
        } else {
            "配置未保存，未重启；请检查内容和小新存储状态"
        }
        .into();
        result.evidence["config"]["restart"] = json!("not_started");
        return checkpoint(state, id, plan, result).await;
    }
    apply_home(
        state,
        id,
        plan,
        screen,
        original,
        held,
        result,
        &device,
        patch.home_enabled,
        &cancel,
    )
    .await?;
    if saved["restartRequired"] == true {
        let Some(capability) = capability else {
            // 实际屏在读取后切换了环境，重新检查重启条件。
            result.device = ResultState::Failed;
            result.evidence["config"]["restart"] = json!("not_started");
            result.message = "配置已保存，实际运行环境已变化；请单独重启小新使配置生效".into();
            return checkpoint(state, id, plan, result).await;
        };
        progress
            .report("重启小新", 55, "配置已保存，正在重启小新应用", true)
            .await?;
        authorize(state, plan, held).await?;
        result.evidence["config"]["restart"] = json!("unknown");
        result.evidence["config"]["restartRequested"] = json!(true);
        result.evidence["phase"] = json!("config_restart_requested");
        checkpoint(state, id, plan, result).await?;
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        device_maintenance::stop_application(&device, &screen.fields.ip, &capability).await?;
        result.evidence["phase"] = json!("config_app_stopped");
        checkpoint(state, id, plan, result).await?;
        authorize(state, plan, held).await?;
        if cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        device_maintenance::start_application(&device, &screen.fields.ip).await?;
        result.evidence["phase"] = json!("config_app_started");
        checkpoint(state, id, plan, result).await?;
    }
    if patch.home_enabled == Some(true) && saved["restartRequired"] == true {
        progress
            .report(
                "等待小新配置就绪",
                65,
                "正在确认重启后的小新配置，再激活系统主屏",
                true,
            )
            .await?;
        let mut ready = None;
        for attempt in 0..12 {
            if cancel.is_cancelled() {
                return Err(AppError::Cancelled);
            }
            match read_device(&device, &screen.fields.ip, Some(patch)).await {
                Ok((_, after)) => {
                    ready = Some(Ok(after));
                    break;
                }
                Err(error) => ready = Some(Err(error)),
            }
            if attempt < 11 {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        let after =
            ready.unwrap_or_else(|| Err(AppError::Conflict("重启后小新配置尚未就绪".into())))?;
        result.evidence["config"]["after"] = after.clone();
        result.evidence["config"]["readAt"] = json!(now());
        if !patch.matches(&after) {
            result.device = ResultState::Failed;
            result.evidence["config"]["readback"] = json!("different");
            result.message =
                "配置重启后回读与本次设置不同，未继续激活主屏；已有保存结果保留".into();
            return checkpoint(state, id, plan, result).await;
        }
        result.evidence["config"]["readback"] = json!("succeeded");
        checkpoint(state, id, plan, result).await?;
    }
    activate_home(
        state,
        id,
        plan,
        screen,
        original,
        held,
        result,
        &device,
        patch.home_enabled,
        &cancel,
    )
    .await?;
    progress
        .report(
            if patch.home_enabled == Some(true) {
                "核验系统主屏"
            } else {
                "回读配置"
            },
            85,
            if patch.home_enabled == Some(true) {
                "正在确认持久规则、小新主屏任务与前台运行状态"
            } else {
                "正在读取屏端保存结果，确认应用状态"
            },
            true,
        )
        .await?;
    // 仅重复读取，不重复保存或重启。
    let mut last = None;
    for attempt in 0..12 {
        match verify(state, id, plan, data, screen, result).await {
            Ok(()) => return Ok(()),
            Err(error) => last = Some(error),
        }
        if attempt < 11 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
    Err(last.unwrap_or_else(|| AppError::Conflict("配置结果尚未确认，请重新读取".into())))
}

async fn finish_verification(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    result: &mut ScreenTargetResult,
    home_error: Option<AppError>,
) -> AppResult<()> {
    if let Some(error) = home_error {
        result.device = ResultState::Unknown;
        result.message = format!("普通配置结果已记录；主屏策略或运行状态尚待核实：{error}");
        checkpoint(state, id, plan, result).await?;
        return Err(error);
    }
    merge_home_verification(result);
    checkpoint(state, id, plan, result).await
}

fn merge_home_verification(result: &mut ScreenTargetResult) {
    let Some(home) = result.evidence.get("home") else {
        return;
    };
    let enabled = home["targetEnabled"] == true;
    if home["readback"] == "unknown" || (enabled && home["runtimeReadback"] == "unknown") {
        result.device = ResultState::Unknown;
        result.message =
            "普通配置的已有保存结果保留；主屏策略或运行状态仍待核实，不自动重复保存、激活或重启"
                .into();
    } else if home["readback"] != "succeeded" || home["save"] == "rejected" {
        result.device = ResultState::Failed;
        result.message =
            "所选普通配置和主屏策略未全部完成；已有保存结果保留，请分别查看结果，未重复保存或重启"
                .into();
    } else if enabled && home["runtimeReadback"] != "succeeded" {
        result.device = ResultState::Failed;
        result.message = "持久主屏规则已确认，但未确认小新作为系统主屏在前台运行；请重新读取并激活主屏，已有配置保存结果保留".into();
    } else if result.device == ResultState::Succeeded || result.device == ResultState::Skipped {
        result.device = ResultState::Succeeded;
        let home_message = if enabled {
            "小新持久主屏规则、主屏任务和前台运行状态均已确认"
        } else {
            "小新持久主屏已解除；当前普通主屏选择保持系统实际状态"
        };
        result.message = if result.evidence["config"]["fields"]
            .as_array()
            .is_some_and(Vec::is_empty)
        {
            format!("{home_message}，本次未重启应用或屏、未清空应用数据")
        } else {
            format!("{}；{home_message}", result.message)
        };
    } else {
        result
            .message
            .push_str("；主屏设置已回读确认，普通配置仍有未完成项");
    }
}

fn restart_was_requested(evidence: &Value) -> bool {
    evidence["config"]["restartRequested"] == true
        || matches!(
            evidence["phase"].as_str(),
            Some(
                "config_restart_requested"
                    | "config_app_stopped"
                    | "config_app_started"
                    | "config_verified"
            )
        )
}

/// 结果核实只读取；任何不确定的保存、主屏策略或重启都不自动重放。
pub async fn verify(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let patch = data
        .app_config
        .as_ref()
        .and_then(|p| p.get(&screen.id))
        .ok_or_else(|| AppError::Conflict("缺少原配置修改内容".into()))?;
    let original = data
        .observations
        .get(&screen.id)
        .ok_or_else(|| AppError::Conflict("缺少原设备身份".into()))?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    let observed = device
        .inspect(screen, "inspect", CancellationToken::new())
        .await?;
    if !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(AppError::Conflict(
            "未确认原设备身份，保留待核实结果".into(),
        ));
    }
    let mut home_error = None;
    if let Some(enabled) = patch.home_enabled {
        result.evidence["home"]["targetEnabled"] = json!(enabled);
        result.evidence["home"]["component"] = json!(super::home::COMPONENT);
        result.evidence["home"]["readback"] = json!("unknown");
        match super::home::wait_for_target(
            &device,
            &screen.fields.ip,
            enabled,
            Duration::from_secs(20),
        )
        .await
        {
            Ok(status) => {
                let matched = status.matches_target(enabled);
                result.evidence["home"]["after"] = json!(status);
                result.evidence["home"]["readback"] =
                    json!(if matched { "succeeded" } else { "failed" });
                if matched && result.evidence["home"]["save"] == "unknown" {
                    result.evidence["home"]["save"] = json!("verified");
                } else if matched && result.evidence["home"]["save"] == "pending" {
                    result.evidence["home"]["save"] = json!("unchanged");
                }
            }
            Err(error) => home_error = Some(error),
        }
        if enabled {
            result.evidence["home"]["runtimeReadback"] = json!("unknown");
            match super::home::wait_runtime_verified(
                &device,
                &screen.fields.ip,
                Duration::from_secs(20),
                CancellationToken::new(),
            )
            .await
            {
                Ok(runtime) => {
                    let verified = runtime.verified();
                    result.evidence["home"]["runtimeAfter"] = json!(runtime);
                    result.evidence["home"]["runtimeReadback"] =
                        json!(if verified { "succeeded" } else { "failed" });
                    if verified
                        && (result.evidence["home"]["activation"] == "unknown"
                            || result.evidence["home"]["activation"].is_null())
                    {
                        result.evidence["home"]["activation"] = json!("verified");
                    } else if verified && result.evidence["home"]["activation"] == "pending" {
                        result.evidence["home"]["activation"] = json!("unchanged");
                    }
                }
                Err(error) => {
                    if home_error.is_none() {
                        home_error = Some(error);
                    }
                }
            }
        } else {
            result.evidence["home"]["activation"] = json!("not_required");
            result.evidence["home"]["runtimeReadback"] = json!("not_required");
        }
    }
    if patch.fields().is_empty() {
        result.observation = Some(observed);
        result.evidence["config"]["save"] = json!("not_required");
        result.evidence["config"]["readback"] = json!("not_required");
        result.evidence["config"]["restart"] = json!("not_required");
        result.device = ResultState::Succeeded;
        return finish_verification(state, id, plan, result, home_error).await;
    }
    let (_, after) = read_device(&device, &screen.fields.ip, Some(patch)).await?;
    result.evidence["config"]["after"] = after.clone();
    result.evidence["config"]["readAt"] = json!(now());
    result.observation = Some(observed);
    if matches!(
        result.evidence["config"]["save"].as_str(),
        Some("partial" | "failed")
    ) {
        result.device = ResultState::Failed;
        result.evidence["config"]["readback"] = json!(if patch.matches(&after) {
            "succeeded"
        } else {
            "different"
        });
        result.message =
            "已重新读取当前配置；原保存未全部成功，未自动重启，请检查未保存字段".into();
        return finish_verification(state, id, plan, result, home_error).await;
    }
    if !patch.matches(&after) {
        result.device = ResultState::Failed;
        result.evidence["config"]["readback"] = json!("different");
        result.message = "回读值与本次设置不同，请检查屏端当前配置；未重复保存或重启".into();
    } else {
        result.evidence["config"]["readback"] = json!("succeeded");
        if result.evidence["config"]["save"] == "unknown" {
            result.evidence["config"]["save"] = json!("verified");
        }
        let needs_restart = result.evidence["config"]["restartRequired"] == true;
        if needs_restart {
            let pid = device
                .shell(
                    &screen.fields.ip,
                    &["pidof", super::device::XIAOXIN_PACKAGE],
                    CancellationToken::new(),
                )
                .await?;
            let before = result.evidence["config"]["beforePid"]
                .as_str()
                .unwrap_or("");
            let sent = restart_was_requested(&result.evidence);
            if !sent || pid.is_empty() || pid == before {
                result.device = ResultState::Failed;
                result.evidence["config"]["restart"] = json!("not_confirmed");
                result.message =
                    "配置已保存并回读一致，但未确认小新完成重启；只需单独重启小新，无需重新保存"
                        .into();
                return finish_verification(state, id, plan, result, home_error).await;
            }
            result.evidence["config"]["restart"] = json!("succeeded");
        }
        result.device = ResultState::Succeeded;
        result.evidence["phase"] = json!("config_verified");
        result.message = if needs_restart {
            "配置已保存，小新已重启，回读一致"
        } else {
            "配置已保存并回读一致，本次无需重启"
        }
        .into();
    }
    finish_verification(state, id, plan, result, home_error).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabled_policy_without_active_home_task_is_not_skipped() {
        let mut policy = super::super::home::HomeStatus {
            protocol_version: 1,
            package_name: "chat.xiaoxin.app".into(),
            device_owner: true,
            persistent_preferred: true,
            resolved_home: Some(super::super::home::COMPONENT.into()),
        };
        let mut runtime = super::super::home::HomeRuntimeStatus {
            home_task: false,
            resumed: true,
            pid: Some(321),
            task_id: Some(9),
            component: Some(super::super::home::COMPONENT.into()),
        };
        assert!(!home_request_satisfied(true, Some(&policy), Some(&runtime)));
        assert!(!home_request_satisfied(true, Some(&policy), None));
        runtime.home_task = true;
        assert!(home_request_satisfied(true, Some(&policy), Some(&runtime)));
        runtime.resumed = false;
        assert!(!home_request_satisfied(true, Some(&policy), Some(&runtime)));
        policy.persistent_preferred = false;
        assert!(home_request_satisfied(false, Some(&policy), None));
    }

    #[test]
    fn successful_policy_write_requires_separate_runtime_readback() {
        let mut result = ScreenTargetResult {
            device: ResultState::Succeeded,
            evidence: json!({"config":{"fields":[],"save":"not_required"},"home":{"targetEnabled":true,"save":"applied","readback":"succeeded","activation":"activated","runtimeReadback":"failed"}}),
            ..Default::default()
        };
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Failed);
        assert!(result.message.contains("未确认小新作为系统主屏"));
        result.evidence["home"]["activation"] = json!("unknown");
        result.evidence["home"]["runtimeReadback"] = json!("unknown");
        result.device = ResultState::Succeeded;
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Unknown);
        result.evidence["home"]["activation"] = json!("verified");
        result.evidence["home"]["runtimeReadback"] = json!("succeeded");
        result.device = ResultState::Succeeded;
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Succeeded);
        assert!(result.message.contains("主屏任务和前台运行状态均已确认"));
    }

    #[test]
    fn home_activation_phase_preserves_prior_configuration_restart_evidence() {
        assert!(restart_was_requested(
            &json!({"phase":"config_home_activated","config":{"restartRequested":true}})
        ));
        assert!(!restart_was_requested(
            &json!({"phase":"config_home_activated","config":{"restartRequired":true}})
        ));
        assert!(restart_was_requested(
            &json!({"phase":"config_app_started","config":{"restartRequired":true}})
        ));
    }

    #[test]
    fn mixed_configuration_results_require_both_sides_and_preserve_saved_fields() {
        let mut result = ScreenTargetResult {
            device: ResultState::Succeeded,
            evidence: json!({"config":{"fields":["customDeviceName"],"save":"saved","savedFields":["customDeviceName"],"readback":"succeeded"},"home":{"targetEnabled":true,"save":"applied","readback":"failed","runtimeReadback":"succeeded"}}),
            ..Default::default()
        };
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Failed);
        assert_eq!(result.evidence["config"]["save"], "saved");
        assert_eq!(
            result.evidence["config"]["savedFields"],
            json!(["customDeviceName"])
        );
        result.evidence["home"]["readback"] = json!("succeeded");
        result.evidence["config"]["save"] = json!("partial");
        result.device = ResultState::Failed;
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Failed);
        assert_eq!(result.evidence["home"]["readback"], "succeeded");
        result.device = ResultState::Succeeded;
        result.evidence["home"]["readback"] = json!("unknown");
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Unknown);
    }

    #[test]
    fn home_only_clear_and_recovered_write_do_not_require_config_save_or_restart() {
        let mut result = ScreenTargetResult {
            device: ResultState::Succeeded,
            evidence: json!({"config":{"fields":[],"save":"not_required","readback":"not_required","restart":"not_required"},"home":{"targetEnabled":false,"save":"verified","readback":"succeeded","after":{"deviceOwner":true,"persistentPreferred":false,"resolvedHome":super::super::home::COMPONENT}}}),
            ..Default::default()
        };
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Succeeded);
        assert_eq!(result.evidence["config"]["save"], "not_required");
        assert_eq!(result.evidence["config"]["restart"], "not_required");
        assert!(result.message.contains("已解除"));
        result.evidence["home"]["save"] = json!("rejected");
        merge_home_verification(&mut result);
        assert_eq!(result.device, ResultState::Failed);
    }

    struct OldApplication;
    impl DeviceCommandPort for OldApplication {
        async fn execute(
            &self,
            _: &std::path::Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<super::super::device::ProcessOutput> {
            assert!(args.iter().any(|arg| arg == "readHome"));
            Ok(super::super::device::ProcessOutput {
                success: true,
                stdout: "Could not find provider: chat.xiaoxin.app.workbench.home".into(),
                stderr: String::new(),
            })
        }
    }
    #[tokio::test]
    async fn unavailable_home_is_an_optional_capability_not_a_configuration_read_error() {
        let device = AdbDevice::with_commands(AndroidTools { adb: "adb".into() }, OldApplication);
        let home = read_home(&device, "192.0.2.1").await;
        assert!(!home.supported);
        assert!(home.enabled.is_none());
        assert!(home.active.is_none());
        assert!(home.message.contains("升级"));
    }

    struct UnknownRuntime;
    impl DeviceCommandPort for UnknownRuntime {
        async fn execute(
            &self,
            _: &std::path::Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<super::super::device::ProcessOutput> {
            let stdout = if args.iter().any(|arg| arg == "readHome") {
                let response = json!({"status":"ok","data":{"protocolVersion":1,"packageName":"chat.xiaoxin.app","deviceOwner":true,"resolvedHome":"chat.xiaoxin.app/.MainActivity"}});
                format!(
                    "Result: Bundle[{{result={}}}]",
                    STANDARD.encode(serde_json::to_vec(&response).unwrap())
                )
            } else if args.last().is_some_and(|arg| arg == "id") {
                "uid=0(root) gid=0(root)".into()
            } else if args.iter().any(|arg| arg == "cat") {
                r#"<package-restrictions><persistent-preferred-activities><item name="chat.xiaoxin.app/.MainActivity"><filter><action name="android.intent.action.MAIN"/><cat name="android.intent.category.HOME"/><cat name="android.intent.category.DEFAULT"/></filter></item></persistent-preferred-activities></package-restrictions>"#.into()
            } else if args.iter().any(|arg| arg == "resolve-activity") {
                "chat.xiaoxin.app/.MainActivity".into()
            } else {
                assert!(args.ends_with(&[
                    "dumpsys".into(),
                    "activity".into(),
                    "activities".into()
                ]));
                "unsupported runtime dump".into()
            };
            Ok(super::super::device::ProcessOutput {
                success: true,
                stdout,
                stderr: String::new(),
            })
        }
    }
    #[tokio::test]
    async fn unreadable_runtime_preserves_known_policy_and_does_not_disable_configuration() {
        let device = AdbDevice::with_commands(AndroidTools { adb: "adb".into() }, UnknownRuntime);
        let home = read_home(&device, "192.0.2.1").await;
        assert!(home.supported);
        assert_eq!(home.enabled, Some(true));
        assert_eq!(home.active, None);
        assert!(home.message.contains("运行状态尚待核实"));
    }
    #[test]
    fn strict_bundle_parser_never_guesses_success() {
        let value = json!({"status":"ok","data":{}});
        let encoded = STANDARD.encode(serde_json::to_vec(&value).unwrap());
        assert_eq!(
            parse_response(&format!("Result: Bundle[{{result={encoded}}}]")).unwrap(),
            value
        );
        for bad in [
            "Success",
            "Result: Bundle[{result=bad}]",
            "",
            "Error: provider not found",
        ] {
            assert!(parse_response(bad).is_err());
        }
    }
}
