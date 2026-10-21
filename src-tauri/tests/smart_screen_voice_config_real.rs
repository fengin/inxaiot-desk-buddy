#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            app_config::{AppConfigPatch, AppConfigRead, AppHomeRead, field_value},
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
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const VOICE: &str = "autoStartVoiceEngine";
const ENVIRONMENT_FIELDS: [&str; 4] = ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"];

struct Baseline {
    id: String,
    slot: String,
    target: support::LiveScreen,
    config: Value,
    home: Option<AppHomeRead>,
    description_mac_matches: bool,
}

fn require(condition: bool, message: &str) -> TestResult<()> {
    if condition { Ok(()) } else { Err(message.to_string().into()) }
}

fn safe_error(error: &dyn std::fmt::Display) -> String {
    let text = error.to_string();
    let hidden = regex::Regex::new(r#"(?i)(?:https?|wss?)://[^\s\"'<>]+"#)
        .map(|pattern| pattern.replace_all(&text, "[地址已隐藏]").into_owned())
        .unwrap_or_else(|_| "本阶段未完成".into());
    let hidden = regex::Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b")
        .map(|pattern| pattern.replace_all(&hidden, "[IP已隐藏]").into_owned())
        .unwrap_or_else(|_| "本阶段未完成".into());
    regex::Regex::new(r"(?i)\b(?:[0-9a-f]{2}[:-]){5}[0-9a-f]{2}\b")
        .map(|pattern| pattern.replace_all(&hidden, "[MAC已隐藏]").chars().take(800).collect())
        .unwrap_or_else(|_| "本阶段未完成".into())
}

fn authorized_targets() -> TestResult<(Vec<support::LiveScreen>, BTreeMap<String, String>)> {
    let targets = support::live_screens()?;
    require(targets.len() == 3 && targets.iter().filter(|item| item.size == "4").count() == 2
        && targets.iter().filter(|item| item.size == "10").count() == 1,
        "本轮只接受测试说明中明确列出的两台4寸和一台10寸屏")?;
    let encoded = std::env::var("INX_SCREEN_VOICE_EXPECTED_SERIALS")
        .map_err(|_| "必须先由操作者核实设备身份并显式设置 INX_SCREEN_VOICE_EXPECTED_SERIALS")?;
    let serials: BTreeMap<String, String> = serde_json::from_str(&encoded)
        .map_err(|_| "授权设备序列号映射必须是 IP 到非空序列号的 JSON 对象")?;
    require(serials.len() == targets.len() && targets.iter().all(|target| serials.get(&target.ip)
        .is_some_and(|serial| !serial.is_empty() && serial.len() <= 128
            && serial.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'))),
        "授权序列号映射必须与测试说明的三台屏完全对应；未连接设备")?;
    Ok((targets, serials))
}

async fn runtime(device: &AdbDevice, ip: &str) -> TestResult<(String, String)> {
    let pid = device.shell(ip, &["pidof", "chat.xiaoxin.app"], CancellationToken::new()).await?;
    let boot = device.shell(ip, &["cat", "/proc/sys/kernel/random/boot_id"], CancellationToken::new()).await?;
    require(!pid.trim().is_empty() && pid.chars().all(|ch| ch.is_ascii_digit() || ch.is_ascii_whitespace()),
        "没有有效的小新进程，不能确认应用是否重启")?;
    require(!boot.trim().is_empty() && boot.trim().bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-'),
        "没有有效的系统启动标识，不能排除意外系统重启")?;
    Ok((pid.trim().into(), boot.trim().into()))
}

async fn read_one(state: &FormalAppState, project: &str, id: &str) -> TestResult<AppConfigRead> {
    let mut rows = app_config::read(state, project, &[id.to_string()]).await?;
    require(rows.len() == 1, "配置读取未返回唯一目标")?;
    let row = rows.pop().ok_or("配置读取没有目标")?;
    require(row.config.is_some(), "未读取到屏端普通配置；未输出设备配置原文")?;
    Ok(row)
}

fn voice_supported(row: &AppConfigRead) -> bool {
    row.capabilities.as_ref().is_some_and(|cap| cap["ready"] == true
        && cap["supportedFields"].as_array().is_some_and(|fields| fields.contains(&json!(VOICE))))
        && row.config.as_ref().and_then(|config| config.get(VOICE)).is_some_and(Value::is_boolean)
}

fn flag(config: &Value) -> TestResult<bool> {
    config[VOICE].as_bool().ok_or_else(|| "语音启动设置未返回严格布尔值".into())
}

fn current_environment(config: &Value) -> TestResult<String> {
    config["environments"]["current"].as_str().filter(|value| ["test", "pre", "prod"].contains(value))
        .map(str::to_string).ok_or_else(|| "当前运行环境无效".into())
}

fn patch(set: Value, home: Option<bool>) -> TestResult<AppConfigPatch> {
    let patch = AppConfigPatch { set: serde_json::from_value(set)?, clear: Vec::new(), home_enabled: home };
    patch.validate()?;
    Ok(patch)
}

fn input(id: &str) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "app_config".into(), target_ids: vec![id.into()], application_id: None, apk: None,
        app_version: String::new(), abi: "universal".into(), reinstall: false, concurrency: 1,
        expected_targets: Default::default(), retry_of_operation_id: None,
    }
}

async fn baselines(
    state: &FormalAppState, project: &str, targets: Vec<support::LiveScreen>,
    serials: &BTreeMap<String, String>, directory: &Path,
) -> TestResult<Vec<Baseline>> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut originals = BTreeMap::new();
    let mut homes = BTreeMap::new();
    let mut output = Vec::new();
    for (index, target) in targets.into_iter().enumerate() {
        device.connect(&target.ip, CancellationToken::new()).await?;
        let serial = device.shell(&target.ip, &["getprop", "ro.serialno"], CancellationToken::new()).await?;
        require(serials.get(&target.ip).is_some_and(|expected| serial.trim() == expected),
            "设备稳定序列号不匹配，停止建立本轮设备基线")?;
        let id = repo.save_local(project, &ScreenFields {
            name: format!("语音启动配置实机屏{}", index + 1), ip: target.ip.clone(), size: target.size.clone(),
            ..Default::default()
        }, None, None).await?;
        let asset = repo.asset(project, &id).await?;
        // 沿用产品 eth0 身份规则；无线或人工设备名中的 MAC 不替代设备实测。
        let observed = device.inspect(&asset, "inspect", CancellationToken::new()).await?;
        let actual_mac = observed.observed_mac.as_deref().filter(|value| valid_mac(value))
            .ok_or("稳定序列号已核实，但未取得有效的产品设备身份，未开始修改")?;
        let mut fields = asset.fields.clone(); fields.mac = normalize_mac(actual_mac);
        repo.save_local(project, &fields, Some(&id), Some(asset.revision)).await?;
        let row = read_one(state, project, &id).await?;
        let config = row.config.ok_or("普通配置基线缺失")?;
        originals.insert(id.clone(), config.clone());
        homes.insert(id.clone(), row.home.clone());
        // 私有恢复快照与公开阶段报告分开，绝不将原值写 stdout 或汇总报告。
        std::fs::write(directory.join("original-config.json"), serde_json::to_vec(&originals)?)?;
        std::fs::write(directory.join("original-home.json"), serde_json::to_vec(&homes)?)?;
        output.push(Baseline {
            id, slot: format!("screen-{}", index + 1), description_mac_matches: !target.mac.is_empty()
                && normalize_mac(&target.mac) == normalize_mac(actual_mac),
            target, config, home: row.home,
        });
    }
    std::fs::write(directory.join("baseline-context.json"), serde_json::to_vec(&json!({
        "projectId": project, "screenIds": output.iter().map(|item| &item.id).collect::<Vec<_>>()
    }))?)?;
    Ok(output)
}

