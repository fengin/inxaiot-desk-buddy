//! 配置桥生命周期实机回归。外部生命周期动作由操作者完成，本用例不启动/停止应用或语音。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{
            app_config::{AppConfigPatch, AppConfigRead},
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
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const TARGETS: [&str; 3] = ["192.168.3.106", "192.168.3.74", "192.168.3.82"];
const NAME: &str = "customDeviceName";

struct Authorization {
    ip: String,
    serial: String,
    stage: String,
    mode: String,
    expect_ui_absent: bool,
}

fn authorization_from_env() -> TestResult<Authorization> {
    let values = [
        "INX_CONFIG_BRIDGE_TARGET",
        "INX_CONFIG_BRIDGE_EXPECTED_SERIAL",
        "INX_CONFIG_BRIDGE_STAGE",
        "INX_CONFIG_BRIDGE_STAGE_READY",
        "INX_CONFIG_BRIDGE_MODE",
        "INX_CONFIG_BRIDGE_ALLOW_WRITES",
        "INX_CONFIG_BRIDGE_EXPECT_UI_ABSENT",
    ]
    .into_iter()
    .filter_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| (key.to_string(), value))
    })
    .collect();
    authorization(&values)
}

fn require(condition: bool, message: &'static str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn authorization(values: &BTreeMap<String, String>) -> TestResult<Authorization> {
    let get = |name: &str| values.get(name).map(String::as_str).unwrap_or("");
    let ip = get("INX_CONFIG_BRIDGE_TARGET");
    let serial = get("INX_CONFIG_BRIDGE_EXPECTED_SERIAL");
    let stage = get("INX_CONFIG_BRIDGE_STAGE");
    let mode = get("INX_CONFIG_BRIDGE_MODE");
    let ui_absent = get("INX_CONFIG_BRIDGE_EXPECT_UI_ABSENT");
    require(
        ["", "YES", "NO"].contains(&ui_absent),
        "界面关闭断言只接受YES或NO",
    )?;
    require(TARGETS.contains(&ip), "目标不在本轮三屏白名单，未连接设备")?;
    require(
        !serial.is_empty()
            && serial.len() <= 128
            && serial
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "必须显式提供已核实的稳定序列号，未连接设备",
    )?;
    require(
        !stage.is_empty()
            && stage.len() <= 60
            && stage
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
        "必须明确本轮生命周期阶段名，未连接设备",
    )?;
    require(
        get("INX_CONFIG_BRIDGE_STAGE_READY") == "YES",
        "操作者尚未声明外部生命周期动作完成，未连接设备",
    )?;
    require(
        [
            "read-only",
            "roundtrip",
            "begin-name",
            "update-name",
            "assert-name",
            "restore-name",
        ]
        .contains(&mode),
        "联测模式必须显式指定",
    )?;
    require(
        ["read-only", "assert-name"].contains(&mode)
            || get("INX_CONFIG_BRIDGE_ALLOW_WRITES") == "YES",
        "修改/恢复模式缺少显式写入门禁，未连接设备",
    )?;
    Ok(Authorization {
        ip: ip.into(),
        serial: serial.into(),
        stage: stage.into(),
        mode: mode.into(),
        expect_ui_absent: ui_absent == "YES",
    })
}

fn sha(value: &Value) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).expect("JSON value serializes"))
    )
}

// 只比较原本允许编辑的字段；地址原文始终只留在内存，不写报告或恢复文件。
fn writable(config: &Value) -> Value {
    let mut value = json!({NAME:config[NAME], "autoStartVoiceEngine":config["autoStartVoiceEngine"],
        "environments":{"current":config["environments"]["current"]}});
    for env in ["test", "pre", "prod"] {
        value["environments"][env] = json!({});
        for field in ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"] {
            value["environments"][env][field] = config["environments"][env][field].clone();
        }
    }
    value
}

fn other_fields(config: &Value) -> Value {
    let mut value = writable(config);
    value.as_object_mut().unwrap().remove(NAME);
    value
}

fn name_patch(name: &Value) -> TestResult<AppConfigPatch> {
    let mut patch = AppConfigPatch::default();
    if name.is_null() {
        patch.clear.push(NAME.into());
    } else {
        patch.set.insert(NAME.into(), name.clone());
    }
    patch.validate()?;
    Ok(patch)
}

