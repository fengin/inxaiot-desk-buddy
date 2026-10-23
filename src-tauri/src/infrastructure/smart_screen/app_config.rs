use super::device::{AdbDevice, AndroidTools, DeviceCommandPort};
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::app_config::{AppConfigPatch, AppConfigRead, AppHomeRead};
use crate::domain::smart_screen::app_config_schema::{self as config_schema, ConfigSchema};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::local_sqlite::screen_repository::{ScreenRepository, now};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};
use tokio_util::sync::CancellationToken;

pub(super) const URI: &str = "content://chat.xiaoxin.app.workbench.config";

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
            "homeActivation",
            "ensureDeviceOwner",
            "kioskMode",
            "voiceMode",
            "voiceEdits",
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
    if draft.get("homeActivation").is_some_and(|value| !value.is_boolean()) {
        return Err(AppError::InvalidConfig("切到小新主屏草稿无效，请重新选择".into()));
    }
    if draft.get("ensureDeviceOwner").is_some_and(|value| !value.is_boolean())
        || draft.get("kioskMode").is_some_and(|mode| !matches!(mode.as_str(), Some("keep" | "on" | "off"))) {
        return Err(AppError::InvalidConfig("设备所有者或锁定草稿无效，请重新读取".into()));
    }
    validate_voice_draft(draft.get("voiceEdits"))?;
    // 字段定义来自设备；草稿恢复必须重新读取，避免为每台重复持久化同一schema。
    if let Some(rows) = draft["rows"].as_array_mut() {
        for row in rows {
            if let Some(cap) = row.get_mut("capabilities").and_then(Value::as_object_mut) {
                cap.remove("configSchema");
            }
        }
    }
    if draft
        .get("voiceMode")
        .is_some_and(|mode| !matches!(mode.as_str(), Some("keep" | "on" | "off")))
    {
        return Err(AppError::InvalidConfig(
            "语音引擎随应用启动草稿无效，请重新选择".into(),
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

fn validate_voice_draft(value: Option<&Value>) -> AppResult<()> {
    let Some(value) = value else { return Ok(()); };
    let invalid = || AppError::InvalidConfig("语音配置草稿无效，请重新读取配置".into());
    let edits = value.as_object().ok_or_else(invalid)?;
    if edits.len() > 64 { return Err(invalid()); }
    for (key, value) in edits {
        if key != "autoStartVoiceEngine" && !config_schema::audio_field(key) { return Err(invalid()); }
        let edit = value.as_object().ok_or_else(invalid)?;
        if edit.keys().any(|key| !["mode", "value"].contains(&key.as_str())) { return Err(invalid()); }
        let candidate = edit.get("value");
        let scalar = |value: &Value| value.is_boolean() || value.as_f64().is_some_and(f64::is_finite)
            || value.as_str().is_some_and(|s| s.chars().count() <= 4096 && !s.chars().any(char::is_control));
        match edit.get("mode").and_then(Value::as_str) {
            Some("keep") if candidate.is_none_or(|v| v.is_null() || scalar(v)) => {},
            Some("set") if candidate.is_some_and(scalar) => {},
            _ => return Err(invalid()),
        }
    }
    Ok(())
}

pub async fn call<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    method: &str,
    request: Option<&Value>,
) -> AppResult<Value> {
    if !["getCapabilities", "readConfig", "saveConfig", "readKiosk", "saveKiosk", "applyKiosk"].contains(&method) {
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
        Some("ok" | "saved" | "partial" | "failed" | "unchanged" | "applied" | "pending")
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
fn validated_save_status<'a>(saved: &'a Value, patch: &AppConfigPatch) -> AppResult<&'a str> {
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
            if items.iter().any(|field| {
                !field
                    .as_str()
                    .is_some_and(|name| fields.iter().any(|value| value == name) || name == "h5ReadyRecord")
            }) {
                return Err(AppError::Conflict(
                    "小新保存结果字段不完整，请重新读取核实".into(),
                ));
            }
        }
    }
    Ok(status)
}
pub async fn read_device<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    patch: Option<&AppConfigPatch>,
) -> AppResult<(Value, Value)> {
    if let Some(patch) = patch { patch.validate()?; }
    device.connect(ip, CancellationToken::new()).await?;
    let received_cap = accepted(call(device, ip, "getCapabilities", None).await?)?;
    let (cap, schema) = parse_capabilities(received_cap, patch)?;
    let config = accepted(call(device, ip, "readConfig", None).await?)?;
    project_configuration(cap, schema, config, patch)
}

fn parse_capabilities(received_cap:Value,patch:Option<&AppConfigPatch>)->AppResult<(Value,Option<ConfigSchema>)> {
    let mut cap = json!({});
    for key in ["packageName", "appVersionName", "appVersionCode", "protocolVersion", "ready", "supportedFields", "configSchema", "kioskManagement", "homeManagement"] {
        if let Some(value) = received_cap.get(key) { cap[key] = value.clone(); }
    }
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
    let schema = match ConfigSchema::from_capabilities(&cap) {
        Ok(schema) => schema,
        Err(message) => {
            cap.as_object_mut().unwrap().remove("configSchema");
            cap["configSchemaError"] = json!(message);
            cap["supportedFields"] = json!(cap["supportedFields"].as_array().into_iter().flatten()
                .filter_map(|field| field.as_str().filter(|key| config_schema::legacy_field(key))).collect::<BTreeSet<_>>());
            None
        }
    };
    if patch.is_some_and(|patch| patch.fields().iter().any(|key| config_schema::audio_field(key))) && schema.is_none() {
        return Err(AppError::Conflict(cap["configSchemaError"].as_str()
            .unwrap_or("当前小新尚未开放语音参数，请升级小新应用后重新读取").into()));
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
    Ok((cap,schema))
}

fn project_configuration(mut cap:Value,schema:Option<ConfigSchema>,config:Value,patch:Option<&AppConfigPatch>)->AppResult<(Value,Value)> {
    // 不保存设备返回的任意私有字段。
    if config.get("customDeviceName").is_none()
        || !["test", "pre", "prod"]
            .contains(&config["environments"]["current"].as_str().unwrap_or(""))
    {
        return Err(AppError::Conflict("小新配置返回不完整".into()));
    }
    let mut safe = json!({"customDeviceName":config["customDeviceName"],"environments":{"current":config["environments"]["current"]}});
    let voice_supported = cap["supportedFields"].as_array().is_some_and(|fields| fields.contains(&json!("autoStartVoiceEngine")));
    if let Some(value) = config.get("autoStartVoiceEngine").filter(|value| voice_supported && value.is_boolean()) {
        safe["autoStartVoiceEngine"] = value.clone();
    } else if patch.is_some_and(|patch| patch.set.contains_key("autoStartVoiceEngine") || patch.clear.iter().any(|field| field == "autoStartVoiceEngine")) {
        return Err(AppError::Conflict(
            "小新未返回有效的语音引擎随应用启动开关，不能修改此项；请升级应用或重新读取".into(),
        ));
    }
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
    if let Some(schema) = &schema {
        cap["configSchema"] = serde_json::to_value(schema).map_err(|_| AppError::Conflict("小新字段定义无法读取".into()))?;
        let mut audio = serde_json::Map::new();
        for field in schema.fields.iter().filter(|field| config_schema::audio_field(&field.key)) {
            if let Some(value) = crate::domain::smart_screen::app_config::field_value(&config, &field.key).filter(|value| field.readable_value(value)) {
                audio.insert(field.key.trim_start_matches("audioConfig.").into(), value.clone());
            }
        }
        safe["audioConfig"] = Value::Object(audio);
        if let Some(patch) = patch { schema.validate_patch(patch, &safe)?; }
    }
    Ok((cap, safe))
}

fn project_management_configuration(capabilities:&Value,configuration:&Value)->(Option<Value>,Option<Value>,String) {
    let (mut cap,schema)=match accepted(capabilities.clone()).and_then(|cap|parse_capabilities(cap,None)) {
        Ok(value)=>value,Err(error)=>return (None,None,error.to_string()),
    };
    // 即使配置段失败，也只保留已经校验并规范投影的能力段，不携带原始私有定义。
    if let Some(schema)=&schema {
        match serde_json::to_value(schema) {
            Ok(value)=>cap["configSchema"]=value,
            Err(_)=>return (None,None,"小新字段定义无法读取".into()),
        }
    }
    match accepted(configuration.clone()).and_then(|config|project_configuration(cap.clone(),schema,config,None)) {
        Ok((cap,config))=>(Some(config),Some(cap),"已从当前屏读取".into()),
        Err(error)=>(None,Some(cap),error.to_string()),
    }
}
pub async fn read(
    state: &FormalAppState,
    project: &str,
    ids: &[String],
) -> AppResult<Vec<AppConfigRead>> {
    read_with_device_factory(state,project,ids,||Ok(AdbDevice::new(AndroidTools::discover()?))).await
}

/// 与正式读取共用整批门禁及调度；允许回归注入只计数或模拟的设备端口。
#[doc(hidden)]
pub async fn read_with_device_factory<P:DeviceCommandPort,F:FnOnce()->AppResult<AdbDevice<P>> + Send>(
    state:&FormalAppState,project:&str,ids:&[String],device_factory:F,
)->AppResult<Vec<AppConfigRead>> {
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AppError::InvalidConfig("请选择不同的智能屏".into()));
    }
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let snapshot = repo.snapshot(project).await?;
    // 先解析完整批次；不能连过前几台后才发现后面混入另一尺寸或已移除目标。
    let selected = ids.iter().map(|id| snapshot.screens.iter().find(|screen| &screen.id == id)
        .ok_or_else(|| AppError::NotFound("该屏不在当前项目中".into())))
        .collect::<AppResult<Vec<_>>>()?;
    crate::domain::smart_screen::app_config::validate_batch_sizes(selected.iter().map(|screen| screen.fields.size.as_str()))?;
    let device = device_factory()?;
    Ok(super::configuration_reads::ordered(selected,super::configuration_reads::LIMIT,
        |screen| read_screen(&device,screen)).await)
}