fn writable_projection(config: &Value) -> Value {
    let mut value = json!({"customDeviceName":config["customDeviceName"],"environments":{"current":config["environments"]["current"]}});
    if let Some(flag) = config.get(VOICE) { value[VOICE] = flag.clone(); }
    for environment in ["test", "pre", "prod"] {
        for field in ENVIRONMENT_FIELDS { value["environments"][environment][field] = config["environments"][environment][field].clone(); }
    }
    value
}

fn domain_guards() -> TestResult<()> {
    for value in [Value::Null, json!("false"), json!(0)] {
        let invalid: AppConfigPatch = serde_json::from_value(json!({"set":{VOICE:value}}))?;
        require(invalid.validate().is_err(), "领域规则不应允许非布尔语音启动值")?;
    }
    let clear: AppConfigPatch = serde_json::from_value(json!({"clear":[VOICE]}))?;
    require(clear.validate().is_err(), "领域规则不应允许清空语音启动值")?;
    patch(json!({VOICE:false}), None)?.validate()?;
    Ok(())
}

async fn submit_patch(state: &FormalAppState, project: &str, id: &str, patch: AppConfigPatch) -> TestResult<ScreenTargetResult> {
    let request = input(id);
    let preview = maintenance::preflight_with_config(state, project, request.clone(), Some(BTreeMap::from([(id.into(), patch)]))).await?;
    require(preview.items.len() == 1 && preview.items[0].state == "ready", "正式配置预检未通过；未提交执行")?;
    let task = tasks::submit(state, project, &preview.id, request).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let value = state.task_repository.get(&task).await?;
            if value.state.is_terminal() || value.state == TaskState::FinalizingFailed { break; }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        Ok::<(), Box<dyn std::error::Error>>(())
    }).await??;
    let results = task_data::read_results(state.local_store.pool(), project, &task).await?;
    require(results.finished && results.targets.len() == 1, "配置任务没有唯一完整结果")?;
    results.targets.get(id).cloned().ok_or_else(|| "配置任务没有目标结果".into())
}

