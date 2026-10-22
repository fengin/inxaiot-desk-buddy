//! 语音字段定义的正式链路实机回归。默认不连接设备，只在显式门禁后使用新建隔离库。
//! 本文件不修改平台数据，不直接调用 saveConfig，不把配置地址写入恢复记录。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            app_config::{field_value, AppConfigPatch, AppConfigRead},
            app_config_schema::{values_equal, ConfigSchema, FieldType},
            model::{ResultState, ScreenFields, ScreenTargetResult},
            operation::ScreenOperationInput,
            rules::{normalize_mac, valid_mac},
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            app_config,
            device::{AdbDevice, AndroidTools},
            maintenance, task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, io::Write, path::{Path, PathBuf}, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const TARGETS: [(&str, &str); 3] = [
    ("192.168.3.106", "9c000c3c3643071205d"),
    ("192.168.3.74", "A47A4AK02040"),
    ("192.168.3.82", "A47A5CK00524"),
];
const AUDIO: [&str; 8] = [
    "audioConfig.micSampleRate", "audioConfig.vadThreshold",
    "audioConfig.minSilenceDuration", "audioConfig.minSpeechDuration",
    "audioConfig.vadWindowSize", "audioConfig.vadPreCacheDurationMs",
    "audioConfig.keywordsScore", "audioConfig.keywordsThreshold",
];
const AUTO: &str = "autoStartVoiceEngine";

struct Authorization { ip: String, serial: String, mode: String, case: Option<String> }

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition { Ok(()) } else { Err(message.into()) }
}

fn authorization(values: &BTreeMap<String, String>) -> TestResult<Authorization> {
    let get = |key: &str| values.get(key).map(String::as_str).unwrap_or("");
    require(get("INX_VOICE_SCHEMA_REAL") == "YES", "缺少实机门禁，未连接设备")?;
    let ip = get("INX_VOICE_SCHEMA_TARGET");
    let serial = get("INX_VOICE_SCHEMA_EXPECTED_SERIAL");
    require(TARGETS.contains(&(ip, serial)), "IP和稳定序列号不在已核实的三屏白名单，未连接设备")?;
    let mode = get("INX_VOICE_SCHEMA_MODE");
    require(["old-read", "voice-roundtrip", "restore"].contains(&mode), "未指定有效联测模式")?;
    require(mode == "old-read" || get("INX_VOICE_SCHEMA_ALLOW_WRITES") == "YES",
        "修改或恢复模式缺少独立写入门禁，未连接设备")?;
    let case = get("INX_VOICE_SCHEMA_CASE");
    require((mode == "restore") == !case.is_empty(), "只有恢复模式需要提供原用例目录名")?;
    if !case.is_empty() {
        require(uuid::Uuid::parse_str(case).is_ok_and(|id| id.to_string() == case),
            "恢复用例仅接受本轮证据根目录下的UUID目录名")?;
    }
    Ok(Authorization { ip: ip.into(), serial: serial.into(), mode: mode.into(),
        case: (!case.is_empty()).then(|| case.into()) })
}

fn authorization_from_env() -> TestResult<Authorization> {
    authorization(&["INX_VOICE_SCHEMA_REAL", "INX_VOICE_SCHEMA_TARGET",
        "INX_VOICE_SCHEMA_EXPECTED_SERIAL", "INX_VOICE_SCHEMA_MODE",
        "INX_VOICE_SCHEMA_ALLOW_WRITES", "INX_VOICE_SCHEMA_CASE"]
        .into_iter().filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value))).collect())
}

fn sha(value: &Value) -> String {
    format!("{:x}", Sha256::digest(serde_json::to_vec(value).expect("JSON可序列化")))
}

fn evidence_root() -> TestResult<PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("工程目录缺失")?.canonicalize()?;
    let base = workspace.join(".review-tools/voice-schema-real-20261022");
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    require(base.starts_with(&workspace), "证据目录不能离开工作台仓库")?;
    Ok(base)
}

fn persist(directory: &Path, record: &Value) -> TestResult<()> {
    let pending = directory.join("restore-state.pending.json");
    let mut file = std::fs::File::create(&pending)?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(pending, directory.join("restore-state.json"))?;
    Ok(())
}

fn phase(directory: &Path, record: &mut Value, name: &str) -> TestResult<()> {
    record["phase"] = json!(name);
    persist(directory, record)
}