async fn read_screen<P:DeviceCommandPort>(device:&AdbDevice<P>,screen:&ScreenAsset)->AppConfigRead {
    match super::management_snapshot::read(device,&screen.fields.ip).await {
        Ok(super::management_snapshot::Snapshot::Unsupported)=>read_legacy_screen(device,screen).await,
        Ok(super::management_snapshot::Snapshot::Data(sections))=>{
            let (config,capabilities,message)=project_management_configuration(&sections.capabilities,&sections.config);
            // 聚合只替代四个Provider读取；系统持久规则、解析、运行态和Owner仍独立核验。
            let home=Some(read_home_response(device,&screen.fields.ip,&sections.home).await);
            let owner=Some(super::device_owner::read(device,&screen.fields.ip).await);
            let kiosk=Some(super::kiosk::read_response(&sections.kiosk));
            AppConfigRead {screen_id:screen.id.clone(),read_at:now(),config,capabilities,home,owner,kiosk,message}
        }
        Err(error)=>AppConfigRead {screen_id:screen.id.clone(),read_at:now(),config:None,capabilities:None,home:None,owner:None,kiosk:None,message:error.to_string()},
    }
}

async fn read_legacy_screen<P:DeviceCommandPort>(device:&AdbDevice<P>,screen:&ScreenAsset)->AppConfigRead {
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
        let (owner, kiosk) = if config.is_some() {
            (Some(super::device_owner::read(&device, &screen.fields.ip).await), Some(super::kiosk::read(&device, &screen.fields.ip).await))
        } else { (None, None) };
        AppConfigRead {
            screen_id: screen.id.clone(),
            read_at: now(),
            config,
            capabilities,
            home,
            owner,
            kiosk,
            message,
        }
}

pub async fn read_home<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str) -> AppHomeRead {
    read_home_status(device,ip,super::home::query_for_owner_setup(device,ip).await).await
}

pub(super) async fn read_home_response<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,response:&Value)->AppHomeRead {
    read_home_status(device,ip,super::home::query_response(device,ip,response,false).await).await
}

async fn read_home_status<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,status:AppResult<super::home::HomeStatus>)->AppHomeRead {
    match status {
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

fn home_requires_prior_unlock(patch: &AppConfigPatch, kiosk: &super::kiosk::KioskStatus, home_active: bool, restart: bool) -> bool {
    patch.home_enabled == Some(true) && patch.kiosk_enabled != Some(false)
        && (!home_active || restart) && (kiosk.enabled || kiosk.active || kiosk.lock_task_mode != "none")
}

pub(super) async fn preflight_controls<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, patch: &AppConfigPatch, restart: bool) -> AppResult<()> {
    if patch.ensure_device_owner.is_none() && patch.home_enabled.is_none() && patch.kiosk_enabled.is_none() { return Ok(()) }
    let owner = super::device_owner::read(device, ip).await;
    super::device_owner::require(&owner, patch.ensure_device_owner == Some(true))?;
    if patch.home_enabled.is_some() {
        super::home::query_for_owner_setup(device, ip).await?;
    }
    if patch.kiosk_enabled.is_some() || patch.home_enabled == Some(true) {
        let kiosk = super::kiosk::query(device, ip).await?;
        if patch.home_enabled == Some(true) {
            if restart { super::home_activation::require_capability(device,ip).await?; }
            let runtime = super::home::query_runtime(device, ip, CancellationToken::new()).await?;
            if !runtime.verified() {
                let activation=super::home_activation::query(device,ip,CancellationToken::new()).await?;
                if activation.kind=="standard" { super::home_activation::require_capability(device,ip).await?; }
            }
            if home_requires_prior_unlock(patch, &kiosk, runtime.verified(), restart) {
                return Err(AppError::Conflict("锁定已开启或会随应用恢复，主屏需要重新激活；请先关闭锁定，或在本次明确选择关闭锁定后再设置主屏".into()));
            }
        }
    }
    Ok(())
}

async fn control_identity(
    state: &FormalAppState, plan: &ScreenPlan, screen: &ScreenAsset,
    original: &crate::domain::smart_screen::model::ScreenObservation,
    held: Option<&HeldScreenLeases>, device: &AdbDevice, cancel: &CancellationToken,
) -> AppResult<()> {
    if cancel.is_cancelled() { return Err(AppError::Cancelled) }
    authorize(state, plan, held).await?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let current = repo.asset(&plan.project_id, &screen.id).await?;
    let observed = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || super::value_updates::critical_draft(&repo.snapshot(&plan.project_id).await?, &current)
        || !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(AppError::Conflict("设备身份或资料已变化，未继续设备所有者或锁定操作".into()));
    }
    Ok(())
}