fn safe_for_formal_evidence(value: &Value) -> bool {
    match value {
        Value::Object(fields) => fields.values().all(safe_for_formal_evidence),
        Value::Array(values) => values.iter().all(safe_for_formal_evidence),
        Value::String(text) if text.contains("://") => reqwest::Url::parse(text).is_ok_and(|url| {
            url.username().is_empty()
                && url.password().is_none()
                && url.fragment().is_none()
                && url.query_pairs().all(|(name, _)| {
                    ![
                        "token",
                        "access_token",
                        "refresh_token",
                        "id_token",
                        "auth",
                        "authorization",
                        "password",
                        "pwd",
                        "secret",
                        "key",
                        "api_key",
                        "apikey",
                        "signature",
                        "sig",
                        "credential",
                    ]
                    .contains(&name.to_ascii_lowercase().as_str())
                })
        }),
        _ => true,
    }
}

fn evidence_root() -> TestResult<PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("工作台目录缺失")?
        .canonicalize()?;
    let base = workspace.join(".review-tools/config-bridge-real-20261022");
    std::fs::create_dir_all(&base)?;
    let base = base.canonicalize()?;
    require(
        base.starts_with(&workspace),
        "证据目录不能指向工作台仓库以外",
    )?;
    Ok(base)
}

fn persist(directory: &Path, record: &Value) -> TestResult<()> {
    // 明确记录恢复责任；仅落盘原名称、测试名称和字段摘要，不保存地址或其他配置原值。
    let pending = directory.join("restore-state.pending.json");
    let mut file = std::fs::File::create(&pending)?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(pending, directory.join("restore-state.json"))?;
    Ok(())
}

async fn runtime(device: &AdbDevice, ip: &str) -> TestResult<(String, String)> {
    let pid = device
        .shell(ip, &["pidof", "chat.xiaoxin.app"], CancellationToken::new())
        .await?;
    require(
        !pid.is_empty()
            && pid
                .bytes()
                .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace()),
        "应用进程状态不可靠",
    )?;
    let boot = device
        .shell(
            ip,
            &["cat", "/proc/sys/kernel/random/boot_id"],
            CancellationToken::new(),
        )
        .await?;
    require(
        uuid::Uuid::parse_str(boot.trim()).is_ok(),
        "系统启动标识不可靠",
    )?;
    Ok((pid, boot))
}

async fn check_identity(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    device.connect(&auth.ip, CancellationToken::new()).await?;
    let actual = device
        .shell(
            &auth.ip,
            &["getprop", "ro.serialno"],
            CancellationToken::new(),
        )
        .await?;
    require(
        actual == auth.serial,
        "稳定序列号与授权不符，未读取或修改配置",
    )
}

async fn check_ui_absent(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    if !auth.expect_ui_absent {
        return Ok(());
    }
    let activities = device
        .shell(
            &auth.ip,
            &["dumpsys", "activity", "activities"],
            CancellationToken::new(),
        )
        .await?;
    require(
        activities.contains("ACTIVITY MANAGER ACTIVITIES"),
        "未取得可识别的Activity状态，不能判定界面未启动",
    )?;
    let resumed = activities.lines().any(|line| {
        (line.contains("mResumedActivity:")
            || line.contains("mResumedActivity=")
            || line.trim_start().starts_with("ResumedActivity:")
            || line.contains("topResumedActivity="))
            && (line.contains("chat.xiaoxin.app/.MainActivity ")
                || line.contains("chat.xiaoxin.app/chat.xiaoxin.app.MainActivity "))
    });
    require(!resumed, "Provider-only阶段出现小新前台主界面，未继续联测")
}

async fn read_one(state: &FormalAppState, project: &str, id: &str) -> TestResult<AppConfigRead> {
    let mut rows = app_config::read(state, project, &[id.into()]).await?;
    require(rows.len() == 1, "正式读取未返回唯一目标")?;
    let row = rows.pop().ok_or("正式读取结果缺失")?;
    require(
        row.capabilities
            .as_ref()
            .is_some_and(|cap| cap["ready"] == true),
        "配置桥未就绪",
    )?;
    require(row.config.is_some(), "正式读取没有配置")?;
    Ok(row)
}