// 正式任务会保存前后快照；所有带用户信息、查询串或片段的地址均先拒绝，避免鉴权信息落盘。
fn safe_for_formal_evidence(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.values().all(safe_for_formal_evidence),
        Value::Array(values) => values.iter().all(safe_for_formal_evidence),
        Value::String(text) if text.contains("://") => reqwest::Url::parse(text).is_ok_and(|url|
            url.username().is_empty() && url.password().is_none() && url.query().is_none() && url.fragment().is_none()),
        _ => true,
    }
}

fn input(id: &str) -> ScreenOperationInput {
    ScreenOperationInput { action: "app_config".into(), target_ids: vec![id.into()], application_id: None,
        apk: None, app_version: String::new(), abi: "universal".into(), reinstall: false,
        concurrency: 1, expected_targets: Default::default(), retry_of_operation_id: None }
}

fn patch(field: &str, value: Value) -> TestResult<AppConfigPatch> {
    let patch = AppConfigPatch { set: BTreeMap::from([(field.into(), value)]), ..Default::default() };
    patch.validate()?;
    Ok(patch)
}

async fn identity(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    device.connect(&auth.ip, CancellationToken::new()).await?;
    let actual = device.shell(&auth.ip, &["getprop", "ro.serialno"], CancellationToken::new()).await?;
    require(actual.trim() == auth.serial, "实机稳定序列号不匹配，未读取或写入配置")
}

async fn runtime(device: &AdbDevice, ip: &str) -> TestResult<(String, String)> {
    let pid = device.shell(ip, &["pidof", "chat.xiaoxin.app"], CancellationToken::new()).await?;
    require(!pid.trim().is_empty() && pid.bytes().all(|c| c.is_ascii_digit() || c.is_ascii_whitespace()),
        "应用进程标识不可用")?;
    let boot = device.shell(ip, &["cat", "/proc/sys/kernel/random/boot_id"], CancellationToken::new()).await?;
    require(uuid::Uuid::parse_str(boot.trim()).is_ok(), "系统启动标识不可用")?;
    Ok((pid.trim().into(), boot.trim().into()))
}

async fn read_one(state: &FormalAppState, project: &str, id: &str) -> TestResult<AppConfigRead> {
    let mut rows = app_config::read(state, project, &[id.into()]).await?;
    require(rows.len() == 1, "正式读取未返回唯一目标")?;
    let row = rows.pop().ok_or("正式读取结果缺失")?;
    require(row.config.is_some() && row.capabilities.as_ref().is_some_and(|cap| cap["ready"] == true),
        "配置未就绪或没有有效配置")?;
    Ok(row)
}

fn schema(row: &AppConfigRead) -> TestResult<ConfigSchema> {
    let schema = ConfigSchema::from_capabilities(row.capabilities.as_ref().ok_or("缺少能力")?)?
        .ok_or("未提供字段定义")?;
    require(schema.fields.len() == 23 && AUDIO.iter().all(|key| schema.field(key).is_some()),
        "字段定义未覆盖本轮23项和8个音频参数")?;
    let config = row.config.as_ref().ok_or("缺少配置")?;
    for key in AUDIO {
        let field = schema.field(key).ok_or("音频定义缺失")?;
        let current = field_value(config, key).ok_or("实际音频值缺失")?;
        require(field.writable && matches!(field.field_type, FieldType::Number | FieldType::Integer)
            && field.accepts(current), "原音频值不可安全恢复，未开始实机写入")?;
    }
    require(config[AUTO].is_boolean(), "原语音启动值缺失")?;
    Ok(schema)
}

fn projection(config: &Value, fields: &[String]) -> Value {
    Value::Object(fields.iter().map(|key| (key.clone(), field_value(config, key).cloned().unwrap_or(Value::Null))).collect())
}

fn equal_except(before: &Value, after: &Value, fields: &[String], except: &[String]) -> bool {
    fields.iter().filter(|key| !except.contains(key)).all(|key|
        values_equal(field_value(before, key).unwrap_or(&Value::Null), field_value(after, key).unwrap_or(&Value::Null)))
}

fn home(row: &AppConfigRead) -> Value {
    row.home.as_ref().and_then(|home| home.enabled).map(|value| json!(value)).unwrap_or(Value::Null)
}