async fn checked_phase(
    state: &FormalAppState, project: &str, baseline: &Baseline, stage: &str, patch: AppConfigPatch,
    expected_voice: Option<bool>, must_skip: bool, phases: &mut Vec<Value>,
) -> TestResult<()> {
    let fields = patch.fields();
    let outcome: TestResult<Value> = async {
        let device = AdbDevice::new(AndroidTools::discover()?);
        let before = read_one(state, project, &baseline.id).await?.config.ok_or("阶段读取没有配置")?;
        let restart = patch.restart_required(&before);
        let config_changed = !patch.matches(&before);
        let (pid, boot) = runtime(&device, &baseline.target.ip).await?;
        let result = submit_patch(state, project, &baseline.id, patch.clone()).await?;
        require(matches!(result.device, ResultState::Succeeded | ResultState::Skipped), "正式配置任务未成功；详细结果保留在独立本机库")?;
        require(result.business == ResultState::NotRequired && result.shared == ResultState::NotRequired,
            "本机配置回归不应写入平台业务库或共享库")?;
        let reread = read_one(state, project, &baseline.id).await?;
        let after = reread.config.as_ref().ok_or("阶段回读没有配置")?;
        let (after_pid, after_boot) = runtime(&device, &baseline.target.ip).await?;
        require(boot == after_boot, "配置操作不应重启设备系统")?;
        require(patch.matches(after), "配置独立回读与本阶段目标不一致")?;
        if fields.is_empty() {
            require(result.evidence["config"]["save"] == "not_required",
                "单独修改主屏不应保存普通配置")?;
        }
        if let Some(value) = expected_voice { require(flag(after)? == value, "语音启动值没有保留本阶段预期")?; }
        if restart {
            require(pid != after_pid && result.evidence["config"]["restart"] == "succeeded"
                && result.evidence["config"]["restartRequested"] == true && result.evidence["config"]["readback"] == "succeeded",
                "改值后没有确认原配置任务的应用重启与回读")?;
        } else {
            require(pid == after_pid && result.evidence["config"]["restart"] == "not_required",
                "无需重启的阶段改变了应用进程")?;
        }
        if config_changed { require(result.evidence["config"]["save"] == "saved", "明确改值未取得完整保存结果")?; }
        if must_skip {
            require(result.device == ResultState::Skipped && result.evidence["config"]["save"] == "unchanged"
                && pid == after_pid, "同值任务不应重复保存或重启")?;
        }
        if let Some(home) = patch.home_enabled {
            require(reread.home.as_ref().is_some_and(|value| value.supported && value.enabled == Some(home)), "主屏持久策略回读不同")?;
            if home { require(result.evidence["home"]["runtimeReadback"] == "succeeded", "组合任务未确认小新主屏激活")?; }
            else { require(result.evidence["home"]["activation"] == "not_required", "解除主屏固定不应发起主屏激活")?; }
        }
        Ok(json!({"phase":stage,"screen":baseline.slot,"passed":true,"fields":fields,
            "restartRequired":restart,"applicationPidChanged":pid != after_pid,"systemBootUnchanged":boot == after_boot,
            "configurationReadbackMatches":true,"singleConfigurationTask":true,"sameValueSkipped":must_skip}))
    }.await;
    match outcome {
        Ok(record) => { phases.push(record); Ok(()) }
        Err(error) => {
            phases.push(json!({"phase":stage,"screen":baseline.slot,"passed":false,"fields":fields,"error":safe_error(&error)}));
            Err(error)
        }
    }
}

