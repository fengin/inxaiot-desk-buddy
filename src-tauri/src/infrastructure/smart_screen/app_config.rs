use super::device::{AdbDevice, AndroidTools, DeviceCommandPort};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::app_config::{AppConfigPatch, AppConfigRead};
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
    if output.stdout.contains("Could not find provider: chat.xiaoxin.app.workbench.config") || output.stderr.contains("Could not find provider: chat.xiaoxin.app.workbench.config") {
        return Err(AppError::Conflict("当前小新未提供远程配置功能，请先升级到支持配置管理的版本".into()));
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
        rows.push(AppConfigRead {
            screen_id: id.clone(),
            read_at: now(),
            config,
            capabilities,
            message,
        });
    }
    Ok(rows)
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
    result.observation = Some(observed.clone());
    result.evidence = json!({"phase":"config_checked","config":{"before":before,"fields":patch.fields(),"save":"pending","restart":"not_required","readback":"pending"}});
    if patch.matches(&before) {
        result.device = ResultState::Skipped;
        result.evidence["config"]["save"] = json!("unchanged");
        result.evidence["config"]["readback"] = json!("succeeded");
        result.message = "配置已经符合本次设置，未重复保存或重启".into();
        checkpoint(state, id, plan, result).await?;
        return Ok(());
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
    if status == "ok" || (status == "failed" && saved.get("code").is_none() && !saved["failedFields"].is_array()) {
        return Err(AppError::Conflict("小新未返回完整的保存结果，请重新读取核实".into()));
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
        result.evidence["phase"] = json!("config_restart_requested");
        checkpoint(state, id, plan, result).await?;
        device_maintenance::stop_application(&device, &screen.fields.ip, &capability).await?;
        result.evidence["phase"] = json!("config_app_stopped");
        checkpoint(state, id, plan, result).await?;
        authorize(state, plan, held).await?;
        device_maintenance::start_application(&device, &screen.fields.ip).await?;
        result.evidence["phase"] = json!("config_app_started");
        checkpoint(state, id, plan, result).await?;
    }
    progress
        .report("回读配置", 80, "正在读取屏端保存结果，确认应用状态", true)
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

/// 结果核实只读取；任何不确定的保存或重启都不自动重放。
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
        return checkpoint(state, id, plan, result).await;
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
            let sent = matches!(
                result.evidence["phase"].as_str(),
                Some(
                    "config_restart_requested"
                        | "config_app_stopped"
                        | "config_app_started"
                        | "config_verified"
                )
            );
            if !sent || pid.is_empty() || pid == before {
                result.device = ResultState::Failed;
                result.evidence["config"]["restart"] = json!("not_confirmed");
                result.message =
                    "配置已保存并回读一致，但未确认小新完成重启；只需单独重启小新，无需重新保存"
                        .into();
                return checkpoint(state, id, plan, result).await;
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
    checkpoint(state, id, plan, result).await
}

#[cfg(test)]
mod tests {
    use super::*;
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