fn candidate(schema: &ConfigSchema, config: &Value, key: &str) -> TestResult<Value> {
    let field = schema.field(key).ok_or("缺少目标定义")?;
    let current = field_value(config, key).and_then(Value::as_f64).ok_or("目标当前值不可读")?;
    let step = field.step.unwrap_or(0.1);
    for number in [current + step, current - step, field.default_value.as_f64().unwrap_or(current)] {
        // 限定本轮两项小数参数；避免二进制加法产生无意义尾数。
        let value = json!((number * 1_000_000.0).round() / 1_000_000.0);
        if field.accepts(&value) && !values_equal(&value, field_value(config, key).unwrap()) { return Ok(value); }
    }
    Err("找不到合法且不同的测试数值，未写入".into())
}

async fn settle(state: &FormalAppState, project: &str, id: &str) -> TestResult<()> {
    for task in state.task_repository.list_recent(project, 50).await? {
        if task.operation_type != "app_config" { continue; }
        require(!matches!(task.state, TaskState::Queued | TaskState::Running | TaskState::Cancelling),
            "原任务仍在执行，未并发修改或恢复")?;
        let result = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
        if task.state == TaskState::FinalizingFailed || state.task_repository.results_protected(&task.id).await?
            || result.targets.get(id).is_none_or(|row| matches!(row.device, ResultState::Pending | ResultState::Unknown)) {
            tasks::verify(state, project, &task.id).await?; // 仅核实原结果；绝不重放 saveConfig。
            let after = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
            require(!state.task_repository.results_protected(&task.id).await?
                && state.task_repository.get(&task.id).await?.state != TaskState::FinalizingFailed
                && after.targets.get(id).is_some_and(|row| !matches!(row.device, ResultState::Pending | ResultState::Unknown)),
                "执行结果仍不确定，保留恢复责任，未自动重放")?;
        }
    }
    Ok(())
}

async fn submit(state: &FormalAppState, project: &str, id: &str, patch: AppConfigPatch) -> TestResult<ScreenTargetResult> {
    settle(state, project, id).await?;
    let before = read_one(state, project, id).await?;
    require(safe_for_formal_evidence(before.config.as_ref().ok_or("正式预检前无配置")?),
        "配置含鉴权或不透明地址，未执行会保存快照的任务")?;
    let preview = maintenance::preflight_with_config(state, project, input(id), Some(BTreeMap::from([(id.into(), patch)]))).await?;
    require(preview.items.len() == 1 && preview.items[0].state == "ready", "正式配置预检未通过")?;
    let task = tasks::submit(state, project, &preview.id, input(id)).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let task = state.task_repository.get(&task).await?;
            if task.state.is_terminal() || task.state == TaskState::FinalizingFailed {
                break Ok::<(), inxaiot_desk_buddy_lib::core::error::AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }).await??;
    settle(state, project, id).await?;
    let result = task_data::read_results(state.local_store.pool(), project, &task).await?
        .targets.remove(id).ok_or("正式任务缺少目标结果")?;
    require(matches!(result.device, ResultState::Succeeded | ResultState::Skipped), "正式配置任务没有成功")?;
    require(result.business == ResultState::NotRequired && result.shared == ResultState::NotRequired,
        "本机未注册资产不应写入平台业务或共享库")?;
    Ok(result)
}