async fn ensure_owner(
    state: &FormalAppState, id: &str, plan: &ScreenPlan, screen: &ScreenAsset,
    original: &crate::domain::smart_screen::model::ScreenObservation, held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult, device: &AdbDevice, cancel: &CancellationToken,
) -> AppResult<()> {
    control_identity(state, plan, screen, original, held, device, cancel).await?;
    let before = super::device_owner::read(device, &screen.fields.ip).await;
    super::device_owner::require(&before, true)?;
    result.evidence["owner"] = json!({"component":super::device_owner::ADMIN,"before":before,"ensure":"pending","readback":"pending"});
    if before.state == "self" {
        result.evidence["owner"]["ensure"] = json!("unchanged");
        result.evidence["owner"]["readback"] = json!("succeeded");
        result.evidence["owner"]["after"] = json!(before);
        return checkpoint(state, id, plan, result).await;
    }
    result.device = ResultState::Unknown;
    result.evidence["phase"] = json!("config_owner_requested");
    result.evidence["owner"]["ensure"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    if cancel.is_cancelled() { return Err(AppError::Cancelled) }
    authorize(state, plan, held).await?;
    super::device_owner::ensure(device, &screen.fields.ip).await?;
    let after = super::device_owner::read(device, &screen.fields.ip).await;
    result.evidence["owner"]["after"] = json!(after);
    result.evidence["owner"]["readback"] = json!(if after.state == "self" { "succeeded" } else if after.state == "unknown" { "unknown" } else { "different" });
    if after.state != "self" {
        if after.state != "unknown" { result.device = ResultState::Failed; }
        checkpoint(state, id, plan, result).await?;
        return Err(AppError::Conflict("未确认小新成为设备所有者；未继续主屏或锁定操作，核实不会重发设置".into()));
    }
    result.evidence["owner"]["ensure"] = json!("applied");
    checkpoint(state, id, plan, result).await
}

async fn apply_kiosk(
    state: &FormalAppState, id: &str, plan: &ScreenPlan, screen: &ScreenAsset,
    original: &crate::domain::smart_screen::model::ScreenObservation, held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult, device: &AdbDevice, enabled: bool, home_enabled: Option<bool>, cancel: &CancellationToken,
) -> AppResult<()> {
    control_identity(state, plan, screen, original, held, device, cancel).await?;
    super::device_owner::require(&super::device_owner::read(device, &screen.fields.ip).await, false)?;
    if enabled && home_enabled == Some(true) {
        let runtime = super::home::wait_runtime_verified(device, &screen.fields.ip, Duration::from_secs(20), cancel.clone()).await?;
        result.evidence["home"]["runtimeAfter"] = json!(runtime);
        result.evidence["home"]["runtimeReadback"] = json!(if runtime.verified() { "succeeded" } else { "failed" });
        if !runtime.verified() {
            result.device = ResultState::Failed;
            checkpoint(state, id, plan, result).await?;
            return Err(AppError::Conflict("主屏激活尚未确认，未开启应用锁定".into()));
        }
    }
    let (before, matched) = super::kiosk::verify(device, &screen.fields.ip, enabled).await?;
    if !result.evidence["kiosk"].is_object() {
        result.evidence["kiosk"] = json!({"targetEnabled":enabled,"before":before,"save":"pending","apply":"pending","readback":"pending"});
    }
    if matched {
        result.evidence["kiosk"]["save"] = json!("unchanged");
        result.evidence["kiosk"]["apply"] = json!("unchanged");
        result.evidence["kiosk"]["readback"] = json!("succeeded");
        result.evidence["kiosk"]["after"] = json!(before);
        return checkpoint(state, id, plan, result).await;
    }
    result.device = ResultState::Unknown;
    if !before.configured || before.enabled != enabled {
        result.evidence["phase"] = json!("config_kiosk_save_requested");
        result.evidence["kiosk"]["save"] = json!("unknown");
        checkpoint(state, id, plan, result).await?;
        authorize(state, plan, held).await?;
        if cancel.is_cancelled() { return Err(AppError::Cancelled) }
        result.evidence["kiosk"]["save"] = json!(super::kiosk::save(device, &screen.fields.ip, enabled).await?);
        checkpoint(state, id, plan, result).await?;
    } else { result.evidence["kiosk"]["save"] = json!("unchanged"); }
    let current = super::kiosk::query(device, &screen.fields.ip).await?;
    if current.enabled != enabled || !current.configured {
        return Err(AppError::Conflict("锁定目标已被其他操作改变，未继续应用，请重新读取".into()));
    }
    if enabled && !current.activity_ready {
        authorize(state, plan, held).await?;
        if cancel.is_cancelled() { return Err(AppError::Cancelled) }
        result.evidence["kiosk"]["wakeRequested"] = json!(true);
        result.evidence["phase"] = json!("config_kiosk_wake_requested");
        checkpoint(state, id, plan, result).await?;
        super::kiosk::wake(device, &screen.fields.ip, cancel.clone()).await?;
        authorize(state, plan, held).await?;
        if cancel.is_cancelled() { return Err(AppError::Cancelled) }
        result.evidence["kiosk"]["startRequested"] = json!(true);
        result.evidence["phase"] = json!("config_kiosk_start_requested");
        checkpoint(state, id, plan, result).await?;
        device_maintenance::start_application(device, &screen.fields.ip).await?;
        if !device_maintenance::application_started_with_limit(device, &screen.fields.ip, "", Duration::from_secs(15), Duration::from_millis(250)).await? {
            return Err(AppError::Conflict("锁定目标已保存，但小新主界面尚未确认；请核实结果，不重复启动".into()));
        }
    }
    authorize(state, plan, held).await?;
    if cancel.is_cancelled() { return Err(AppError::Cancelled) }
    result.evidence["phase"] = json!("config_kiosk_apply_requested");
    result.evidence["kiosk"]["apply"] = json!("unknown");
    checkpoint(state, id, plan, result).await?;
    result.evidence["kiosk"]["apply"] = json!(super::kiosk::apply(device, &screen.fields.ip, enabled).await?);
    checkpoint(state, id, plan, result).await?;
    let (after, matched) = super::kiosk::wait_verified(device, &screen.fields.ip, enabled).await?;
    result.evidence["kiosk"]["after"] = json!(after);
    result.evidence["kiosk"]["readback"] = json!(if matched { "succeeded" } else { "different" });
    if !matched {
        result.device = ResultState::Failed;
        checkpoint(state, id, plan, result).await?;
        return Err(AppError::Conflict("锁定设置已保存，但实际锁定状态未达到目标；未重复应用".into()));
    }
    checkpoint(state, id, plan, result).await
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
    if super::kiosk::query(device, &screen.fields.ip).await?.lock_task_mode != "none" {
        result.device = ResultState::Failed;
        return Err(AppError::Conflict("应用已锁定且主屏尚未激活，请先单独解锁；未发送 HOME 动作".into()));
    }
    let activation_plan=super::home_activation::query(device,&screen.fields.ip,cancel.clone()).await?;
    result.evidence["home"]["taskBefore"]=json!(activation_plan);
    if activation_plan.kind=="standard" {
        super::home_activation::require_capability(device,&screen.fields.ip).await?;
        // 显式 HOME 操作可唤醒屏，但不解除系统锁屏、不用普通启动来制造另一个任务。
        control_identity(state,plan,screen,original,held,device,cancel).await?;
        result.evidence["home"]["wakeRequested"]=json!(true);
        checkpoint(state,id,plan,result).await?;
        super::kiosk::wake(device,&screen.fields.ip,cancel.clone()).await?;
        let task_id=activation_plan.task_id.ok_or_else(||AppError::Conflict("小新普通任务编号未确认".into()))?;
        let fresh=super::home_activation::query(device,&screen.fields.ip,cancel.clone()).await?;
        if fresh!=activation_plan { return Err(AppError::Conflict("小新任务在准备前已变化，请重新读取；未退出任务".into())) }
        control_identity(state,plan,screen,original,held,device,cancel).await?;
        let request_id=uuid::Uuid::now_v7().to_string();
        result.device=ResultState::Unknown;
        result.evidence["phase"]=json!("config_home_prepare_requested");
        result.evidence["home"]["prepareTaskId"]=json!(task_id);
        result.evidence["home"]["prepareRequestId"]=json!(request_id);
        result.evidence["home"]["preparation"]=json!("unknown");
        result.evidence["home"]["taskRemovalReadback"]=json!("unknown");
        checkpoint(state,id,plan,result).await?;
        super::home_activation::prepare(device,&screen.fields.ip,task_id,&request_id).await?;
        result.evidence["home"]["preparation"]=json!("requested");
        checkpoint(state,id,plan,result).await?;
        let removed=super::home_activation::wait_removed(device,&screen.fields.ip,task_id,Duration::from_secs(15),cancel.clone()).await?;
        result.evidence["home"]["taskRemovalReadback"]=json!(if removed {"succeeded"} else {"different"});
        if !removed {
            result.device=ResultState::Failed;
            checkpoint(state,id,plan,result).await?;
            return Err(AppError::Conflict("尚未确认小新原普通任务退出，未发送系统 HOME；核实不会重放退出动作".into()));
        }
        result.evidence["home"]["preparation"]=json!("verified");
        checkpoint(state,id,plan,result).await?;
    } else {
        result.evidence["home"]["preparation"]=json!("not_required");
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
    let (capabilities, before) = read_device(&device, &screen.fields.ip, Some(patch)).await?;
    preflight_controls(&device, &screen.fields.ip, patch, patch.restart_required_with_capabilities(&before, &capabilities)).await?;
    let home_before = if patch.home_enabled.is_some() {
        Some(super::home::query_for_owner_setup(&device, &screen.fields.ip).await?)
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
    if let Ok(Some(schema)) = ConfigSchema::from_capabilities(&capabilities) {
        result.evidence["config"]["fieldDefinitions"] = json!(schema.selected_definitions(&patch.fields()));
    }
    if let Some(enabled) = patch.home_enabled {
        result.evidence["home"] = json!({"component":super::home::COMPONENT,"targetEnabled":enabled,"before":home_before,"save":"pending","readback":"pending","activation":if enabled { "pending" } else { "not_required" },"runtimeReadback":if enabled { "pending" } else { "not_required" },"runtimeBefore":runtime_before});
    }
    if let Some(enabled) = patch.kiosk_enabled {
        result.evidence["kiosk"] = json!({"targetEnabled":enabled,"before":super::kiosk::query(&device,&screen.fields.ip).await?,"save":"pending","apply":"pending","readback":"pending"});
    }
    if patch.ensure_device_owner == Some(true) {
        ensure_owner(state,id,plan,screen,original,held,result,&device,&cancel).await?;
    }
    // 明确关闭先完成解锁；开启必须留在配置、重启和 HOME 核验之后。
    if patch.kiosk_enabled == Some(false) {
        apply_kiosk(state,id,plan,screen,original,held,result,&device,false,patch.home_enabled,&cancel).await?;
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
            if patch.kiosk_enabled == Some(true) {
                apply_kiosk(state,id,plan,screen,original,held,result,&device,true,patch.home_enabled,&cancel).await?;
            }
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
        if patch.kiosk_enabled == Some(true) {
            apply_kiosk(state,id,plan,screen,original,held,result,&device,true,patch.home_enabled,&cancel).await?;
        }
        if patch.ensure_device_owner.is_some() || patch.kiosk_enabled.is_some() {
            return verify(state, id, plan, data, screen, result).await;
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
    let restart = patch.restart_required_with_capabilities(&before, &capabilities);
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
    let status = validated_save_status(&saved, patch)?;
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
            result.message = "配置已保存，小新返回的重启要求与检查时不同；请单独重启小新使配置生效".into();
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
    // 在任何后续主屏或开启锁定动作前，先独立核实并保存普通配置及必要重启事实。
    // 后续系统策略失败不得让已经确认的新进程/主界面仍停留在 unknown。
    if patch.home_enabled.is_some() || patch.kiosk_enabled.is_some() {
        progress
            .report(
                "等待小新配置就绪",
                65,
                "正在独立核实配置保存和必要重启，再继续主屏或锁定设置",
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
            ready.unwrap_or_else(|| Err(AppError::Conflict("小新配置回读尚未就绪".into())))?;
        result.evidence["config"]["after"] = after.clone();
        result.evidence["config"]["readAt"] = json!(now());
        let checked = verify_configuration_readback(&device, &screen.fields.ip, patch, &after, result,
            Duration::from_secs(15), Duration::from_millis(250)).await;
        checkpoint(state, id, plan, result).await?;
        checked?;
        if result.device != ResultState::Succeeded { return Ok(()) }
    }
    apply_home(state,id,plan,screen,original,held,result,&device,patch.home_enabled,&cancel).await?;
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
    if patch.kiosk_enabled == Some(true) {
        apply_kiosk(state,id,plan,screen,original,held,result,&device,true,patch.home_enabled,&cancel).await?;
    }
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
        result.message = format!("配置结果已记录；设备策略或运行状态尚待核实：{error}");
        checkpoint(state, id, plan, result).await?;
        return Err(error);
    }
    merge_home_verification(result);
    merge_control_verification(result);
    checkpoint(state, id, plan, result).await
}

fn merge_control_verification(result: &mut ScreenTargetResult) {
    for (key, label) in [("owner", "设备所有者"), ("kiosk", "应用锁定")] {
        let Some(evidence) = result.evidence.get(key) else { continue };
        if evidence["readback"] != "succeeded" {
            result.device = if evidence["readback"] == "unknown" { ResultState::Unknown } else { ResultState::Failed };
            result.message = format!("已有配置结果保留；{label}尚未达到本次目标，请查看分段结果；核实不会重放动作");
            return;
        }
        if matches!(result.device, ResultState::Succeeded | ResultState::Skipped) {
            result.device = ResultState::Succeeded;
            result.message.push_str(&format!("；{label}已独立回读确认"));
        }
    }
}

async fn verify_controls<P: DeviceCommandPort>(device: &AdbDevice<P>, ip: &str, patch: &AppConfigPatch, result: &mut ScreenTargetResult) -> AppResult<()> {
    let mut last_error = None;
    if patch.ensure_device_owner == Some(true) {
        let owner = super::device_owner::read(device, ip).await;
        result.evidence["owner"]["component"] = json!(super::device_owner::ADMIN);
        result.evidence["owner"]["after"] = json!(owner);
        result.evidence["owner"]["readback"] = json!(match owner.state.as_str() { "self" => "succeeded", "unknown" => "unknown", _ => "different" });
        if owner.state == "self" && result.evidence["owner"]["ensure"] == "unknown" {
            result.evidence["owner"]["ensure"] = json!("applied");
        }
        if owner.state == "unknown" { last_error = Some(AppError::Conflict("设备所有者仍待核实".into())); }
    }
    if let Some(enabled) = patch.kiosk_enabled {
        result.evidence["kiosk"]["targetEnabled"] = json!(enabled);
        result.evidence["kiosk"]["readback"] = json!("unknown");
        match super::kiosk::wait_verified(device, ip, enabled).await {
            Ok((after, matched)) => {
                result.evidence["kiosk"]["after"] = json!(after);
                result.evidence["kiosk"]["readback"] = json!(if matched { "succeeded" } else { "different" });
                if after.configured && after.enabled == enabled && result.evidence["kiosk"]["save"] == "unknown" {
                    result.evidence["kiosk"]["save"] = json!("verified");
                }
                if matched && matches!(result.evidence["kiosk"]["apply"].as_str(), Some("pending" | "unknown")) {
                    result.evidence["kiosk"]["apply"] = json!("verified");
                }
            }
            Err(error) => last_error = Some(error),
        }
    }
    last_error.map_or(Ok(()), Err)
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
    } else if enabled && home.get("prepareTaskId").is_some() && home["taskRemovalReadback"]!="succeeded" {
        result.device=if home["taskRemovalReadback"]=="unknown" {ResultState::Unknown} else {ResultState::Failed};
        result.message="小新原普通任务退出尚未确认，已有配置和主屏核验事实保留；核实不会重复退出或激活".into();
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

fn record_configuration_readback(
    patch: &AppConfigPatch,
    after: &Value,
    result: &mut ScreenTargetResult,
) -> bool {
    if matches!(
        result.evidence["config"]["save"].as_str(),
        Some("partial" | "failed")
    ) {
        result.device = ResultState::Failed;
        result.evidence["config"]["readback"] = json!(if patch.matches(after) {
            "succeeded"
        } else {
            "different"
        });
        result.message =
            "已重新读取当前配置；原保存未全部成功，未自动重启，请检查未保存字段".into();
        return false;
    }
    if !patch.matches(after) {
        result.device = ResultState::Failed;
        result.evidence["config"]["readback"] = json!("different");
        result.message = "回读值与本次设置不同，请检查屏端当前配置；未重复保存或重启".into();
        return false;
    }
    result.evidence["config"]["readback"] = json!("succeeded");
    if result.evidence["config"]["save"] == "unknown" {
        result.evidence["config"]["save"] = json!("verified");
    }
    true
}

async fn verify_configuration_readback<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    patch: &AppConfigPatch,
    after: &Value,
    result: &mut ScreenTargetResult,
    limit: Duration,
    interval: Duration,
) -> AppResult<()> {
    if !record_configuration_readback(patch, after, result) {
        return Ok(());
    }
    let needs_restart = result.evidence["config"]["restartRequired"] == true;
    if needs_restart {
        let before = result.evidence["config"]["beforePid"].as_str().unwrap_or("");
        // 冷 Provider 可以独立回读，不能把它的新 PID 当作主应用已启动。
        // 在本次核实内有限等待，避免返回 Failed 后结束外层读取重试。
        let started = if restart_was_requested(&result.evidence) {
            match device_maintenance::application_started_with_limit(
                device, ip, before, limit, interval,
            ).await {
                Ok(started) => started,
                Err(error) => {
                    result.device = ResultState::Unknown;
                    result.evidence["config"]["restart"] = json!("unknown");
                    result.message = format!("配置已保存并回读一致，应用启动状态仍待核实：{error}");
                    return Err(AppError::Conflict(result.message.clone()));
                }
            }
        } else { false };
        if !started {
            result.device = ResultState::Failed;
            result.evidence["config"]["restart"] = json!("not_confirmed");
            result.message = "配置已保存并回读一致，但未确认小新新进程和主界面；请检查屏端状态，必要时单独重启小新，无需重新保存".into();
            return Ok(());
        }
        result.evidence["config"]["restart"] = json!("succeeded");
    }
    result.device = ResultState::Succeeded;
    result.evidence["phase"] = json!("config_verified");
    result.message = if needs_restart {
        "配置已保存并回读一致，已确认小新新进程和主界面"
    } else {
        "配置已保存并回读一致，本次无需重启"
    }.into();
    Ok(())
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
    let mut home_error = verify_controls(&device, &screen.fields.ip, patch, result).await.err();
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
            if let Err(error)=super::home_activation::verify_preparation(&device,&screen.fields.ip,&mut result.evidence["home"]).await {
                if home_error.is_none() {home_error=Some(error)}
            }
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
        result.message = "本次普通配置无需修改或重启".into();
        return finish_verification(state, id, plan, result, home_error).await;
    }
    let (_, after) = read_device(&device, &screen.fields.ip, Some(patch)).await?;
    result.evidence["config"]["after"] = after.clone();
    result.evidence["config"]["readAt"] = json!(now());
    result.observation = Some(observed);
    if let Err(error) = verify_configuration_readback(
        &device, &screen.fields.ip, patch, &after, result,
        Duration::from_secs(15), Duration::from_millis(250),
    ).await {
        checkpoint(state, id, plan, result).await?;
        return Err(error);
    }
    finish_verification(state, id, plan, result, home_error).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn home_gate_accounts_for_persisted_auto_lock_and_explicit_unlock_without_blocking_plain_restart() {
        let commands = super::super::kiosk::tests::Commands::new(true);
        let mut kiosk: super::super::kiosk::KioskStatus = serde_json::from_value(commands.status).unwrap();
        kiosk.active = false; kiosk.activity_ready = false; kiosk.lock_task_mode = "none".into();
        let mut patch = AppConfigPatch { home_enabled: Some(true), ..Default::default() };
        assert!(home_requires_prior_unlock(&patch, &kiosk, false, false));
        assert!(home_requires_prior_unlock(&patch, &kiosk, true, true));
        assert!(!home_requires_prior_unlock(&patch, &kiosk, true, false));
        patch.kiosk_enabled = Some(false);
        assert!(!home_requires_prior_unlock(&patch, &kiosk, false, true));
        patch.kiosk_enabled = None; patch.home_enabled = None;
        assert!(!home_requires_prior_unlock(&patch, &kiosk, false, true));
        patch.home_enabled = Some(false);
        assert!(!home_requires_prior_unlock(&patch, &kiosk, false, true));
    }

    #[tokio::test]
    async fn unknown_owner_and_kiosk_results_are_recovered_only_by_reading() {
        let mut commands = super::super::kiosk::tests::Commands::new(true); commands.read_only = true;
        let patch = AppConfigPatch { ensure_device_owner: Some(true), kiosk_enabled: Some(true), ..Default::default() };
        let mut result = ScreenTargetResult { device: ResultState::Unknown,
            evidence: json!({"config":{"save":"saved","readback":"succeeded","restart":"succeeded"},
                "owner":{"ensure":"unknown"},"kiosk":{"save":"unknown","apply":"unknown"}}), ..Default::default() };
        verify_controls(&commands.device(),"192.0.2.1",&patch,&mut result).await.unwrap();
        assert_eq!(result.evidence["owner"]["ensure"],"applied");
        assert_eq!(result.evidence["owner"]["readback"],"succeeded");
        assert_eq!(result.evidence["kiosk"]["save"],"verified");
        assert_eq!(result.evidence["kiosk"]["apply"],"verified");
        assert_eq!(result.evidence["kiosk"]["readback"],"succeeded");
        assert_eq!(result.evidence["config"]["save"],"saved");
        assert_eq!(*commands.calls.lock().unwrap(),vec!["owner-read","readKiosk","runtime-read"]);
    }

    #[tokio::test]
    async fn unreadable_lock_state_stays_unknown_without_replaying_saved_actions() {
        let mut commands = super::super::kiosk::tests::Commands::new(true);
        commands.read_only = true; commands.runtime.clear();
        let patch = AppConfigPatch { kiosk_enabled: Some(true), ..Default::default() };
        let mut result = ScreenTargetResult { device: ResultState::Succeeded,
            evidence: json!({"config":{"save":"saved","readback":"succeeded","restart":"not_required"},
                "kiosk":{"save":"saved","apply":"unknown"}}), ..Default::default() };
        assert!(verify_controls(&commands.device(),"192.0.2.1",&patch,&mut result).await.is_err());
        merge_control_verification(&mut result);
        assert_eq!(result.device,ResultState::Unknown);
        assert_eq!(result.evidence["config"]["save"],"saved");
        assert_eq!(result.evidence["config"]["restart"],"not_required");
        assert_eq!(result.evidence["kiosk"]["save"],"saved");
        assert_eq!(result.evidence["kiosk"]["apply"],"unknown");
    }

    #[test]
    fn kiosk_failure_does_not_overwrite_configuration_or_home_facts() {
        let mut result = ScreenTargetResult { device: ResultState::Succeeded,
            evidence: json!({"config":{"save":"saved","readback":"succeeded","restart":"succeeded"},
                "home":{"readback":"succeeded"},"kiosk":{"save":"saved","apply":"pending","readback":"different"}}), ..Default::default() };
        merge_control_verification(&mut result);
        assert_eq!(result.device,ResultState::Failed);
        assert_eq!(result.evidence["config"]["restart"],"succeeded");
        assert_eq!(result.evidence["home"]["readback"],"succeeded");
        assert_eq!(result.evidence["kiosk"]["save"],"saved");
    }

    fn configuration_fixture() -> Value {
        let environment = json!({
            "otaUrl":"https://example.test", "wsUrl":null, "h5Url":null,
            "h5ReadyCheckEnabled":true, "otaWsUrl":null, "otaH5Url":null,
            "effectiveWsUrl":null, "effectiveH5Url":null, "wsSource":"none", "h5Source":"none"
        });
        json!({"customDeviceName":"测试小新","privateValue":"must-not-project","environments":{
            "current":"prod", "test":environment, "pre":environment, "prod":environment
        }})
    }

    struct ConfigurationApplication {
        capabilities: Value,
        config: Value,
        requests: Arc<Mutex<Vec<Value>>>,
    }
    impl DeviceCommandPort for ConfigurationApplication {
        async fn execute(
            &self,
            _: &std::path::Path,
            args: Vec<String>,
            _: Duration,
            _: CancellationToken,
        ) -> AppResult<super::super::device::ProcessOutput> {
            let stdout = if args.first().is_some_and(|value| value == "connect") {
                "connected".into()
            } else if args.last().is_some_and(|value| value == "get-state") {
                "device".into()
            } else {
                let method = args.windows(2).find(|pair| pair[0] == "--method").expect("only configuration calls")[1].as_str();
                let response = match method {
                    "getCapabilities" => json!({"status":"ok","data":self.capabilities}),
                    "readConfig" => json!({"status":"ok","data":self.config}),
                    "saveConfig" => {
                        let encoded = args.iter().find_map(|value| value.strip_prefix("payload:s:")).unwrap();
                        let request: Value = serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
                        let fields = request["set"].as_object().unwrap().keys().cloned().chain(request["clear"].as_array().unwrap().iter().map(|v|v.as_str().unwrap().into())).collect::<Vec<_>>();
                        self.requests.lock().unwrap().push(request);
                        json!({"status":"saved","savedFields":fields,"failedFields":[],"restartRequired":true})
                    }
                    _ => panic!("unexpected method: {method}"),
                };
                format!("Result: Bundle[{{result={}}}]", STANDARD.encode(serde_json::to_vec(&response).unwrap()))
            };
            Ok(super::super::device::ProcessOutput { success: true, stdout, stderr: String::new() })
        }
    }

    fn configuration_device(advertised: bool, config: Value) -> AdbDevice<ConfigurationApplication> {
        let mut supported = vec![json!("customDeviceName")];
        if advertised { supported.push(json!("autoStartVoiceEngine")); }
        AdbDevice::with_commands(AndroidTools { adb: "mock-adb-never-executed".into() }, ConfigurationApplication {
            capabilities: json!({"protocolVersion":1,"packageName":"chat.xiaoxin.app","ready":true,"supportedFields":supported}),
            config, requests: Arc::new(Mutex::new(Vec::new()))
        })
    }

    fn numeric_schema_fixture() -> (Value,Value) {
        let mut config = configuration_fixture();
        config["autoStartVoiceEngine"] = json!(false);
        config["audioConfig"] = json!({"keywordsScore":2.0,"keywordsThreshold":0.2,"vadThreshold":0.8,"minSilenceDuration":0.6,
            "minSpeechDuration":0.3,"vadWindowSize":512,"vadPreCacheDurationMs":800,"micSampleRate":11025,"privateValue":"must-not-project"});
        let mut fields = vec![
            json!({"key":"customDeviceName","type":"string","label":"设备名","group":"general","order":0,"writable":true,"clearable":true,"defaultValue":null,"maxLength":128,"restartPolicy":"none"}),
            json!({"key":"autoStartVoiceEngine","type":"boolean","label":"语音引擎随应用启动","group":"voice","order":0,"writable":true,"clearable":false,"defaultValue":false,"restartPolicy":"appRestart"}),
        ];
        for (index,(key,kind,default,min,max)) in [
            ("keywordsScore","number",json!(2.0),1.0,2.0),("keywordsThreshold","number",json!(0.2),0.2,0.8),
            ("vadThreshold","number",json!(0.8),0.3,0.9),("minSilenceDuration","number",json!(0.6),0.3,2.0),
            ("minSpeechDuration","number",json!(0.3),0.1,0.8),("vadWindowSize","integer",json!(512),256.0,1024.0),
            ("vadPreCacheDurationMs","integer",json!(800),200.0,800.0),("micSampleRate","integer",json!(11025),8000.0,48000.0),
        ].into_iter().enumerate() {
            fields.push(json!({"key":format!("audioConfig.{key}"),"type":kind,"label":key,"group":"voice","order":index+1,"writable":true,"clearable":false,"defaultValue":default,"min":min,"max":max,"step":0.1,"restartPolicy":"appRestart"}));
        }
        let cap = json!({"packageName":"chat.xiaoxin.app","protocolVersion":1,"ready":true,"privateCapability":"must-not-project",
            "supportedFields":fields.iter().map(|field|field["key"].clone()).collect::<Vec<_>>(),"configSchema":{"version":1,"fields":fields}});
        (cap,config)
    }

    fn schema_device(cap:Value,config:Value) -> AdbDevice<ConfigurationApplication> {
        AdbDevice::with_commands(AndroidTools { adb:"mock-adb-never-executed".into() },ConfigurationApplication { capabilities:cap,config,requests:Arc::new(Mutex::new(Vec::new())) })
    }

    #[tokio::test]
    async fn actual_kotlin_store_envelopes_are_compatible_for_default_and_adaptive_small_screens() {
        for (cap_text,read_text,rate) in [
            (include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../test/fixtures/app-config-schema-v1/schema-capabilities-default.json")),include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../test/fixtures/app-config-schema-v1/schema-read-default.json")),16000),
            (include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../test/fixtures/app-config-schema-v1/schema-capabilities-small.json")),include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../test/fixtures/app-config-schema-v1/schema-read-small.json")),11025),
        ] {
            let cap=accepted(serde_json::from_str(cap_text).unwrap()).unwrap();
            let read=accepted(serde_json::from_str(read_text).unwrap()).unwrap();
            let device=schema_device(cap,read);
            let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.minSilenceDuration":0.62}})).unwrap();
            let (cap,read)=read_device(&device,"192.0.2.1",Some(&patch)).await.unwrap();
            assert_eq!(cap["configSchema"]["fields"].as_array().unwrap().len(),23);
            assert_eq!(read["audioConfig"].as_object().unwrap().len(),8);
            assert_eq!(read["audioConfig"]["micSampleRate"],rate);
            assert!(patch.restart_required_with_capabilities(&read,&cap));
            for field in ["authCode","deviceId","clientId","extendParams","storage","defaults"] { assert!(read.get(field).is_none()); }
            // 数学同值不触发保存/重启，即使Kotlin默认分数被编码为2.0而客户端发2。
            let same:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.keywordsScore":2}})).unwrap();
            assert!(same.matches(&read));
            assert!(!same.restart_required_with_capabilities(&read,&cap));
        }
    }

    #[tokio::test]
    async fn schema_read_projects_eight_audio_leaves_without_private_values_or_invented_defaults() {
        let (cap,config)=numeric_schema_fixture();
        let device=schema_device(cap,config);
        let (cap,safe)=read_device(&device,"192.0.2.1",None).await.unwrap();
        assert!(cap.get("privateCapability").is_none());
        assert_eq!(safe["audioConfig"].as_object().unwrap().len(),8);
        assert!(safe["audioConfig"].get("privateValue").is_none());
        assert_eq!(safe["audioConfig"]["micSampleRate"],11025);
        let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.minSilenceDuration":0.62}})).unwrap();
        assert!(read_device(&device,"192.0.2.1",Some(&patch)).await.is_ok());
    }

    #[tokio::test]
    async fn missing_or_malformed_audio_value_is_unknown_instead_of_default_and_blocks_that_patch() {
        let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.vadThreshold":0.7}})).unwrap();
        let legacy:AppConfigPatch=serde_json::from_value(json!({"set":{"customDeviceName":"仍可改名"}})).unwrap();
        for value in [None,Some(Value::Null),Some(json!("0.8")),Some(json!(true))] {
            let (cap,mut config)=numeric_schema_fixture();
            if let Some(value)=value {config["audioConfig"]["vadThreshold"]=value;} else {config["audioConfig"].as_object_mut().unwrap().remove("vadThreshold");}
            let device=schema_device(cap,config);
            let (_,safe)=read_device(&device,"192.0.2.1",None).await.unwrap();
            assert!(safe["audioConfig"].get("vadThreshold").is_none());
            assert!(read_device(&device,"192.0.2.1",Some(&patch)).await.is_err());
            assert!(read_device(&device,"192.0.2.1",Some(&legacy)).await.is_ok());
        }
    }

    #[tokio::test]
    async fn historical_audio_value_outside_new_range_is_preserved_and_can_be_corrected() {
        let (cap,mut config)=numeric_schema_fixture();
        config["audioConfig"]["vadThreshold"]=json!(1.8);
        let device=schema_device(cap,config);
        let (_,safe)=read_device(&device,"192.0.2.1",None).await.unwrap();
        assert_eq!(safe["audioConfig"]["vadThreshold"],1.8);
        let valid:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.vadThreshold":0.7}})).unwrap();
        let invalid:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.vadThreshold":1.2}})).unwrap();
        assert!(read_device(&device,"192.0.2.1",Some(&valid)).await.is_ok());
        assert!(read_device(&device,"192.0.2.1",Some(&invalid)).await.is_err());
    }

    #[tokio::test]
    async fn old_apps_and_bad_new_schemas_keep_legacy_edits_but_cannot_authorize_audio() {
        let audio:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.keywordsScore":1.5}})).unwrap();
        let name:AppConfigPatch=serde_json::from_value(json!({"set":{"customDeviceName":"兼容名称"}})).unwrap();
        let (cap,config)=numeric_schema_fixture();
        for variant in 0..4 {
            let mut candidate=cap.clone();
            match variant {
                0=>{candidate.as_object_mut().unwrap().remove("configSchema");},
                1=>{candidate["configSchema"]["version"]=json!(2);},
                2=>{candidate["configSchema"]["fields"][2]["min"]=json!(10);},
                _=>{candidate["configSchema"]["fields"][2]["key"]=json!("authCode");},
            }
            let device=schema_device(candidate,config.clone());
            let (read_cap,safe)=read_device(&device,"192.0.2.1",None).await.unwrap();
            assert!(safe.get("audioConfig").is_none());
            assert_eq!(read_cap.get("configSchemaError").is_some(),variant!=0);
            assert!(read_device(&device,"192.0.2.1",Some(&name)).await.is_ok());
            assert!(read_device(&device,"192.0.2.1",Some(&audio)).await.is_err());
        }
    }

    struct MutableSchemaApplication {
        cap:Arc<Mutex<Value>>,config:Value,requests:Arc<Mutex<Vec<Value>>>,
    }
    impl DeviceCommandPort for MutableSchemaApplication {
        async fn execute(&self,path:&std::path::Path,args:Vec<String>,timeout:Duration,cancel:CancellationToken)->AppResult<super::super::device::ProcessOutput>{
            let capabilities=self.cap.lock().unwrap().clone();
            ConfigurationApplication{capabilities,config:self.config.clone(),requests:self.requests.clone()}.execute(path,args,timeout,cancel).await
        }
    }

    #[tokio::test]
    async fn fresh_device_definition_is_rechecked_before_write_not_trusted_from_frontend_snapshot() {
        let (cap,config)=numeric_schema_fixture();
        let cap=Arc::new(Mutex::new(cap));let requests=Arc::new(Mutex::new(Vec::new()));
        let device=AdbDevice::with_commands(AndroidTools{adb:"mock-adb-never-executed".into()},MutableSchemaApplication{cap:cap.clone(),config,requests:requests.clone()});
        let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.minSilenceDuration":0.8}})).unwrap();
        assert!(read_device(&device,"192.0.2.1",Some(&patch)).await.is_ok());
        // 打开表单后设备能力改变；再次预检/执行必须重新拿真实定义。
        cap.lock().unwrap()["configSchema"]["fields"][5]["max"]=json!(0.7);
        assert!(read_device(&device,"192.0.2.1",Some(&patch)).await.is_err());
        assert!(requests.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn audio_leaf_keeps_existing_set_clear_protocol_without_sending_root_object_or_schema() {
        let (cap,config)=numeric_schema_fixture();let requests=Arc::new(Mutex::new(Vec::new()));
        let device=AdbDevice::with_commands(AndroidTools{adb:"mock-adb-never-executed".into()},ConfigurationApplication{capabilities:cap,config,requests:requests.clone()});
        let patch:AppConfigPatch=serde_json::from_value(json!({"set":{"audioConfig.minSilenceDuration":0.62}})).unwrap();
        read_device(&device,"192.0.2.1",Some(&patch)).await.unwrap();
        let request=json!({"protocolVersion":1,"requestId":"voice-leaf-fixture","set":patch.set,"clear":patch.clear});
        let response=call(&device,"192.0.2.1","saveConfig",Some(&request)).await.unwrap();
        assert_eq!(validated_save_status(&response,&patch).unwrap(),"saved");
        let recorded=requests.lock().unwrap();
        assert_eq!(recorded.as_slice(),&[request]);
        assert_eq!(recorded[0]["set"].as_object().unwrap().len(),1);
        assert!(recorded[0].get("configSchema").is_none());
        assert!(recorded[0]["set"].get("audioConfig").is_none());
    }

    #[test]
    fn voice_drafts_accept_old_absence_and_safe_partial_values_but_never_authorize_arbitrary_json() {
        assert!(validate_voice_draft(None).is_ok());
        assert!(validate_voice_draft(Some(&json!({"autoStartVoiceEngine":{"mode":"set","value":false},"audioConfig.vadThreshold":{"mode":"set","value":0.62},"audioConfig.keywordsScore":{"mode":"keep"}}))).is_ok());
        for value in [json!([]),json!({"authCode":{"mode":"set","value":"private"}}),json!({"audioConfig":{"mode":"set","value":{}}}),json!({"audioConfig.__proto__":{"mode":"set","value":1}}),json!({"audioConfig.vadThreshold":{"mode":"set","value":null}}),json!({"audioConfig.vadThreshold":{"mode":"set","value":{"x":1}}})] {
            assert!(validate_voice_draft(Some(&value)).is_err());
        }
    }

    #[tokio::test]
    async fn optional_voice_flag_requires_advertised_capability_and_boolean_without_breaking_old_fields() {
        let voice: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false}})).unwrap();
        let name: AppConfigPatch = serde_json::from_value(json!({"set":{"customDeviceName":"新名称"}})).unwrap();
        for advertised in [false, true] {
            for value in [None, Some(Value::Null), Some(json!("false")), Some(json!(0)), Some(json!([])), Some(json!({})), Some(json!(true)), Some(json!(false))] {
                let mut config = configuration_fixture();
                if let Some(value) = &value { config["autoStartVoiceEngine"] = value.clone(); }
                let device = configuration_device(advertised, config);
                let valid = advertised && value.as_ref().is_some_and(Value::is_boolean);
                let (_, safe) = read_device(&device, "192.0.2.1", None).await.unwrap();
                assert_eq!(safe.get("autoStartVoiceEngine"), value.as_ref().filter(|_| valid));
                assert!(safe.get("privateValue").is_none());
                assert_eq!(safe["customDeviceName"], "测试小新");
                assert!(read_device(&device, "192.0.2.1", Some(&name)).await.is_ok());
                assert_eq!(read_device(&device, "192.0.2.1", Some(&voice)).await.is_ok(), valid);
            }
        }
    }

    #[tokio::test]
    async fn voice_false_uses_existing_protocol_set_payload_and_validated_save_receipt() {
        let patch: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false}})).unwrap();
        patch.validate().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let device = AdbDevice::with_commands(AndroidTools { adb: "mock-adb-never-executed".into() }, ConfigurationApplication {
            capabilities: json!({}), config: configuration_fixture(), requests: requests.clone()
        });
        let request = json!({"protocolVersion":1,"requestId":"isolated-voice-request","set":patch.set,"clear":patch.clear});
        let receipt = call(&device, "192.0.2.1", "saveConfig", Some(&request)).await.unwrap();
        assert_eq!(validated_save_status(&receipt, &patch).unwrap(), "saved");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["protocolVersion"], 1);
        assert_eq!(requests[0]["set"]["autoStartVoiceEngine"], false);
        assert_eq!(requests[0]["clear"], json!([]));
    }

    #[test]
    fn voice_save_receipts_keep_strict_existing_validation() {
        let patch: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false}})).unwrap();
        let valid = json!({"status":"saved","savedFields":["autoStartVoiceEngine"],"failedFields":[],"restartRequired":true});
        assert_eq!(validated_save_status(&valid, &patch).unwrap(), "saved");
        for receipt in [
            json!({"status":"ok"}),
            json!({"status":"saved","savedFields":["autoStartVoiceEngine"],"restartRequired":true}),
            json!({"status":"saved","savedFields":["autoStartVoiceEngine"],"failedFields":[],"restartRequired":"false"}),
            json!({"status":"saved","savedFields":["customDeviceName"],"failedFields":[],"restartRequired":true}),
            json!({"status":"partial","savedFields":[],"failedFields":[1],"restartRequired":false}),
            json!({"status":"failed","code":"TIMEOUT"}),
        ] { assert!(validated_save_status(&receipt, &patch).is_err()); }
        assert_eq!(validated_save_status(&json!({"status":"partial","savedFields":[],"failedFields":["autoStartVoiceEngine"],"restartRequired":false}), &patch).unwrap(), "partial");
        assert_eq!(validated_save_status(&json!({"status":"unchanged","savedFields":[],"failedFields":[],"restartRequired":false}), &patch).unwrap(), "unchanged");
        let old: AppConfigPatch = serde_json::from_value(json!({"set":{"environments.prod.h5ReadyCheckEnabled":false}})).unwrap();
        assert!(validated_save_status(&json!({"status":"saved","savedFields":["environments.prod.h5ReadyCheckEnabled","h5ReadyRecord"],"failedFields":[],"restartRequired":true}), &old).is_ok());
    }

    #[test]
    fn voice_unknown_readback_requires_real_false_and_preserves_partial_failure_without_replaying_actions() {
        let patch: AppConfigPatch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false}})).unwrap();
        for after in [json!({}), json!({"autoStartVoiceEngine":null}), json!({"autoStartVoiceEngine":"false"}), json!({"autoStartVoiceEngine":true})] {
            let mut result = ScreenTargetResult { device: ResultState::Unknown, evidence: json!({"phase":"config_save_requested","config":{"save":"unknown","restartRequired":true}}), ..Default::default() };
            assert!(!record_configuration_readback(&patch, &after, &mut result));
            assert_eq!(result.device, ResultState::Failed);
            assert_eq!(result.evidence["config"]["save"], "unknown");
            assert_eq!(result.evidence["config"]["readback"], "different");
        }
        let mut result = ScreenTargetResult { device: ResultState::Unknown, evidence: json!({"phase":"config_save_requested","config":{"save":"unknown","restartRequired":true}}), ..Default::default() };
        assert!(record_configuration_readback(&patch, &json!({"autoStartVoiceEngine":false}), &mut result));
        assert_eq!(result.evidence["config"]["save"], "verified");
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.device, ResultState::Unknown);
        assert!(!restart_was_requested(&result.evidence));
        for saved in ["partial", "failed"] {
            result.evidence["config"]["save"] = json!(saved);
            result.evidence["config"]["restart"] = json!("not_started");
            assert!(!record_configuration_readback(&patch, &json!({"autoStartVoiceEngine":false}), &mut result));
            assert_eq!(result.device, ResultState::Failed);
            assert_eq!(result.evidence["config"]["save"], saved);
            assert_eq!(result.evidence["config"]["restart"], "not_started");
        }
    }

    #[derive(Clone)]
    struct ConfigurationRuntime {
        pid: &'static str,
        activities: Arc<Mutex<std::collections::VecDeque<&'static str>>>,
        fallback: &'static str,
        disconnected: bool,
        query_hangs: bool,
        calls: Arc<Mutex<Vec<&'static str>>>,
    }
    const PROVIDER_ONLY: &str = "ProviderRecord{abcd chat.xiaoxin.app/.WorkbenchConfigProvider}";
    const MAIN_RESUMED: &str = "mResumedActivity: ActivityRecord{abcd u0 chat.xiaoxin.app/.MainActivity t8}";
    impl ConfigurationRuntime {
        fn new(pid: &'static str, activities: Vec<&'static str>, fallback: &'static str) -> Self {
            Self { pid, activities: Arc::new(Mutex::new(activities.into())), fallback,
                disconnected: false, query_hangs: false, calls: Arc::default() }
        }
        fn device(&self) -> AdbDevice<Self> {
            AdbDevice::with_commands(AndroidTools { adb: "mock-adb-never-executed".into() }, self.clone())
        }
    }
    impl DeviceCommandPort for ConfigurationRuntime {
        async fn execute(
            &self, _: &std::path::Path, args: Vec<String>, _: Duration, _: CancellationToken,
        ) -> AppResult<super::super::device::ProcessOutput> {
            let stdout = if args.iter().any(|arg| arg.contains("__INX_XIAOXIN_PIDOF_EXIT__=")) {
                self.calls.lock().unwrap().push("pid");
                if self.disconnected { return Err(AppError::Conflict("模拟 ADB 连接断开".into())); }
                if self.query_hangs { std::future::pending::<()>().await; }
                format!("{}\n__INX_XIAOXIN_PIDOF_EXIT__=0\n", self.pid)
            } else if args.ends_with(&["dumpsys".into(), "activity".into(), "activities".into()]) {
                self.calls.lock().unwrap().push("activity");
                self.activities.lock().unwrap().pop_front().unwrap_or(self.fallback).into()
            } else {
                panic!("配置结果核实不得保存、停止或启动应用：{args:?}");
            };
            Ok(super::super::device::ProcessOutput { success: true, stdout, stderr: String::new() })
        }
    }
    fn pending_configuration_restart(sent: bool) -> ScreenTargetResult {
        ScreenTargetResult { device: ResultState::Unknown,
            evidence: json!({"phase":"config_saved","config":{"save":"saved","restartRequired":true,
                "restartRequested":sent,"beforePid":"2125","restart":"unknown","readback":"pending"}}),
            ..Default::default() }
    }
    async fn verify_runtime(runtime: &ConfigurationRuntime, result: &mut ScreenTargetResult) -> AppResult<()> {
        let patch = serde_json::from_value(json!({"set":{"autoStartVoiceEngine":false}})).unwrap();
        verify_configuration_readback(&runtime.device(), "192.0.2.1", &patch,
            &json!({"autoStartVoiceEngine":false}), result, Duration::from_millis(30), Duration::from_millis(1)).await
    }

    #[tokio::test]
    async fn configuration_restart_rejects_cold_provider_pid_and_preserves_saved_readback() {
        let runtime = ConfigurationRuntime::new("5447", vec![], PROVIDER_ONLY);
        let mut result = pending_configuration_restart(true);
        verify_runtime(&runtime, &mut result).await.unwrap();
        assert_eq!(result.device, ResultState::Failed);
        assert_eq!(result.evidence["config"]["save"], "saved");
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "not_confirmed");
        assert!(result.message.contains("主界面"));
        assert!(runtime.calls.lock().unwrap().iter().filter(|call| **call == "activity").count() > 1);
    }

    #[tokio::test]
    async fn configuration_restart_waits_for_main_activity_after_provider_becomes_ready() {
        let runtime = ConfigurationRuntime::new("5447", vec![PROVIDER_ONLY], MAIN_RESUMED);
        let mut result = pending_configuration_restart(true);
        verify_runtime(&runtime, &mut result).await.unwrap();
        assert_eq!(result.device, ResultState::Succeeded);
        assert_eq!(result.evidence["config"]["restart"], "succeeded");
        assert_eq!(*runtime.calls.lock().unwrap(), vec!["pid", "activity", "pid", "activity"]);
    }

    #[tokio::test]
    async fn later_home_failure_preserves_already_confirmed_configuration_restart() {
        let runtime = ConfigurationRuntime::new("5447", vec![PROVIDER_ONLY], MAIN_RESUMED);
        let mut result = pending_configuration_restart(true);
        // 与执行链相同：先完成普通配置/新PID/主界面核实，再进入主屏阶段。
        verify_runtime(&runtime,&mut result).await.unwrap();
        assert_eq!(result.device,ResultState::Succeeded);
        assert_eq!(result.evidence["config"]["restart"],"succeeded");
        result.device=ResultState::Unknown;
        result.evidence["phase"]=json!("config_home_activated");
        result.evidence["home"]=json!({"targetEnabled":true,"save":"unchanged","readback":"succeeded",
            "activation":"activated","runtimeReadback":"failed"});
        merge_home_verification(&mut result);
        assert_eq!(result.device,ResultState::Failed);
        for key in ["save","readback","restart"] {
            assert_eq!(result.evidence["config"][key],if key=="save" {"saved"} else {"succeeded"});
        }
        assert_eq!(*runtime.calls.lock().unwrap(),vec!["pid","activity","pid","activity"]);
    }

    #[tokio::test]
    async fn configuration_restart_rejects_old_pid_even_with_main_activity() {
        let runtime = ConfigurationRuntime::new("2125", vec![], MAIN_RESUMED);
        let mut result = pending_configuration_restart(true);
        verify_runtime(&runtime, &mut result).await.unwrap();
        assert_eq!(result.device, ResultState::Failed);
        assert_eq!(result.evidence["config"]["restart"], "not_confirmed");
        assert!(runtime.calls.lock().unwrap().iter().all(|call| *call == "pid"));
    }

    #[tokio::test]
    async fn configuration_restart_never_infers_an_unsent_restart_from_current_runtime() {
        let runtime = ConfigurationRuntime::new("5447", vec![], MAIN_RESUMED);
        let mut result = pending_configuration_restart(false);
        result.evidence["config"]["save"] = json!("unknown");
        verify_runtime(&runtime, &mut result).await.unwrap();
        assert_eq!(result.device, ResultState::Failed);
        assert_eq!(result.evidence["config"]["save"], "verified");
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "not_confirmed");
        assert!(runtime.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn configuration_restart_connection_failure_remains_unknown_after_matching_readback() {
        let mut runtime = ConfigurationRuntime::new("5447", vec![], MAIN_RESUMED);
        runtime.disconnected = true;
        let mut result = pending_configuration_restart(true);
        assert!(verify_runtime(&runtime, &mut result).await.unwrap_err().to_string().contains("连接断开"));
        assert_eq!(result.device, ResultState::Unknown);
        assert_eq!(result.evidence["config"]["save"], "saved");
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "unknown");
        assert_eq!(*runtime.calls.lock().unwrap(), vec!["pid"]);
    }

    #[tokio::test]
    async fn configuration_restart_query_timeout_preserves_unknown_instead_of_reporting_stopped() {
        let mut runtime = ConfigurationRuntime::new("5447", vec![], MAIN_RESUMED);
        runtime.query_hangs = true;
        let mut result = pending_configuration_restart(true);
        assert!(verify_runtime(&runtime, &mut result).await.unwrap_err().to_string().contains("超时"));
        assert_eq!(result.device, ResultState::Unknown);
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "unknown");
        assert_eq!(*runtime.calls.lock().unwrap(), vec!["pid"]);
    }

    #[tokio::test]
    async fn configuration_unknown_restart_recovers_with_readonly_runtime_and_matching_values() {
        let runtime = ConfigurationRuntime::new("5447", vec![], MAIN_RESUMED);
        let mut result = pending_configuration_restart(true);
        result.evidence["config"]["save"] = json!("unknown");
        verify_runtime(&runtime, &mut result).await.unwrap();
        assert_eq!(result.device, ResultState::Succeeded);
        assert_eq!(result.evidence["config"]["save"], "verified");
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "succeeded");
        assert_eq!(*runtime.calls.lock().unwrap(), vec!["pid", "activity"]);
    }

    #[tokio::test]
    async fn configuration_without_restart_accepts_cold_provider_readback_without_activity_query() {
        // 此模拟只实现配置 Provider；任何运行态、保存或启动调用都会失败。
        let device = configuration_device(false, configuration_fixture());
        let patch = serde_json::from_value(json!({"set":{"customDeviceName":"测试小新"}})).unwrap();
        let (_, after) = read_device(&device, "192.0.2.1", Some(&patch)).await.unwrap();
        let mut result = pending_configuration_restart(false);
        result.evidence["config"]["restartRequired"] = json!(false);
        result.evidence["config"]["restart"] = json!("not_required");
        verify_configuration_readback(&device, "192.0.2.1", &patch, &after, &mut result,
            Duration::from_millis(30), Duration::from_millis(1)).await.unwrap();
        assert_eq!(result.device, ResultState::Succeeded);
        assert_eq!(result.evidence["config"]["readback"], "succeeded");
        assert_eq!(result.evidence["config"]["restart"], "not_required");
    }

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