async fn submit(
    state: &FormalAppState,
    project: &str,
    id: &str,
    patch: AppConfigPatch,
) -> TestResult<ScreenTargetResult> {
    // 正式任务会在自己的本机库保存配置前后快照；鉴权地址不进入该任务，避免落盘。
    let current = read_one(state, project, id)
        .await?
        .config
        .ok_or("预检前配置缺失")?;
    require(
        safe_for_formal_evidence(&current),
        "配置包含鉴权地址或不透明片段，只保留内存检查，不执行会落盘快照的正式任务",
    )?;
    let input = ScreenOperationInput {
        action: "app_config".into(),
        target_ids: vec![id.into()],
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 1,
        expected_targets: Default::default(),
        retry_of_operation_id: None,
    };
    let preview = maintenance::preflight_with_config(
        state,
        project,
        input.clone(),
        Some(BTreeMap::from([(id.into(), patch)])),
    )
    .await?;
    require(
        preview.items.len() == 1 && preview.items[0].state == "ready",
        "正式配置预检没有通过",
    )?;
    let task = tasks::submit(state, project, &preview.id, input).await?;
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let current = state.task_repository.get(&task).await?;
            if current.state.is_terminal() || current.state == TaskState::FinalizingFailed {
                break Ok::<(), inxaiot_desk_buddy_lib::core::error::AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await??;
    let result = task_data::read_results(state.local_store.pool(), project, &task)
        .await?
        .targets
        .remove(id)
        .ok_or("正式任务没有目标结果")?;
    require(
        matches!(result.device, ResultState::Succeeded | ResultState::Skipped),
        "正式配置任务未成功",
    )?;
    require(
        result.business == ResultState::NotRequired && result.shared == ResultState::NotRequired,
        "本机未注册屏不应产生平台业务或共享写入",
    )?;
    require(
        result.evidence["config"]["restart"] == "not_required",
        "仅修改名称不应重启小新",
    )?;
    Ok(result)
}

async fn settle(state: &FormalAppState, project: &str, id: &str) -> TestResult<()> {
    for task in state.task_repository.list_recent(project, 30).await? {
        if task.operation_type != "app_config" {
            continue;
        }
        require(
            !matches!(
                task.state,
                TaskState::Queued | TaskState::Running | TaskState::Cancelling
            ),
            "旧配置任务仍在执行，未并发覆盖或恢复",
        )?;
        let result = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
        let uncertain = result
            .targets
            .get(id)
            .is_none_or(|row| matches!(row.device, ResultState::Pending | ResultState::Unknown));
        if uncertain
            || task.state == TaskState::FinalizingFailed
            || state.task_repository.results_protected(&task.id).await?
        {
            tasks::verify(state, project, &task.id).await?; // 只回读原结果，不重放保存。
            let after =
                task_data::read_results(state.local_store.pool(), project, &task.id).await?;
            require(
                !state.task_repository.results_protected(&task.id).await?
                    && state.task_repository.get(&task.id).await?.state
                        != TaskState::FinalizingFailed
                    && after.targets.get(id).is_some_and(|row| {
                        !matches!(row.device, ResultState::Pending | ResultState::Unknown)
                    }),
                "旧任务结果尚不确定，恢复责任保留供操作者处理",
            )?;
        }
    }
    Ok(())
}

async fn restore_name(
    state: &FormalAppState,
    directory: &Path,
    record: &mut Value,
) -> TestResult<()> {
    let project = record["projectId"]
        .as_str()
        .ok_or("恢复项目缺失")?
        .to_string();
    let id = record["screenId"]
        .as_str()
        .ok_or("恢复屏编号缺失")?
        .to_string();
    let asset = ScreenRepository::new(state.local_store.pool().clone())
        .asset(&project, &id)
        .await?;
    require(
        asset.source == "local" && record["target"] == asset.fields.ip,
        "恢复记录与原本机未注册资产不匹配",
    )?;
    settle(state, &project, &id).await?;
    let current = read_one(state, &project, &id)
        .await?
        .config
        .ok_or("恢复前未读到配置")?;
    require(
        current[NAME] == record["originalName"]
            || record["temporaryNames"]
                .as_array()
                .is_some_and(|names| names.contains(&current[NAME])),
        "当前名称不是本轮写入值，不覆盖其他人的修改",
    )?;
    if current[NAME] != record["originalName"] {
        submit(state, &project, &id, name_patch(&record["originalName"])?).await?;
    }
    let after = read_one(state, &project, &id).await?;
    let config = after.config.as_ref().ok_or("恢复后未读到配置")?;
    require(config[NAME] == record["originalName"], "名称未恢复原值")?;
    // 名称已恢复便清除其恢复责任；其他字段差异不擅自覆写，由最终断言报告。
    record["restoreRequired"] = json!(false);
    record["restored"] = json!(true);
    record["otherFieldsUnchanged"] =
        json!(sha(&other_fields(config)) == record["otherFieldsSha256"]);
    record["homePolicyUnchanged"] = json!(
        after
            .home
            .as_ref()
            .and_then(|home| home.enabled)
            .map(|value| json!(value))
            .unwrap_or(Value::Null)
            == record["homeEnabled"]
    );
    persist(directory, record)?;
    require(
        record["otherFieldsUnchanged"] == true && record["homePolicyUnchanged"] == true,
        "名称已恢复，但环境、语音启动值或主屏策略发生意外变化",
    )
}

#[tokio::test]
#[ignore = "需显式设置三屏白名单IP/序列号/阶段完成门禁；roundtrip另需允许写入，仅改名称并恢复"]
async fn configuration_bridge_after_lifecycle_real() -> TestResult<()> {
    let auth = authorization_from_env()?; // 全部门禁先于任何设备命令。
    require(
        ["read-only", "roundtrip", "begin-name"].contains(&auth.mode.as_str()),
        "跨阶段及恢复模式请使用对应的继续用例",
    )?;
    let base = evidence_root()?;
    let directory = base.join(format!("{}-{}", auth.stage, uuid::Uuid::now_v7()));
    std::fs::create_dir(&directory)?;
    let state = support::state_at(&directory, true).await;
    let mut record = json!({"target":auth.ip,"stage":auth.stage,"mode":auth.mode,
        "serialSha256":sha(&json!(auth.serial)),"restoreRequired":false,"restored":false});
    let outcome: TestResult<()> = async {
        let device = AdbDevice::new(AndroidTools::discover()?);
        check_identity(&device, &auth).await?;
        check_ui_absent(&device, &auth).await?;
        let project = Stage75Adapter::new(&state)
            .create_project(support::local_input())
            .await?
            .project
            .id;
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let id = repo
            .save_local(
                &project,
                &ScreenFields {
                    name: "配置桥生命周期回归".into(),
                    ip: auth.ip.clone(),
                    size: if auth.ip == TARGETS[0] { "10" } else { "4" }.into(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await?;
        record["projectId"] = json!(project);
        record["screenId"] = json!(id);
        let asset = repo.asset(&project, &id).await?;
        require(asset.source == "local", "仅允许本机未注册测试屏")?;
        let observed = device
            .inspect(&asset, "inspect", CancellationToken::new())
            .await?;
        let mac = observed
            .observed_mac
            .as_deref()
            .filter(|value| valid_mac(value))
            .ok_or("未取得正式操作需要的设备身份")?;
        let mut fields = asset.fields.clone();
        fields.mac = normalize_mac(mac);
        repo.save_local(&project, &fields, Some(&id), Some(asset.revision))
            .await?;
        let before = read_one(&state, &project, &id).await?;
        check_ui_absent(&device, &auth).await?;
        let original = before.config.ok_or("原配置缺失")?;
        let original_runtime = runtime(&device, &auth.ip).await?;
        record["originalPid"] = json!(original_runtime.0);
        record["originalBootId"] = json!(original_runtime.1);
        require(
            original["autoStartVoiceEngine"].is_boolean(),
            "原语音启动值缺失或类型无效",
        )?;
        record["originalAutoStartVoiceEngine"] = original["autoStartVoiceEngine"].clone();
        record["otherFieldsSha256"] = json!(sha(&other_fields(&original)));
        record["homeEnabled"] = before
            .home
            .and_then(|home| home.enabled)
            .map(|value| json!(value))
            .unwrap_or(Value::Null);
        for _ in 0..3 {
            let actual = read_one(&state, &project, &id)
                .await?
                .config
                .ok_or("重复读取缺少配置")?;
            require(
                writable(&actual) == writable(&original),
                "连续只读期间配置意外变化",
            )?;
            check_ui_absent(&device, &auth).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        record["repeatedReadsPassed"] = json!(true);
        if ["roundtrip", "begin-name"].contains(&auth.mode.as_str()) {
            name_patch(&original[NAME])?; // 原值不可准确恢复时，禁止开始写入。
            require(
                safe_for_formal_evidence(&original),
                "配置含鉴权地址，只作内存读取，不开始修改",
            )?;
            require(
                original[NAME].as_str().is_none_or(|value| {
                    !value.contains("://")
                        && !value.to_ascii_lowercase().contains("token=")
                        && !value.to_ascii_lowercase().contains("password=")
                }),
                "原名称疑似包含地址或鉴权内容，不落盘恢复文件也不开始修改",
            )?;
            let temporary = json!(format!("配置桥回归-{}", uuid::Uuid::now_v7()));
            record["originalName"] = original[NAME].clone();
            record["temporaryName"] = temporary.clone();
            record["temporaryNames"] = json!([temporary]);
            record["restoreRequired"] = json!(true);
            persist(&directory, &record)?; // 必须先持久化恢复责任，后提交正式任务。
            let change: TestResult<()> = async {
                let result = submit(&state, &project, &id, name_patch(&temporary)?).await?;
                require(
                    result.evidence["config"]["save"] == "saved",
                    "名称改值未取得保存成功结果",
                )?;
                let actual = read_one(&state, &project, &id)
                    .await?
                    .config
                    .ok_or("改值回读失败")?;
                require(
                    actual[NAME] == temporary && other_fields(&actual) == other_fields(&original),
                    "改值未生效或影响其他字段",
                )?;
                let same = submit(&state, &project, &id, name_patch(&temporary)?).await?;
                require(
                    same.device == ResultState::Skipped
                        && same.evidence["config"]["save"] == "unchanged",
                    "同值保存没有跳过",
                )?;
                record["saveAndSameValuePassed"] = json!(true);
                Ok(())
            }
            .await;
            if auth.mode == "roundtrip" || change.is_err() {
                let restored = restore_name(&state, &directory, &mut record).await;
                if restored.is_err() {
                    record["restoreFailed"] = json!(true);
                    persist(&directory, &record)?;
                }
                restored?;
            } else {
                record["expectedName"] = temporary;
                record["heldAcrossLifecycle"] = json!(true);
                record["nameGeneration"] = json!(1);
                persist(&directory, &record)?;
            }
            change?;
        }
        require(
            runtime(&device, &auth.ip).await? == original_runtime,
            "测试期间应用进程或系统启动标识改变",
        )?;
        check_ui_absent(&device, &auth).await?;
        record["pidAndBootPreserved"] = json!(true);
        Ok(())
    }
    .await;
    let report = json!({"passed":outcome.is_ok(),"stage":auth.stage,"target":auth.ip,"mode":auth.mode,
        "repeatedReadsPassed":record["repeatedReadsPassed"],"saveAndSameValuePassed":record["saveAndSameValuePassed"],
        "restoreRequired":record["restoreRequired"],"restored":record["restored"],
        "heldAcrossLifecycle":record["heldAcrossLifecycle"],"nameGeneration":record["nameGeneration"],
        "otherFieldsUnchanged":record["otherFieldsUnchanged"],"homePolicyUnchanged":record["homePolicyUnchanged"],
        "pidAndBootPreserved":record["pidAndBootPreserved"],"platformDatabaseUsed":false,
        "originalAutoStartVoiceEngine":record["originalAutoStartVoiceEngine"],
        "configurationValuesPrinted":false,"lifecycleActionsPerformed":false});
    let mut report = report;
    report["providerOnlyUiAbsenceRequired"] = json!(auth.expect_ui_absent);
    std::fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    support::close(state).await;
    if outcome.is_err() {
        return Err(format!(
            "配置桥联测未通过；只读安全报告位于 {}；请先核对restoreRequired",
            directory.display()
        )
        .into());
    }
    println!("CONFIG_BRIDGE_REAL_PASS {}", directory.display());
    Ok(())
}

fn open_case(auth: &Authorization) -> TestResult<(PathBuf, Value)> {
    let base = evidence_root()?;
    let requested = PathBuf::from(
        std::env::var_os("INX_CONFIG_BRIDGE_RESTORE_CASE").ok_or("必须明确指定原隔离恢复目录")?,
    )
    .canonicalize()?;
    require(
        requested.starts_with(&base)
            && requested.parent() == Some(base.as_path())
            && requested.join("local.db").is_file(),
        "只允许本轮直接子目录及原本机库",
    )?;
    let record: Value =
        serde_json::from_slice(&std::fs::read(requested.join("restore-state.json"))?)?;
    require(
        record["target"] == auth.ip
            && record["serialSha256"] == sha(&json!(auth.serial))
            && record["restoreRequired"] == true,
        "恢复目标或恢复责任不匹配",
    )?;
    Ok((requested, record))
}

#[tokio::test]
#[ignore = "跨Activity阶段复用原隔离case；assert-name只读，update-name需明确允许写入并保留恢复责任"]
async fn continue_configuration_bridge_case_real() -> TestResult<()> {
    let auth = authorization_from_env()?;
    require(
        ["assert-name", "update-name"].contains(&auth.mode.as_str()),
        "继续用例只接受断言或第二次改名",
    )?;
    let (directory, mut record) = open_case(&auth)?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    check_identity(&device, &auth).await?;
    check_ui_absent(&device, &auth).await?;
    let state = support::state_at(&directory, true).await;
    let outcome: TestResult<()> = async {
        let project = record["projectId"]
            .as_str()
            .ok_or("原项目缺失")?
            .to_string();
        let id = record["screenId"]
            .as_str()
            .ok_or("原屏编号缺失")?
            .to_string();
        let asset = ScreenRepository::new(state.local_store.pool().clone())
            .asset(&project, &id)
            .await?;
        require(
            asset.source == "local" && asset.fields.ip == auth.ip,
            "原资产与本次授权目标不一致",
        )?;
        settle(&state, &project, &id).await?;
        let current_runtime = runtime(&device, &auth.ip).await?;
        require(
            record["originalPid"] == current_runtime.0
                && record["originalBootId"] == current_runtime.1,
            "跨Activity用例要求同一应用进程与系统启动周期，不能用重启掩盖缓存失效",
        )?;
        let before = read_one(&state, &project, &id)
            .await?
            .config
            .ok_or("继续阶段未读到配置")?;
        require(
            before[NAME] == record["expectedName"],
            "当前引擎未读到前一阶段最新名称，不能接受旧缓存或覆盖未知值",
        )?;
        require(
            sha(&other_fields(&before)) == record["otherFieldsSha256"],
            "其他允许配置发生漂移，不继续写入",
        )?;
        if auth.mode == "update-name" {
            require(
                record["nameGeneration"] == 1,
                "第二次改名只能接续第一次持有阶段",
            )?;
            let next = json!(format!("配置桥第二次回归-{}", uuid::Uuid::now_v7()));
            require(
                safe_for_formal_evidence(&before),
                "配置含鉴权地址，不进入正式写入任务",
            )?;
            record["temporaryNames"]
                .as_array_mut()
                .ok_or("原临时名称集合缺失")?
                .push(next.clone());
            record["pendingNameGeneration"] = json!(2);
            persist(&directory, &record)?;
            let result = submit(&state, &project, &id, name_patch(&next)?).await?;
            require(
                result.evidence["config"]["save"] == "saved",
                "第二次改名未保存",
            )?;
            let actual = read_one(&state, &project, &id)
                .await?
                .config
                .ok_or("第二次改名未回读")?;
            require(
                actual[NAME] == next && sha(&other_fields(&actual)) == record["otherFieldsSha256"],
                "第二次改名没有生效或改变了其他配置",
            )?;
            record["expectedName"] = next;
            record["nameGeneration"] = json!(2);
            record
                .as_object_mut()
                .unwrap()
                .remove("pendingNameGeneration");
            persist(&directory, &record)?;
        }
        // 包含多次独立调用，避免一次读到正确值但随后又回到旧绑定。
        for _ in 0..3 {
            let row = read_one(&state, &project, &id).await?;
            let actual = row.config.ok_or("跨阶段复核配置缺失")?;
            require(
                actual[NAME] == record["expectedName"]
                    && sha(&other_fields(&actual)) == record["otherFieldsSha256"],
                "跨阶段配置读取未保持最新持久值",
            )?;
            require(
                row.home
                    .and_then(|home| home.enabled)
                    .map(|value| json!(value))
                    .unwrap_or(Value::Null)
                    == record["homeEnabled"],
                "跨阶段主屏持久策略改变",
            )?;
        }
        require(
            runtime(&device, &auth.ip).await? == current_runtime,
            "本阶段发生意外应用或系统重启",
        )?;
        check_ui_absent(&device, &auth).await?;
        Ok(())
    }
    .await;
    // 更新阶段失败仍恢复；纯只读断言失败保留现场与restoreRequired，由操作者决定何时执行恢复用例。
    let mut restore_failed = false;
    if outcome.is_err() && auth.mode == "update-name" {
        restore_failed = restore_name(&state, &directory, &mut record).await.is_err();
    }
    let report = json!({"passed":outcome.is_ok(),"target":auth.ip,"stage":auth.stage,"mode":auth.mode,
        "nameGeneration":record["nameGeneration"],"restoreRequired":record["restoreRequired"],
        "restored":record["restored"],"restorationFailed":restore_failed,"valuesPrinted":false});
    std::fs::write(
        directory.join(format!(
            "stage-{}-{}.json",
            auth.stage,
            uuid::Uuid::now_v7()
        )),
        serde_json::to_vec_pretty(&report)?,
    )?;
    support::close(state).await;
    require(
        outcome.is_ok(),
        "跨阶段配置桥断言失败，详见原case内安全阶段报告；名称恢复责任仍以restore-state为准",
    )
}

#[tokio::test]
#[ignore = "只恢复显式指定的本轮隔离记录中待恢复名称；需白名单/序列号/阶段门禁及写入授权"]
async fn restore_configuration_bridge_name_real() -> TestResult<()> {
    let auth = authorization_from_env()?;
    require(auth.mode == "restore-name", "此用例仅接受恢复模式")?;
    let (requested, mut record) = open_case(&auth)?;
    let device = AdbDevice::new(AndroidTools::discover()?);
    check_identity(&device, &auth).await?;
    check_ui_absent(&device, &auth).await?;
    let state = support::state_at(&requested, true).await;
    let outcome = restore_name(&state, &requested, &mut record).await;
    let ui_outcome = check_ui_absent(&device, &auth).await;
    support::close(state).await;
    require(
        outcome.is_ok() && ui_outcome.is_ok(),
        "名称恢复未完成；保留恢复记录，未输出配置原文",
    )
}

#[test]
fn gate_refuses_unlisted_targets_and_requires_stage_and_write_acknowledgement() {
    let mut values = BTreeMap::from([
        ("INX_CONFIG_BRIDGE_TARGET".into(), TARGETS[0].into()),
        (
            "INX_CONFIG_BRIDGE_EXPECTED_SERIAL".into(),
            "test-serial".into(),
        ),
        ("INX_CONFIG_BRIDGE_STAGE".into(), "after-reopen".into()),
        ("INX_CONFIG_BRIDGE_STAGE_READY".into(), "YES".into()),
        ("INX_CONFIG_BRIDGE_MODE".into(), "read-only".into()),
    ]);
    assert!(authorization(&values).is_ok());
    values.insert("INX_CONFIG_BRIDGE_TARGET".into(), "192.168.3.1".into());
    assert!(authorization(&values).is_err());
    values.insert("INX_CONFIG_BRIDGE_TARGET".into(), TARGETS[0].into());
    values.insert("INX_CONFIG_BRIDGE_STAGE_READY".into(), "NO".into());
    assert!(authorization(&values).is_err());
    values.insert("INX_CONFIG_BRIDGE_STAGE_READY".into(), "YES".into());
    values.insert("INX_CONFIG_BRIDGE_MODE".into(), "roundtrip".into());
    assert!(authorization(&values).is_err());
    values.insert("INX_CONFIG_BRIDGE_ALLOW_WRITES".into(), "YES".into());
    assert!(authorization(&values).is_ok());
    values.insert("INX_CONFIG_BRIDGE_STAGE".into(), "../escape".into());
    assert!(authorization(&values).is_err());
}

#[test]
fn authorization_urls_never_enter_formal_evidence() {
    assert!(safe_for_formal_evidence(
        &json!({"url":"http://192.0.2.1/h5?buildingId=1"})
    ));
    for url in [
        "https://user:pass@example.invalid/",
        "https://example.invalid/?access_token=test",
        "https://example.invalid/#opaque",
    ] {
        assert!(!safe_for_formal_evidence(&json!({"nested":[url]})));
    }
}