async fn checked_change(state: &FormalAppState, auth: &Authorization, project: &str, id: &str,
    fields: &[String], patch: AppConfigPatch, must_skip: bool) -> TestResult<Value> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    let before = read_one(state, project, id).await?;
    let original = before.config.as_ref().ok_or("阶段原配置缺失")?;
    let cap = before.capabilities.as_ref().ok_or("阶段能力缺失")?;
    let restart = patch.restart_required_with_capabilities(original, cap);
    let changed = !patch.matches(original);
    let original_runtime = runtime(&device, &auth.ip).await?;
    let result = submit(state, project, id, patch.clone()).await?;
    let after = read_one(state, project, id).await?;
    let config = after.config.as_ref().ok_or("阶段回读配置缺失")?;
    let after_runtime = runtime(&device, &auth.ip).await?;
    require(patch.matches(config), "独立配置回读与目标值不符")?;
    require(equal_except(original, config, fields, &patch.fields()) && home(&before) == home(&after),
        "阶段修改了未选择字段或主屏策略")?;
    require(original_runtime.1 == after_runtime.1, "配置操作意外重启系统")?;
    if restart {
        require(original_runtime.0 != after_runtime.0 && result.evidence["config"]["restart"] == "succeeded"
            && result.evidence["config"]["restartRequested"] == true && result.evidence["config"]["readback"] == "succeeded",
            "未确认应用新进程及重启回读")?;
    } else {
        require(original_runtime.0 == after_runtime.0 && result.evidence["config"]["restart"] == "not_required",
            "同值或无需重启阶段改变了应用进程")?;
    }
    if changed { require(result.evidence["config"]["save"] == "saved", "改值未取得保存确认")?; }
    if must_skip {
        require(!changed && result.device == ResultState::Skipped && result.evidence["config"]["save"] == "unchanged",
            "同值配置未跳过重复保存")?;
    }
    Ok(json!({"fields":patch.fields(),"passed":true,"restartRequired":restart,
        "pidChanged":original_runtime.0 != after_runtime.0,"otherFieldsUnchanged":true,"sameValueSkipped":must_skip}))
}

fn record_fields(record: &Value) -> TestResult<Vec<String>> {
    let fields: Vec<String> = serde_json::from_value(record["publicFields"].clone())?;
    require(fields.len() == 23 && fields.iter().all(|field|
        inxaiot_desk_buddy_lib::domain::smart_screen::app_config_schema::safe_field_path(field)), "恢复字段清单无效")?;
    Ok(fields)
}

async fn restore(state: &FormalAppState, auth: &Authorization, directory: &Path, record: &mut Value) -> TestResult<()> {
    phase(directory, record, "restore-read-and-settle")?;
    let project = record["projectId"].as_str().ok_or("恢复项目缺失")?.to_string();
    let id = record["screenId"].as_str().ok_or("恢复资产缺失")?.to_string();
    let asset = ScreenRepository::new(state.local_store.pool().clone()).asset(&project, &id).await?;
    require(asset.source == "local" && asset.fields.ip == auth.ip && record["target"] == auth.ip,
        "恢复目标与隔离本机资产不匹配")?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    settle(state, &project, &id).await?;
    let row = read_one(state, &project, &id).await?;
    let schema = schema(&row)?;
    let config = row.config.as_ref().ok_or("恢复前配置缺失")?;
    let fields = record_fields(record)?;
    let others: Vec<_> = fields.iter().filter(|key| !AUDIO.contains(&key.as_str()) && key.as_str() != AUTO).cloned().collect();
    require(sha(&projection(config, &others)) == record["otherFieldsSha256"] && home(&row) == record["homeEnabled"],
        "环境或主屏被外部修改，未覆盖并保留恢复责任")?;
    // 只有本轮候选值可自动恢复；未知外部改值必须交给操作者判断。
    let mut patch = AppConfigPatch::default();
    for key in AUDIO.into_iter().chain([AUTO]) {
        let original = record["originalValues"].get(key).ok_or("恢复原值缺失")?;
        let current = field_value(config, key).ok_or("恢复实际值缺失")?;
        require(schema.field(key).is_some_and(|field| field.accepts(original)), "原值不符合当前设备定义，未写入")?;
        if !values_equal(original, current) {
            require(record["attemptedValues"].get(key).is_some_and(|value| values_equal(value, current)),
                "当前值不是本轮写入值，未覆盖外部修改")?;
            patch.set.insert(key.into(), original.clone());
        }
    }
    if !patch.is_empty() {
        phase(directory, record, "restore-formal-task")?;
        let result = checked_change(state, auth, &project, &id, &fields, patch, false).await?;
        record["restoreResult"] = result;
    }
    let after = read_one(state, &project, &id).await?;
    let config = after.config.as_ref().ok_or("恢复后配置缺失")?;
    require(AUDIO.into_iter().chain([AUTO]).all(|key|
        values_equal(field_value(config, key).unwrap_or(&Value::Null), &record["originalValues"][key])), "音频或启动配置尚未恢复")?;
    require(sha(&projection(config, &fields)) == record["allFieldsSha256"] && home(&after) == record["homeEnabled"],
        "全部公开可写配置或主屏策略未恢复一致")?;
    record["restoreRequired"] = json!(false);
    record["restored"] = json!(true);
    phase(directory, record, "restored")
}