async fn settle_before_restore(state: &FormalAppState, project: &str, id: &str) -> TestResult<()> {
    for task in state.task_repository.list_recent(project, 500).await? {
        if task.operation_type != "app_config" { continue; }
        let results = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
        let Some(target) = results.targets.get(id) else { continue; };
        require(!matches!(task.state, TaskState::Queued | TaskState::Running | TaskState::Cancelling),
            "原任务仍在执行，未与它并发恢复配置")?;
        if task.state == TaskState::FinalizingFailed || state.task_repository.results_protected(&task.id).await?
            || matches!(target.device, ResultState::Pending | ResultState::Unknown) {
            // 这里只核实原结果，不重放 saveConfig 或应用重启。
            tasks::verify(state, project, &task.id).await?;
            let current = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
            require(!state.task_repository.results_protected(&task.id).await?
                && state.task_repository.get(&task.id).await?.state != TaskState::FinalizingFailed
                && current.targets.get(id).is_some_and(|value| !matches!(value.device, ResultState::Pending | ResultState::Unknown)),
                "旧任务结果仍不确定，未自动重放或覆盖配置")?;
        }
    }
    Ok(())
}

fn restoration_patch(original: &Value, current: &Value, fields: &[String]) -> TestResult<AppConfigPatch> {
    let mut patch = AppConfigPatch::default();
    for field in fields {
        let value = field_value(original, field).ok_or("恢复基线缺少原字段")?;
        if field_value(current, field) == Some(value) { continue; }
        if value.is_null() { patch.clear.push(field.clone()); }
        else { patch.set.insert(field.clone(), value.clone()); }
    }
    patch.validate()?;
    Ok(patch)
}

async fn restore_one(
    state: &FormalAppState, project: &str, baseline: &Baseline, phases: &mut Vec<Value>,
) -> TestResult<Value> {
    settle_before_restore(state, project, &baseline.id).await?;
    let mut errors = Vec::new();
    // 每套环境各自恢复，某一套失败后仍尝试其它环境和全局项。
    for environment in ["test", "pre", "prod"] {
        let outcome: TestResult<()> = async {
            let current = read_one(state, project, &baseline.id).await?.config.ok_or("恢复时未读到配置")?;
            let fields = ENVIRONMENT_FIELDS.map(|field| format!("environments.{environment}.{field}"));
            let patch = restoration_patch(&baseline.config, &current, &fields)?;
            if !patch.is_empty() { checked_phase(state, project, baseline, "restore_environment", patch, None, false, phases).await?; }
            Ok(())
        }.await;
        if let Err(error) = outcome { errors.push(safe_error(&error)); }
    }
    let global: TestResult<()> = async {
        let read = read_one(state, project, &baseline.id).await?;
        let current = read.config.as_ref().ok_or("恢复全局项时没有配置")?;
        let fields = ["environments.current".to_string(), "customDeviceName".to_string(), VOICE.to_string()];
        let mut patch = restoration_patch(&baseline.config, current, &fields)?;
        let original_home = baseline.home.as_ref().filter(|home| home.supported).and_then(|home| home.enabled)
            .ok_or("原主屏设置未确认，不能猜测恢复值")?;
        if !read.home.as_ref().is_some_and(|home| home.supported && home.enabled == Some(original_home)
            && (!original_home || home.active == Some(true))) { patch.home_enabled = Some(original_home); }
        if !patch.is_empty() { checked_phase(state, project, baseline, "restore_global_and_home", patch, Some(flag(&baseline.config)?), false, phases).await?; }
        Ok(())
    }.await;
    if let Err(error) = global { errors.push(safe_error(&error)); }
    let final_read = read_one(state, project, &baseline.id).await?;
    let config_restored = final_read.config.as_ref().map(writable_projection) == Some(writable_projection(&baseline.config));
    let home_restored = baseline.home.as_ref().zip(final_read.home.as_ref()).is_some_and(|(before, after)|
        before.supported && after.supported && before.enabled == after.enabled);
    let home_runtime_matches = baseline.home.as_ref().zip(final_read.home.as_ref()).is_some_and(|(before, after)|
        before.active.is_none() || before.active == after.active);
    Ok(json!({"screen":baseline.slot,"phase":"restoration","passed":config_restored && home_restored && errors.is_empty(),
        "ordinaryConfigurationAndVoiceRestored":config_restored,"homePolicyRestored":home_restored,
        "homeRuntimeMatchesBaseline":home_runtime_matches,"errors":errors}))
}

async fn new_configuration_scenarios(
    state: &FormalAppState, project: &str, baseline: &Baseline, phases: &mut Vec<Value>,
) -> TestResult<()> {
    for value in [false, true, false] {
        let current = read_one(state, project, &baseline.id).await?.config.ok_or("语音轮转前配置缺失")?;
        checked_phase(state, project, baseline, "voice_value", patch(json!({VOICE:value}), None)?, Some(value), flag(&current)? == value, phases).await?;
        checked_phase(state, project, baseline, "voice_same_value", patch(json!({VOICE:value}), None)?, Some(value), true, phases).await?;
    }
    let environment = current_environment(&baseline.config)?;
    let inactive = ["test", "pre", "prod"].into_iter().find(|candidate| *candidate != environment).ok_or("没有非当前环境")?;
    for (stage, environment) in [("inactive_h5_keeps_global_voice", inactive), ("current_h5_keeps_global_voice", environment.as_str())] {
        let field = format!("environments.{environment}.h5ReadyCheckEnabled");
        let original = field_value(&baseline.config, &field).and_then(Value::as_bool).ok_or("原H5检查开关不是布尔值")?;
        checked_phase(state, project, baseline, stage, patch(json!({field.clone():!original}), None)?, Some(false), false, phases).await?;
        checked_phase(state, project, baseline, "restore_h5_before_combination", patch(json!({field:original}), None)?, Some(false), false, phases).await?;
    }
    checked_phase(state, project, baseline, "home_off_only_preserves_voice_and_pid",
        patch(json!({}), Some(false))?, Some(false), false, phases).await?;
    checked_phase(state, project, baseline, "home_on_only_preserves_voice_and_pid",
        patch(json!({}), Some(true))?, Some(false), false, phases).await?;
    let h5 = format!("environments.{environment}.h5ReadyCheckEnabled");
    let original_h5 = field_value(&baseline.config, &h5).and_then(Value::as_bool).ok_or("原当前环境H5检查开关缺失")?;
    checked_phase(state, project, baseline, "combined_voice_current_h5_home_one_task",
        patch(json!({VOICE:true,h5:!original_h5}), Some(true))?, Some(true), false, phases).await?;
    let proposed_name = if baseline.config["customDeviceName"] == "语音配置联测临时名" { "语音配置联测另一个临时名" } else { "语音配置联测临时名" };
    checked_phase(state, project, baseline, "name_only_keeps_voice_and_pid",
        patch(json!({"customDeviceName":proposed_name}), None)?, Some(true), false, phases).await
}