async fn run_case(state: &FormalAppState, auth: &Authorization, directory: &Path, record: &mut Value) -> TestResult<()> {
    phase(directory, record, "identity")?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    let project = Stage75Adapter::new(state).create_project(support::local_input()).await?.project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let id = repo.save_local(&project, &ScreenFields { name: "语音字段定义实机回归".into(), ip: auth.ip.clone(),
        size: if auth.ip == TARGETS[0].0 { "10" } else { "4" }.into(), ..Default::default() }, None, None).await?;
    let asset = repo.asset(&project, &id).await?;
    require(asset.source == "local", "不允许平台注册资产参与本机回归")?;
    let observed = device.inspect(&asset, "inspect", CancellationToken::new()).await?;
    let mac = observed.observed_mac.as_deref().filter(|value| valid_mac(value)).ok_or("未取得有效设备身份")?;
    let mut fields = asset.fields.clone(); fields.mac = normalize_mac(mac);
    repo.save_local(&project, &fields, Some(&id), Some(asset.revision)).await?;
    record["projectId"] = json!(project); record["screenId"] = json!(id);
    phase(directory, record, "baseline-read")?;
    let before = read_one(state, &project, &id).await?;
    let config = before.config.as_ref().ok_or("原配置缺失")?;
    let cap = before.capabilities.as_ref().ok_or("原能力缺失")?;
    require(safe_for_formal_evidence(config), "配置含鉴权或不透明地址，不进入正式预检或任务")?;
    let original_runtime = runtime(&device, &auth.ip).await?;
    record["originalPid"] = json!(original_runtime.0);
    record["originalBootId"] = json!(original_runtime.1);
    if auth.mode == "old-read" {
        require(cap.get("configSchema").is_none() && cap.get("configSchemaError").is_none(),
            "旧版模式实际读取到了schema或损坏schema")?;
        require(field_value(config, "environments.current").and_then(Value::as_str).is_some()
            && config.get("environments").is_some() && cap["supportedFields"].as_array().is_some_and(|fields|
                fields.iter().any(|field| field == "customDeviceName")), "旧版基础配置不可读")?;
        let baseline = sha(config);
        phase(directory, record, "old-audio-preflight-must-refuse")?;
        let preview = maintenance::preflight_with_config(state, &project, input(&id),
            Some(BTreeMap::from([(id.clone(), patch("audioConfig.keywordsScore", json!(2.1))?)]))).await?;
        require(preview.items.len() == 1 && preview.items[0].state != "ready", "旧版错误允许了新音频参数")?;
        require(state.task_repository.list_recent(&project, 10).await?.is_empty(), "旧版只读场景意外创建执行任务")?;
        let after = read_one(state, &project, &id).await?;
        require(sha(after.config.as_ref().ok_or("旧版回读失败")?) == baseline
            && runtime(&device, &auth.ip).await? == original_runtime && home(&before) == home(&after),
            "旧版拒绝新字段后配置、进程或主屏发生变化")?;
        record["oldFieldsReadable"] = json!(true); record["audioPreflightRefused"] = json!(true);
        record["deviceWrites"] = json!(0); record["passed"] = json!(true);
        return phase(directory, record, "old-read-passed");
    }
    let baseline_schema = schema(&before)?;
    let fields: Vec<_> = baseline_schema.fields.iter().filter(|field| field.writable).map(|field| field.key.clone()).collect();
    require(fields.len() == 23, "本轮要求23项公开可写配置")?;
    let others: Vec<_> = fields.iter().filter(|key| !AUDIO.contains(&key.as_str()) && key.as_str() != AUTO).cloned().collect();
    record["publicFields"] = json!(fields);
    record["originalValues"] = projection(config, &AUDIO.into_iter().chain([AUTO]).map(String::from).collect::<Vec<_>>());
    record["allFieldsSha256"] = json!(sha(&projection(config, &fields)));
    record["otherFieldsSha256"] = json!(sha(&projection(config, &others)));
    record["homeEnabled"] = home(&before);
    record["attemptedValues"] = json!({}); record["phases"] = json!([]);
    persist(directory, record)?;
    for (stage, key) in [("hotword-change", "audioConfig.keywordsScore"), ("vad-change", "audioConfig.minSilenceDuration")] {
        phase(directory, record, stage)?;
        let current = read_one(state, &project, &id).await?;
        let value = candidate(&schema(&current)?, current.config.as_ref().ok_or("测试前配置缺失")?, key)?;
        record["attemptedValues"][key] = value.clone();
        record["restoreRequired"] = json!(true);
        persist(directory, record)?; // 在预检和任何正式写入前持久化恢复责任。
        let result = checked_change(state, auth, &project, &id, &fields, patch(key, value.clone())?, false).await?;
        record["phases"].as_array_mut().unwrap().push(json!({"phase":stage,"result":result}));
        phase(directory, record, "same-value-no-restart")?;
        let result = checked_change(state, auth, &project, &id, &fields, patch(key, value)?, true).await?;
        record["phases"].as_array_mut().unwrap().push(json!({"phase":"same-value","result":result}));
        persist(directory, record)?;
    }
    restore(state, auth, directory, record).await?;
    record["passed"] = json!(true);
    phase(directory, record, "voice-roundtrip-passed")
}