#[tokio::test]
#[ignore = "仅操作者显式授权三屏稳定序列号后读取旧包基线；仅预检，不提交或安装"]
async fn legacy_voice_flag_is_optional_and_original_fields_remain_readable() -> TestResult<()> {
    domain_guards()?;
    let (targets, serials) = authorized_targets()?;
    let directory = support::evidence_dir("voice-config-legacy-readonly")?;
    let state = support::state_at(&directory, true).await;
    let mut report = Vec::new();
    let outcome: TestResult<()> = async {
        let project = Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let baselines = baselines(&state, &project, targets, &serials, &directory).await?;
        let device = AdbDevice::new(AndroidTools::discover()?);
        let mut old_count = 0;
        for baseline in &baselines {
            let row = read_one(&state, &project, &baseline.id).await?;
            let (pid, boot) = runtime(&device, &baseline.target.ip).await?;
            let old = !voice_supported(&row);
            if old {
                old_count += 1;
                let preview = maintenance::preflight_with_config(&state, &project, input(&baseline.id), Some(BTreeMap::from([
                    (baseline.id.clone(), patch(json!({VOICE:false}), None)?)
                ]))).await?;
                require(preview.items.len() == 1 && preview.items[0].state == "blocked"
                    && (preview.items[0].reason.contains("不支持所选字段") || preview.items[0].reason.contains("未返回有效的语音")),
                    "旧包新字段没有因能力或有效值缺失而阻断")?;
                let current = current_environment(&baseline.config)?;
                let h5 = format!("environments.{current}.h5ReadyCheckEnabled");
                let h5_value = field_value(&baseline.config, &h5).and_then(Value::as_bool).ok_or("旧包未返回原H5字段")?;
                let preview = maintenance::preflight_with_config(&state, &project, input(&baseline.id), Some(BTreeMap::from([
                    (baseline.id.clone(), patch(json!({"customDeviceName":"兼容只读预检",h5:h5_value}), None)?)
                ]))).await?;
                require(preview.items.len() == 1 && preview.items[0].state == "ready", "旧包原有字段预检没有通过")?;
            }
            let after = read_one(&state, &project, &baseline.id).await?.config.ok_or("只读兼容检查后缺少配置")?;
            let (after_pid, after_boot) = runtime(&device, &baseline.target.ip).await?;
            require(writable_projection(&after) == writable_projection(&baseline.config) && pid == after_pid && boot == after_boot,
                "只读兼容预检不应改变配置、应用进程或系统启动标识")?;
            report.push(json!({"screen":baseline.slot,"phase":"legacy_compatibility_readonly","passed":true,
                "voiceUnsupported":old,"originalFieldsPreflightReady":old,"noConfigurationSubmission":true,
                "configurationPreserved":true,"applicationPidPreserved":true,"systemBootPreserved":true,
                "descriptionMacMatchesProductIdentity":baseline.description_mac_matches}));
        }
        require(old_count > 0, "没有发现旧包，不能将新版读取结果声明为旧APK实机兼容通过")?;
        require(tasks::views(&state, &project).await?.is_empty(), "旧包兼容预检不应创建任何设备执行任务")?;
        Ok(())
    }.await;
    std::fs::write(directory.join("report.json"), serde_json::to_vec_pretty(&json!({
        "passed":outcome.is_ok(),"phase":"legacy_compatibility","checks":report,
        "error":outcome.as_ref().err().map(|error| safe_error(error))
    }))?)?;
    support::close(state).await;
    if outcome.is_err() { return Err(format!("旧包只读兼容未通过，安全报告保存在 {}", directory.display()).into()); }
    println!("VOICE_LEGACY_READONLY_PASS {}", directory.display());
    Ok(())
}