#[tokio::test]
#[ignore = "仅三屏白名单及显式实机门禁；修改/恢复还须独立ALLOW_WRITES门禁；数据固定在新隔离库"]
async fn voice_schema_formal_real() -> TestResult<()> {
    let auth = authorization_from_env()?; // 全部门禁先于任何设备命令和数据目录创建。
    let base = evidence_root()?;
    let (directory, mut record) = if let Some(case) = &auth.case {
        let directory = base.join(case).canonicalize()?;
        require(directory.parent() == Some(base.as_path()), "恢复目录不在本轮隔离根下")?;
        let record: Value = serde_json::from_slice(&std::fs::read(directory.join("restore-state.json"))?)?;
        require(record["target"] == auth.ip && record["serialSha256"] == sha(&json!(auth.serial))
            && record["mode"] == "voice-roundtrip" && record["restoreRequired"] == true, "恢复文件不匹配或没有恢复责任")?;
        (directory, record)
    } else {
        let directory = base.join(uuid::Uuid::now_v7().to_string());
        std::fs::create_dir(&directory)?;
        let record = json!({"target":auth.ip,"mode":auth.mode,"serialSha256":sha(&json!(auth.serial)),
            "restoreRequired":false,"restored":false,"passed":false,"phase":"created"});
        persist(&directory, &record)?;
        (directory, record)
    };
    println!("隔离用例目录：{}", directory.display());
    let state = support::state_at(&directory, true).await;
    let outcome = if auth.mode == "restore" { restore(&state, &auth, &directory, &mut record).await }
        else { run_case(&state, &auth, &directory, &mut record).await };
    if outcome.is_err() {
        // 不打印底层错误原文，避免网络地址或设备私有信息进入终端和汇总报告。
        record["failedPhase"] = record["phase"].clone();
        record["passed"] = json!(false);
        let _ = persist(&directory, &record);
        if auth.mode == "voice-roundtrip" && record["restoreRequired"] == true {
            let restored = restore(&state, &auth, &directory, &mut record).await.is_ok();
            record["automaticRestorePassed"] = json!(restored);
            let _ = persist(&directory, &record);
        }
    }
    support::close(state).await;
    if outcome.is_err() {
        return Err(format!("实机回归未通过，失败阶段：{}；恢复责任：{}。只查看隔离记录，未知执行结果不重放。",
            record["failedPhase"].as_str().unwrap_or("unknown"), record["restoreRequired"]).into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "仅INX_VOICE_SCHEMA_GUI_SEED=YES时创建固定新隔离库；不连接任何设备或平台"]
async fn voice_schema_gui_seed_local_only() -> TestResult<()> {
    require(std::env::var("INX_VOICE_SCHEMA_GUI_SEED").as_deref() == Ok("YES"),
        "缺少GUI隔离种子门禁，未创建任何数据")?;
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("工程目录缺失")?.canonicalize()?;
    let directory = workspace.join(".review-tools/voice-schema-20261022/data");
    std::fs::create_dir_all(&directory)?;
    let directory = directory.canonicalize()?;
    require(directory.starts_with(&workspace) && !directory.join("local.db").exists()
        && !directory.join("local.db-wal").exists() && !directory.join("local.db-shm").exists()
        && !directory.join("gui-seed-ids.json").exists(),
        "隔离目录不在工作台仓库内或已存在数据库，不覆盖现有数据")?;
    // devices=false不注册真实设备任务；所有操作仅通过正式本机项目和本机资产接口落盘。
    let state = support::state_at(&directory, false).await;
    let outcome: TestResult<Value> = async {
        let mut input = support::local_input(); input.name = "语音配置隔离回归".into();
        let project = Stage75Adapter::new(&state).create_project(input).await?.project.id;
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let mut screens = Vec::new();
        for (index, (ip, _)) in TARGETS.iter().enumerate() {
            let name = ["10寸测试屏", "4寸测试屏A", "4寸测试屏B"][index];
            let id = repo.save_local(&project, &ScreenFields { name: name.into(), ip: (*ip).into(),
                size: if index == 0 { "10" } else { "4" }.into(), ..Default::default() }, None, None).await?;
            screens.push(json!({"id":id,"name":name,"ip":ip}));
        }
        Ok(json!({"projectId":project,"projectName":"语音配置隔离回归","screens":screens,"deviceConnections":0,"platformConnections":0}))
    }.await;
    support::close(state).await;
    let record = outcome?;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(directory.join("gui-seed-ids.json"))?;
    file.write_all(&serde_json::to_vec_pretty(&record)?)?;
    file.sync_all()?;
    println!("GUI隔离库：{}", directory.display());
    Ok(())
}

#[test]
fn gates_require_matching_ip_serial_and_separate_write_authorization() {
    let mut env = BTreeMap::from([
        ("INX_VOICE_SCHEMA_REAL".into(), "YES".into()),
        ("INX_VOICE_SCHEMA_TARGET".into(), TARGETS[0].0.into()),
        ("INX_VOICE_SCHEMA_EXPECTED_SERIAL".into(), TARGETS[0].1.into()),
        ("INX_VOICE_SCHEMA_MODE".into(), "old-read".into()),
    ]);
    assert!(authorization(&env).is_ok());
    env.insert("INX_VOICE_SCHEMA_MODE".into(), "voice-roundtrip".into());
    assert!(authorization(&env).is_err());
    env.insert("INX_VOICE_SCHEMA_ALLOW_WRITES".into(), "YES".into());
    assert!(authorization(&env).is_ok());
    env.insert("INX_VOICE_SCHEMA_EXPECTED_SERIAL".into(), TARGETS[1].1.into());
    assert!(authorization(&env).is_err());
    env.insert("INX_VOICE_SCHEMA_EXPECTED_SERIAL".into(), TARGETS[0].1.into());
    env.insert("INX_VOICE_SCHEMA_REAL".into(), "NO".into());
    assert!(authorization(&env).is_err());
    env.insert("INX_VOICE_SCHEMA_REAL".into(), "YES".into());
    env.insert("INX_VOICE_SCHEMA_MODE".into(), "restore".into());
    assert!(authorization(&env).is_err());
    env.insert("INX_VOICE_SCHEMA_CASE".into(), "../../production".into());
    assert!(authorization(&env).is_err());
    env.insert("INX_VOICE_SCHEMA_CASE".into(), uuid::Uuid::now_v7().to_string());
    assert!(authorization(&env).is_ok());
}

#[test]
fn authorization_addresses_are_refused_before_formal_snapshot_and_candidate_uses_device_bounds() {
    for address in ["http://u:p@localhost/api", "http://localhost/api?token=abc", "ws://localhost/api#key"] {
        assert!(!safe_for_formal_evidence(&json!({"environments":{"test":{"otaUrl":address}}})));
    }
    assert!(safe_for_formal_evidence(&json!({"otaUrl":"http://192.168.3.13:8091/api/device/ota"})));
    let cap: Value = serde_json::from_str(include_str!("../../test/fixtures/app-config-schema-v1/schema-capabilities-default.json")).unwrap();
    let read: Value = serde_json::from_str(include_str!("../../test/fixtures/app-config-schema-v1/schema-read-default.json")).unwrap();
    let schema = ConfigSchema::from_capabilities(&cap["data"]).unwrap().unwrap();
    for key in ["audioConfig.keywordsScore", "audioConfig.minSilenceDuration"] {
        let value = candidate(&schema, &read["data"], key).unwrap();
        assert!(schema.field(key).unwrap().accepts(&value));
        assert!(!values_equal(&value, field_value(&read["data"], key).unwrap()));
    }
}