#[tokio::test]
#[ignore = "仅操作者授权并安装新版后执行三屏语音开关/主屏/H5/名称真实配置任务；含应用重启和基线恢复"]
async fn voice_startup_roundtrip_restarts_readback_and_restore() -> TestResult<()> {
    domain_guards()?;
    let (targets, serials) = authorized_targets()?;
    let directory = support::evidence_dir("voice-config-roundtrip")?;
    let state = support::state_at(&directory, true).await;
    let project = Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
    let prepared = baselines(&state, &project, targets, &serials, &directory).await;
    let mut phases = Vec::new();
    let mut errors = Vec::new();
    let mut restoration = Vec::new();
    if let Ok(baselines) = &prepared {
        let validated: TestResult<()> = async {
            for baseline in baselines {
                let read = read_one(&state, &project, &baseline.id).await?;
                require(voice_supported(&read), "新版用例需要三屏均明确支持语音字段且回读严格布尔值；尚未开始修改")?;
                require(baseline.home.as_ref().is_some_and(|home| home.supported && home.enabled.is_some()),
                    "新版组合用例需先确认原主屏设置；尚未开始修改")?;
            }
            Ok(())
        }.await;
        if let Err(error) = validated { errors.push(safe_error(&error)); }
        else {
            for baseline in baselines {
                if let Err(error) = new_configuration_scenarios(&state, &project, baseline, &mut phases).await {
                    errors.push(format!("{} 阶段未完成：{}", baseline.slot, safe_error(&error)));
                }
            }
            // 不用 assert!/panic 提前离开：失败也逐台核实旧结果、尝试恢复并保留报告。
            for baseline in baselines {
                match restore_one(&state, &project, baseline, &mut phases).await {
                    Ok(record) => restoration.push(record),
                    Err(error) => restoration.push(json!({"screen":baseline.slot,"phase":"restoration","passed":false,"error":safe_error(&error)})),
                }
            }
        }
    } else if let Err(error) = &prepared { errors.push(safe_error(error)); }
    let restored = !restoration.is_empty() && restoration.iter().all(|item| item["passed"] == true);
    let passed = errors.is_empty() && restored;
    std::fs::write(directory.join("report.json"), serde_json::to_vec_pretty(&json!({
        "passed":passed,"phase":"voice_roundtrip","restored":restored,"checks":phases,"restoration":restoration,"errors":errors
    }))?)?;
    support::close(state).await;
    if !passed { return Err(format!("语音配置联测或还原未完成，安全报告保存在 {}", directory.display()).into()); }
    println!("VOICE_CONFIGURATION_REAL_PASS {}", directory.display());
    Ok(())
}

#[tokio::test]
#[ignore = "仅恢复显式指定的本轮语音配置原证据目录；不重放原42个功能阶段"]
async fn restore_voice_config_baseline_from_evidence() -> TestResult<()> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).parent().ok_or("缺少工作台目录")?;
    let allowed_root = workspace.join(".review-tools/voice-config-real-20261021/formal").canonicalize()?;
    let requested = std::path::PathBuf::from(std::env::var_os("INX_SCREEN_VOICE_RESTORE_CASE")
        .ok_or("必须显式设置 INX_SCREEN_VOICE_RESTORE_CASE 绝对路径")?);
    require(requested.is_absolute(), "恢复目录必须使用绝对路径")?;
    let directory = requested.canonicalize()?;
    require(directory.is_dir() && directory.starts_with(&allowed_root)
        && directory.file_name().and_then(|name| name.to_str())
            == Some("voice-config-roundtrip-01a15c5f-1b5d-7d80-8d1c-b32c60e41b49")
        && ["baseline-context.json", "original-config.json", "original-home.json", "local.db"]
            .iter().all(|name| directory.join(name).is_file()),
        "仅允许恢复本次原语音配置证据目录及其既有基线和本机数据库")?;
    let (targets, serials) = authorized_targets()?;
    let context: Value = serde_json::from_slice(&std::fs::read(directory.join("baseline-context.json"))?)?;
    let project = context["projectId"].as_str().filter(|value| !value.is_empty()).ok_or("原证据没有项目编号")?.to_string();
    let ids: Vec<String> = serde_json::from_value(context["screenIds"].clone())?;
    let originals: BTreeMap<String, Value> = serde_json::from_slice(&std::fs::read(directory.join("original-config.json"))?)?;
    let homes: BTreeMap<String, Option<AppHomeRead>> = serde_json::from_slice(&std::fs::read(directory.join("original-home.json"))?)?;
    let unique: std::collections::BTreeSet<_> = ids.iter().collect();
    require(ids.len() == targets.len() && unique.len() == ids.len()
        && originals.len() == ids.len() && homes.len() == ids.len()
        && ids.iter().all(|id| originals.contains_key(id) && homes.contains_key(id)),
        "原目标、普通配置和主屏基线数量或编号不一致；未恢复")?;
    let state = support::state_at(&directory, true).await;
    let mut phases = Vec::new();
    let mut restoration = Vec::new();
    let mut errors = Vec::new();
    let audited: TestResult<Vec<Baseline>> = async {
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let snapshot = repo.snapshot(&project).await?;
        require(snapshot.screens.len() == ids.len() && snapshot.screens.iter().all(|screen| screen.source == "local"
            && ids.contains(&screen.id)), "原项目不是本轮三台本机屏，未更改目标或项目")?;
        let device = AdbDevice::new(AndroidTools::discover()?);
        let mut output = Vec::new();
        let mut matched = std::collections::BTreeSet::new();
        for (index, id) in ids.iter().enumerate() {
            let asset = repo.asset(&project, id).await?;
            let target = targets.iter().find(|target| target.ip == asset.fields.ip && target.size == asset.fields.size)
                .ok_or("原资产地址或尺寸与当前授权说明不符，未更改原目标")?.clone();
            require(matched.insert(target.ip.clone()), "原资产重复使用同一授权屏，未恢复")?;
            require(valid_mac(&asset.fields.mac), "原资产没有有效的产品身份，不能猜测恢复")?;
            device.connect(&target.ip, CancellationToken::new()).await?;
            let serial = device.shell(&target.ip, &["getprop", "ro.serialno"], CancellationToken::new()).await?;
            require(serials.get(&target.ip).is_some_and(|expected| serial.trim() == expected),
                "恢复前稳定序列号与显式授权不符，未写入设备")?;
            let observed = device.inspect(&asset, "inspect", CancellationToken::new()).await?;
            require(observed.observed_mac.as_deref().is_some_and(|mac| valid_mac(mac)
                && normalize_mac(mac) == normalize_mac(&asset.fields.mac)),
                "恢复前产品采集身份与原资产不符，未改变资产IP或MAC")?;
            let config = originals.get(id).cloned().ok_or("原普通配置缺失")?;
            flag(&config)?;
            current_environment(&config)?;
            let home = homes.get(id).cloned().ok_or("原主屏基线缺失")?;
            require(home.as_ref().is_some_and(|value| value.supported && value.enabled.is_some()),
                "原主屏策略没有明确基线，不能猜测恢复值")?;
            output.push(Baseline { id: id.clone(), slot: format!("screen-{}", index + 1),
                description_mac_matches: !target.mac.is_empty() && normalize_mac(&target.mac) == normalize_mac(&asset.fields.mac),
                target, config, home });
        }
        // 全部原目标都通过只读审核后才进入任何恢复写入。
        for baseline in &output { settle_before_restore(&state, &project, &baseline.id).await?; }
        Ok(output)
    }.await;
    match audited {
        Ok(baselines) => {
            for baseline in &baselines {
                match restore_one(&state, &project, baseline, &mut phases).await {
                    Ok(record) => restoration.push(record),
                    Err(error) => restoration.push(json!({"screen":baseline.slot,"phase":"restoration_only","passed":false,"error":safe_error(&error)})),
                }
            }
        }
        Err(error) => errors.push(safe_error(&error)),
    }
    let passed = errors.is_empty() && restoration.len() == ids.len() && restoration.iter().all(|record| record["passed"] == true);
    // 独立报告，不改原失败报告、原快照，也不重建项目或重放功能场景。
    std::fs::write(directory.join("restoration-only-report.json"), serde_json::to_vec_pretty(&json!({
        "passed":passed,"phase":"restoration_only","originalScenariosReplayed":false,
        "originalBaselineRewritten":false,"checks":phases,"restoration":restoration,"errors":errors
    }))?)?;
    support::close(state).await;
    if !passed { return Err(format!("基线单独恢复未完成，安全报告保存在 {}", directory.display()).into()); }
    println!("VOICE_BASELINE_RESTORED {}", directory.display());
    Ok(())
}
