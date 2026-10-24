//! 批量执行修复的窄范围实机验收。默认忽略，只允许指定测试屏和142随机隔离库。
//! 每个用例的主目标只覆盖安装一次；本机确认故障恢复只核实原任务，不重复安装或平台写入。
#[path = "common/project_test_config.rs"]
mod project_test_config;
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::{
        project_management::ProjectManagementPort, smart_screen::ScreenAssetsPort,
    },
    domain::{
        common::{
            project::{PlatformLoginRequest, ProjectInput},
            task::TaskState,
        },
        smart_screen::{
            app_config::{AppConfigPatch, AppConfigRead, field_value},
            model::{ResultState, ScreenAsset, ScreenFields, ScreenTargetResult},
            operation::ScreenOperationInput,
            rules::valid_mac,
        },
    },
    formal::{app_state::FormalAppState, workbench_store::WorkbenchStore},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            apk, app_config,
            assets_service::ScreenAssetsService,
            device::{AdbDevice, AndroidTools, parse_package_version},
            device_owner, diagnostics, maintenance, task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{ConnectOptions, MySqlPool};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const TARGETS: [(&str, &str, &str, u64, &str, &str); 2] = [
    (
        "192.168.3.42",
        "9c000c3c3643071205d",
        "10",
        25038,
        "xiaoxin-2.1.3-armeabi-v7a-release.apk",
        "b2177ee90b597e7ea3648aa0ac272f8e4cd20e830afb6346fe1fec1f321180c0",
    ),
    (
        "192.168.3.82",
        "A47A5CK00524",
        "4",
        26038,
        "xiaoxin-2.1.3-arm64-v8a-release.apk",
        "f5d3375dca323084f72bbeb4b82ea616d4f62370ef6b0d3f2b162596f5f09037",
    ),
];
const NAME: &str = "customDeviceName";
const AUDIO: &str = "audioConfig.vadThreshold";
const TRIGGER: &str = "fixture_batch_local_confirmation";

struct Authorization {
    ip: String,
    serial: String,
    size: String,
    code: u64,
    apk: PathBuf,
    apk_sha256: String,
    inject_confirmation_failure: bool,
    cancel_inflight: bool,
}
struct TestPlatform {
    cfg: project_test_config::DatabaseTestConfig,
    options: sqlx::mysql::MySqlConnectOptions,
    admin: MySqlPool,
    business: String,
    shared: String,
}
fn require(value: bool, message: &'static str) -> TestResult<()> {
    if value { Ok(()) } else { Err(message.into()) }
}
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .to_path_buf()
}
fn sha(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn cleanup_assertions_enabled() -> bool {
    std::env::var("INX_ARTIFACT_CLEANUP_ASSERTIONS").as_deref() == Ok("YES")
}
async fn require_install_artifact_retained(
    state: &FormalAppState,
    project: &str,
    task: &str,
    expected_sha: &str,
) -> TestResult<()> {
    let file = state
        .paths
        .project_task_dir(project, task)?
        .join("xiaoxin.apk");
    require(file.is_file(), "安装中或待核实时任务APK被提前清理")?;
    let bytes = tokio::fs::read(&file).await?;
    require(
        hex::encode(Sha256::digest(bytes)) == expected_sha,
        "任务APK内容发生变化",
    )
}
async fn require_install_cleanup_and_evidence(
    state: &FormalAppState,
    project: &str,
    task: &str,
    auth: &Authorization,
) -> TestResult<()> {
    let file = state
        .paths
        .project_task_dir(project, task)?
        .join("xiaoxin.apk");
    tokio::time::timeout(Duration::from_secs(8), async {
        while file.exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| "安装已安全结束但任务APK仍未清理")?;
    let record = state.task_repository.get(task).await?;
    require(
        record.state.is_terminal() && !state.task_repository.results_protected(task).await?,
        "清理时任务仍需结果保护",
    )?;
    let (plan, _) = task_data::read_plan(state.local_store.pool(), project, task).await?;
    require(
        plan.detail["apk"]["sha256"] == auth.apk_sha256,
        "清理丢失安装包元数据",
    )?;
    require(
        !task_data::read_results(state.local_store.pool(), project, task)
            .await?
            .targets
            .is_empty(),
        "清理丢失逐屏结果",
    )?;
    require(
        tokio::fs::metadata(&record.log_path).await?.len() > 0,
        "清理丢失任务日志",
    )?;
    require(
        hex::encode(Sha256::digest(tokio::fs::read(&auth.apk).await?)) == auth.apk_sha256,
        "清理影响用户原始安装包",
    )
}
async fn diagnostic_survives_startup_cleanup(
    state: &FormalAppState,
    project: &str,
    id: &str,
    directory: &Path,
) -> TestResult<String> {
    let mut input = config_input(id);
    input.action = "diagnostics".into();
    let preview = tasks::preflight(state, project, input.clone()).await?;
    require(
        preview.items.len() == 1 && preview.items[0].state == "ready",
        "真实诊断检查未通过",
    )?;
    let task = tasks::submit(state, project, &preview.id, input).await?;
    wait_task(state, &task).await?;
    require_success(state, project, &task, id, false).await?;
    let before = diagnostics::read(state, project, &task).await?;
    require(
        serde_json::from_str::<Value>(&before)?["screens"]
            .as_array()
            .is_some_and(|v| !v.is_empty()),
        "真实诊断未返回屏端内容",
    )?;
    // 与应用启动相同的候选查询和正式清理方法，不以合成文件代替诊断实采。
    for candidate in state
        .task_repository
        .list_artifact_cleanup_candidates()
        .await?
    {
        inxaiot_desk_buddy_lib::infrastructure::task_runtime::cleanup_finished_task_artifacts(
            &state.task_repository,
            &state.paths,
            &candidate.id,
        )
        .await?;
    }
    inxaiot_desk_buddy_lib::infrastructure::task_runtime::cleanup_orphan_task_artifacts(
        &state.task_repository,
        &state.paths,
    )
    .await?;
    let after = diagnostics::read(state, project, &task).await?;
    require(after == before, "启动清理改变或删除真实诊断文件")?;
    let exported = diagnostics::export(state, project, &task, directory).await?;
    require(
        tokio::fs::read_to_string(exported).await? == before,
        "清理后真实诊断导出内容不一致",
    )?;
    require(
        tokio::fs::metadata(&state.task_repository.get(&task).await?.log_path)
            .await?
            .len()
            > 0,
        "诊断任务日志被清理",
    )?;
    require(
        !task_data::read_results(state.local_store.pool(), project, &task)
            .await?
            .targets
            .is_empty(),
        "诊断任务结果被清理",
    )?;
    Ok(task)
}
fn safe_error(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<sqlx::Error>() {
        return match error {
            sqlx::Error::Database(db) => format!(
                "测试数据库错误，代码={}",
                db.code().as_deref().unwrap_or("unknown")
            ),
            _ => "测试数据库连接或执行未完成".into(),
        };
    }
    let raw = error.to_string();
    let text = regex::Regex::new(r"(?i)[a-z][a-z0-9+.-]*://[^\s]+")
        .unwrap()
        .replace_all(&raw, "<地址已省略>");
    regex::Regex::new(
        r"(?i)(password|passwd|token|secret|authorization|authcode)\s*[:=]\s*[^\s,;]+",
    )
    .unwrap()
    .replace_all(&text, "$1=<已省略>")
    .chars()
    .filter(|ch| !ch.is_control())
    .take(500)
    .collect()
}
fn authorization() -> TestResult<Authorization> {
    require(
        std::env::var("INX_BATCH_RECOVERY_REAL").as_deref() == Ok("YES"),
        "缺少本轮实机授权门禁",
    )?;
    let ip = std::env::var("INX_BATCH_RECOVERY_TARGET")?;
    let serial = std::env::var("INX_BATCH_RECOVERY_EXPECTED_SERIAL")?;
    let target = TARGETS
        .iter()
        .find(|t| t.0 == ip && t.1 == serial)
        .ok_or("目标不在本轮IP和硬件序列号白名单")?;
    require(
        support::live_screens()?
            .iter()
            .any(|s| s.ip == ip && s.size == target.2),
        "测试说明已变化，停止回退历史地址",
    )?;
    let mode = std::env::var("INX_BATCH_RECOVERY_MODE")?;
    require(
        matches!(
            mode.as_str(),
            "normal" | "local-confirm-failure" | "cancel-inflight"
        ),
        "测试模式无效",
    )?;
    require(
        mode != "cancel-inflight"
            || (ip == "192.168.3.82"
                && support::live_screens()?
                    .iter()
                    .any(|s| s.ip == "192.168.3.74" && s.size == "4")),
        "取消模式仅允许82先安装、74作为未执行目标",
    )?;
    let apk = PathBuf::from(std::env::var_os("INX_BATCH_RECOVERY_APK").ok_or("未指定本轮正式包")?)
        .canonicalize()?;
    let release = root()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("inxvision-xiaoxin/xiaoxin-app/release/2.1.3")
        .canonicalize()?;
    require(
        apk.parent() == Some(release.as_path())
            && apk.file_name().and_then(|n| n.to_str()) == Some(target.4),
        "安装包不属于本轮目标的2.1.3正式分包",
    )?;
    Ok(Authorization {
        ip,
        serial,
        size: target.2.into(),
        code: target.3,
        apk,
        apk_sha256: target.5.into(),
        inject_confirmation_failure: mode == "local-confirm-failure",
        cancel_inflight: mode == "cancel-inflight",
    })
}
fn persist(directory: &Path, report: &Value) -> TestResult<()> {
    use std::io::Write;
    let temporary = directory.join("result.pending.json");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(report)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, directory.join("result.json"))?;
    Ok(())
}
fn phase(directory: &Path, report: &mut Value, value: &str) -> TestResult<()> {
    report["phase"] = json!(value);
    persist(directory, report)?;
    println!("实机阶段：{value}");
    Ok(())
}
async fn identity(device: &AdbDevice, auth: &Authorization) -> TestResult<()> {
    device.connect(&auth.ip, CancellationToken::new()).await?;
    let serial = device
        .shell(
            &auth.ip,
            &["getprop", "ro.serialno"],
            CancellationToken::new(),
        )
        .await?;
    require(
        serial.trim() == auth.serial,
        "硬件身份不匹配，未执行设备写入",
    )?;
    require(
        device_owner::read(device, &auth.ip).await.state == "self",
        "测试只使用已有小新OWNER，不设置或撤销权限",
    )?;
    let package = device
        .shell(
            &auth.ip,
            &["dumpsys", "package", "chat.xiaoxin.app"],
            CancellationToken::new(),
        )
        .await?;
    let version = parse_package_version(&package);
    require(
        version.0.as_deref() == Some("2.1.3") && version.1 == Some(auth.code),
        "屏端不是已确认的2.1.3同架构正式包，不升级其他版本",
    )
}
fn queued_secondary(auth: &Authorization) -> Authorization {
    Authorization {
        ip: "192.168.3.74".into(),
        serial: "A47A4AK02040".into(),
        size: "4".into(),
        code: 24038,
        apk: auth.apk.clone(),
        apk_sha256: auth.apk_sha256.clone(),
        inject_confirmation_failure: false,
        cancel_inflight: false,
    }
}
async fn read_config(state: &FormalAppState, project: &str, id: &str) -> TestResult<AppConfigRead> {
    let mut reads = app_config::read(state, project, &[id.into()]).await?;
    require(reads.len() == 1, "配置读取结果数量异常")?;
    let row = reads.remove(0);
    require(
        row.config.is_some()
            && row.owner.as_ref().is_some_and(|v| v.state == "self")
            && row
                .home
                .as_ref()
                .is_some_and(|v| v.supported && v.enabled.is_some())
            && row
                .kiosk
                .as_ref()
                .is_some_and(|v| v.supported && v.configured.is_some() && v.enabled.is_some()),
        "配置/OWNER/主屏/锁定状态未完整读取",
    )?;
    let config = row.config.as_ref().unwrap();
    let environment = config["environments"]["current"]
        .as_str()
        .ok_or("当前环境不可读")?;
    let h5 = config["environments"][environment]["h5Url"]
        .as_str()
        .ok_or("H5地址不可读")?;
    require(
        reqwest::Url::parse(h5)?.host_str() == Some("192.168.3.44"),
        "当前H5不在44，停止而不覆盖",
    )?;
    Ok(row)
}
fn stable_controls(row: &AppConfigRead) -> Value {
    let home = row.home.as_ref().unwrap();
    let kiosk = row.kiosk.as_ref().unwrap();
    json!({"owner":row.owner.as_ref().unwrap().state,
        "homeEnabled":home.enabled,"ownerComponent":row.owner.as_ref().unwrap().component,
        "kioskConfigured":kiosk.configured,"kioskEnabled":kiosk.enabled,"lockTaskMode":kiosk.lock_task_mode})
}
fn without_changed_fields(config: &Value) -> Value {
    let mut value = config.clone();
    value.as_object_mut().unwrap().remove(NAME);
    value["audioConfig"]
        .as_object_mut()
        .unwrap()
        .remove("vadThreshold");
    value
}
async fn wait_task(state: &FormalAppState, id: &str) -> TestResult<()> {
    tokio::time::timeout(Duration::from_secs(1000), async {
        loop {
            let task = state.task_repository.get(id).await?;
            if (task.state.is_terminal() || task.state == TaskState::FinalizingFailed)
                && !state.job_supervisor.contains(id).await
            {
                return Ok::<_, inxaiot_desk_buddy_lib::core::error::AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .map_err(|_| "正式任务未按时结束，保留原任务，不重发设备动作")??;
    Ok(())
}
async fn target_result(
    state: &FormalAppState,
    project: &str,
    task: &str,
    id: &str,
) -> TestResult<ScreenTargetResult> {
    task_data::read_results(state.local_store.pool(), project, task)
        .await?
        .targets
        .remove(id)
        .ok_or("任务目标结果不存在".into())
}
async fn require_success(
    state: &FormalAppState,
    project: &str,
    task: &str,
    id: &str,
    install: bool,
) -> TestResult<ScreenTargetResult> {
    let result = target_result(state, project, task, id).await?;
    require(
        state.task_repository.get(task).await?.state == TaskState::Succeeded
            && result.device == ResultState::Succeeded
            && result.business
                == if install {
                    ResultState::Succeeded
                } else {
                    ResultState::NotRequired
                }
            && result.shared == ResultState::Succeeded
            && !state.task_repository.results_protected(task).await?,
        "正式任务未形成已确认成功状态",
    )?;
    Ok(result)
}
fn config_input(id: &str) -> ScreenOperationInput {
    ScreenOperationInput {
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
    }
}
async fn config_task(
    state: &FormalAppState,
    auth: &Authorization,
    project: &str,
    id: &str,
    patch: AppConfigPatch,
) -> TestResult<String> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    identity(&device, auth).await?;
    patch.validate()?;
    let changes_audio =
        patch.set.contains_key(AUDIO) || patch.clear.iter().any(|name| name == AUDIO);
    let input = config_input(id);
    let preview = maintenance::preflight_with_config(
        state,
        project,
        input.clone(),
        Some(BTreeMap::from([(id.into(), patch)])),
    )
    .await?;
    require(
        preview.items.len() == 1 && preview.items[0].state == "ready",
        "配置预检未通过",
    )?;
    let task = maintenance::submit(state, project, &preview.id, input).await?;
    wait_task(state, &task).await?;
    let result = require_success(state, project, &task, id, false).await?;
    require(
        result.evidence["config"]["save"] == "saved"
            && result.evidence["config"]["readback"] == "succeeded"
            && result.evidence["config"]["restart"]
                == if changes_audio {
                    "succeeded"
                } else {
                    "not_required"
                },
        "名称及VAD任务未确认保存、按需重启和回读",
    )?;
    Ok(task)
}
async fn install_stamp(device: &AdbDevice, ip: &str) -> TestResult<String> {
    let path = device
        .shell(
            ip,
            &["pm", "path", "chat.xiaoxin.app"],
            CancellationToken::new(),
        )
        .await?;
    let package = device
        .shell(
            ip,
            &["dumpsys", "package", "chat.xiaoxin.app"],
            CancellationToken::new(),
        )
        .await?;
    let line = package
        .lines()
        .find(|line| line.trim().starts_with("lastUpdateTime="))
        .ok_or("未读到安装时间")?;
    Ok(format!("{}|{}", path.trim(), line.trim()))
}
async fn installation_command_count(state: &FormalAppState, task: &str) -> TestResult<usize> {
    let logs = inxaiot_desk_buddy_lib::interface::commands::task_activity::query_task_logs(
        state,
        task,
        &[],
        None,
        0,
        1000,
        false,
    )
    .await
    .map_err(|_| "读取本轮安装日志失败")?;
    // 只数安装命令提交的首条阶段记录，排除等待心跳和其他阶段。
    Ok(logs
        .items
        .iter()
        .filter(|e| {
            e.message.contains("正在安装小新")
                && e.message.contains("等待系统返回结果")
                && !e.message.contains("已等待")
        })
        .count())
}
async fn marker(admin: &MySqlPool, schema: &str, token: &str) -> TestResult<()> {
    require(
        regex::Regex::new(r"^inxaiot_desk_buddy_bfix[bw]_[a-f0-9]{32}$")?.is_match(schema),
        "随机库名称不符合本轮范围",
    )?;
    let actual: String = sqlx::query_scalar(&format!(
        "SELECT marker FROM `{schema}`.__codex_batch_recovery_scope WHERE id=1"
    ))
    .fetch_one(admin)
    .await?;
    require(actual == token, "测试库唯一标记不符，禁止清理")
}
async fn create_schema(admin: &MySqlPool, schema: &str, token: &str) -> TestResult<()> {
    require(
        regex::Regex::new(r"^inxaiot_desk_buddy_bfix[bw]_[a-f0-9]{32}$")?.is_match(schema),
        "随机库名称无效",
    )?;
    sqlx::query(&format!("CREATE DATABASE `{schema}` CHARACTER SET utf8mb4"))
        .execute(admin)
        .await?;
    sqlx::query(&format!("CREATE TABLE `{schema}`.__codex_batch_recovery_scope(id INT PRIMARY KEY,marker VARCHAR(64) NOT NULL) ENGINE=InnoDB")).execute(admin).await?;
    sqlx::query(&format!(
        "INSERT INTO `{schema}`.__codex_batch_recovery_scope(id,marker) VALUES(1,?)"
    ))
    .bind(token)
    .execute(admin)
    .await?;
    marker(admin, schema, token).await
}
async fn remove_failure_trigger(state: &FormalAppState) -> TestResult<()> {
    sqlx::query(&format!("DROP TRIGGER IF EXISTS {TRIGGER}"))
        .execute(state.local_store.pool())
        .await?;
    Ok(())
}
async fn restore_configuration(
    state: &FormalAppState,
    auth: &Authorization,
    report: &mut Value,
    directory: &Path,
) -> TestResult<()> {
    let project = report["projectId"]
        .as_str()
        .ok_or("恢复项目缺失")?
        .to_owned();
    let id = report["screenId"]
        .as_str()
        .ok_or("恢复屏标识缺失")?
        .to_owned();
    for task in state.task_repository.list_recent(&project, 100).await? {
        require(
            !matches!(
                task.state,
                TaskState::Queued | TaskState::Running | TaskState::Cancelling
            ) && !state.job_supervisor.contains(&task.id).await,
            "仍有运行任务，禁止并发恢复",
        )?;
        if task.state == TaskState::FinalizingFailed
            || state.task_repository.results_protected(&task.id).await?
        {
            tasks::verify(state, &project, &task.id).await?;
            require(
                !state.task_repository.results_protected(&task.id).await?,
                "原任务仍待核实，保留恢复责任",
            )?;
        }
    }
    let row = read_config(state, &project, &id).await?;
    let current = row.config.as_ref().unwrap();
    require(
        sha(&without_changed_fields(current)) == report["otherConfigSha256"],
        "非测试配置已发生变化，禁止覆盖",
    )?;
    require(
        stable_controls(&row) == report["originalControls"],
        "锁定/主屏/OWNER已外部变更，禁止覆盖",
    )?;
    let mut patch = AppConfigPatch::default();
    for key in [NAME, AUDIO] {
        let actual = field_value(current, key).ok_or("测试字段不可读取")?;
        let original = &report["originalValues"][key];
        require(
            actual == original || actual == &report["attemptedValues"][key],
            "测试字段已被外部修改，保留恢复责任",
        )?;
        if actual != original {
            if original.is_null() {
                patch.clear.push(key.into());
            } else {
                patch.set.insert(key.into(), original.clone());
            }
        }
    }
    phase(directory, report, "restore-configuration")?;
    if !patch.is_empty() {
        let task = config_task(state, auth, &project, &id, patch).await?;
        report["restoreTaskId"] = json!(task);
    }
    let restored = read_config(state, &project, &id).await?;
    require(
        sha(restored.config.as_ref().unwrap()) == report["allConfigSha256"]
            && stable_controls(&restored) == report["originalControls"],
        "配置或系统策略未恢复",
    )?;
    report["restoreRequired"] = json!(false);
    report["configurationRestored"] = json!(true);
    persist(directory, report)
}

async fn exercise(
    auth: &Authorization,
    directory: &Path,
    report: &mut Value,
    state: &Arc<FormalAppState>,
    platform: &TestPlatform,
) -> TestResult<()> {
    let admin = &platform.admin;
    let options = &platform.options;
    let business = &platform.business;
    let shared = &platform.shared;
    let device = AdbDevice::new(AndroidTools::discover()?);
    phase(directory, report, "verify-test-device")?;
    identity(&device, auth).await?;
    let package = apk::inspect(&auth.apk, CancellationToken::new()).await?;
    require(
        package.package_id == "chat.xiaoxin.app"
            && package.app_version == "2.1.3"
            && package.app_version_code == auth.code
            && package.sha256 == auth.apk_sha256,
        "安装包与目标版本不符",
    )?;
    report["apkSha256"] = json!(package.sha256);

    // 仅从142测试模板复制表结构，不复制任何平台业务行。
    for table in [
        "smart_terminal_screen",
        "t_project_building",
        "op_device_service_area",
    ] {
        sqlx::query(&format!(
            "CREATE TABLE `{business}`.`{table}` LIKE inxvision_iot_dev_demo.`{table}`"
        ))
        .execute(admin)
        .await?;
    }
    sqlx::query(&format!("INSERT INTO `{business}`.t_project_building(id,project_info_id,parent_id,area_name,area_level) VALUES(1000,777001,0,'批量修复验收空间',3)"))
        .execute(admin).await?;
    let temporary = ScreenAsset {
        id: "7001".into(),
        source: "local".into(),
        fields: ScreenFields {
            name: "批量修复验收屏".into(),
            ip: auth.ip.clone(),
            size: auth.size.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    let observed = device
        .inspect(&temporary, "inspect", CancellationToken::new())
        .await?;
    let mac = observed
        .observed_mac
        .as_deref()
        .filter(|value| valid_mac(value))
        .ok_or("未取得有效的产品MAC身份")?;
    require(
        observed.app_version_code == Some(auth.code),
        "实测应用版本与基线不符",
    )?;
    sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,app_version,version,status,delete_flag) VALUES(7001,'批量修复验收屏',?,?,?,1000,'隔离验收','before-install','keep-h5',0,0)"))
        .bind(&auth.ip).bind(mac).bind(format!("{}-inch", auth.size)).execute(admin).await?;
    if auth.cancel_inflight {
        let secondary = queued_secondary(auth);
        identity(&device, &secondary).await?;
        let asset = ScreenAsset {
            id: "7002".into(),
            source: "local".into(),
            fields: ScreenFields {
                name: "取消后不得执行的测试屏".into(),
                ip: secondary.ip.clone(),
                size: "4".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let observed = device
            .inspect(&asset, "inspect", CancellationToken::new())
            .await?;
        let mac = observed
            .observed_mac
            .as_deref()
            .filter(|value| valid_mac(value))
            .ok_or("第二台测试屏MAC不可确认")?;
        sqlx::query(&format!("INSERT INTO `{business}`.smart_terminal_screen(id,name,ip,mac,size,building_id,install_address,app_version,version,status,delete_flag) VALUES(7002,'取消后不得执行的测试屏',?,?,'4-inch',1000,'隔离验收','2.1.3','keep-h5',0,0)"))
            .bind(&secondary.ip).bind(mac).execute(admin).await?;
    }
    let ops = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(3)
        .connect_with(options.clone().database(shared))
        .await?;
    WorkbenchStore::new(ops.clone()).migrate().await?;

    let description = std::fs::read_to_string(root().join("test/测试数据说明.txt"))?;
    let api = description
        .lines()
        .find_map(|line| line.trim().strip_prefix("平台API："))
        .ok_or("缺少测试API")?
        .trim();
    let api = reqwest::Url::parse(&format!("http://{api}"))?;
    require(
        api.host_str() == Some("192.168.3.142")
            && api.username().is_empty()
            && api.password().is_none(),
        "只允许142测试API",
    )?;
    let start = description.find('{').ok_or("缺少测试登录")?;
    let end = start + description[start..].find('}').ok_or("测试登录格式无效")? + 1;
    let login: Value = serde_json::from_str(&description[start..end])?;
    let login_value = |key: &str| -> TestResult<String> {
        login[key]
            .as_str()
            .map(str::to_owned)
            .ok_or("测试登录字段缺失".into())
    };
    let cfg = &platform.cfg;
    let adapter = Stage75Adapter::new(state);
    let project = adapter
        .create_project(ProjectInput {
            name: "批量安装修复实机隔离".into(),
            platform_url: api.to_string(),
            db_host: cfg.host.clone(),
            db_port: cfg.port,
            db_user: cfg.username.clone(),
            db_tls_enabled: false,
            db_password: Some(cfg.password.clone()),
            business_db: business.clone(),
            workbench_db: shared.clone(),
        })
        .await?
        .project
        .id;
    report["projectId"] = json!(project);
    adapter
        .login_project(
            &project,
            PlatformLoginRequest {
                username: login_value("principal")?,
                password: login_value("credentials")?,
                session_uuid: login_value("sessionUUID")?,
                image_code: login_value("imageCode")?,
            },
        )
        .await?;
    let service = ScreenAssetsService::new(state);
    let snapshot = service.snapshot(&project, true).await?;
    require(
        snapshot.platform_available
            && snapshot.screens.len() == if auth.cancel_inflight { 2 } else { 1 },
        "隔离平台范围未就绪",
    )?;
    let screen = snapshot
        .screens
        .iter()
        .find(|screen| screen.id == "7001")
        .ok_or("主测试资产不存在")?;
    require(
        screen.id == "7001" && screen.source == "platform" && screen.fields.ip == auth.ip,
        "隔离资产范围不符",
    )?;
    let id = screen.id.clone();
    report["screenId"] = json!(id);
    let baseline = read_config(state, &project, &id).await?;
    let config = baseline.config.as_ref().unwrap();
    require(
        baseline.kiosk.as_ref().unwrap().enabled == Some(false)
            && baseline.kiosk.as_ref().unwrap().lock_task_mode.as_deref() == Some("none"),
        "仅从已解除锁定的原状态开始",
    )?;
    report["allConfigSha256"] = json!(sha(config));
    report["otherConfigSha256"] = json!(sha(&without_changed_fields(config)));
    report["originalControls"] = stable_controls(&baseline);
    report["h5HostPreserved"] = json!("192.168.3.44");
    report["originalValues"] = json!({NAME:field_value(config,NAME).ok_or("缺少名称")?,AUDIO:field_value(config,AUDIO).ok_or("缺少VAD阈值")?});
    let original_vad = field_value(config, AUDIO)
        .and_then(Value::as_f64)
        .ok_or("VAD阈值无效")?;
    let next_vad = if original_vad > 0.35 {
        original_vad - 0.05
    } else {
        original_vad + 0.05
    };
    report["attemptedValues"] = json!({NAME:format!("批量修复回归-{}",&uuid::Uuid::now_v7().to_string()[..8]),AUDIO:(next_vad*100.0).round()/100.0});
    if auth.cancel_inflight {
        let secondary = queued_secondary(auth);
        let row = read_config(state, &project, "7002").await?;
        require(
            row.kiosk.as_ref().unwrap().activity_ready == Some(false),
            "第二台屏当前不是原无主界面状态，取消模式不修改它",
        )?;
        report["secondaryBaseline"] = json!({"ip":secondary.ip,"serialSha256":sha(&json!(secondary.serial)),
            "versionCode":secondary.code,"allConfigSha256":sha(row.config.as_ref().unwrap()),
            "controls":stable_controls(&row),"activityReady":false,
            "installStampSha256":sha(&json!(install_stamp(&device,&secondary.ip).await?))});
    }
    persist(directory, report)?;

    if auth.inject_confirmation_failure {
        // 唯一隔离本机库中的固定测试触发器；只拦截安装版本的本机确认，不影响平台写入。
        sqlx::raw_sql(&format!("CREATE TRIGGER {TRIGGER} BEFORE INSERT ON local_screen_platform_cache WHEN EXISTS(SELECT 1 FROM local_screen_write_intent WHERE request_id=NEW.read_batch AND operation_type='install_version' AND state='submitted') BEGIN SELECT RAISE(FAIL,'fixture local confirmation unavailable'); END;"))
            .execute(state.local_store.pool()).await?;
        report["localFailureTriggerActive"] = json!(true);
        persist(directory, report)?;
    }
    phase(directory, report, "install-once")?;
    identity(&device, auth).await?;
    let input = ScreenOperationInput {
        action: "install".into(),
        target_ids: if auth.cancel_inflight {
            vec![id.clone(), "7002".into()]
        } else {
            vec![id.clone()]
        },
        application_id: Some("xiaoxin".into()),
        apk: Some(serde_json::to_value(&package)?),
        app_version: package.app_version.clone(),
        abi: "universal".into(),
        reinstall: true,
        concurrency: 1,
        expected_targets: Default::default(),
        retry_of_operation_id: None,
    };
    let preview = maintenance::preflight(state, &project, input.clone()).await?;
    require(
        preview.items.len() == if auth.cancel_inflight { 2 } else { 1 }
            && preview.items.iter().all(|item| item.state == "ready"),
        "正式安装预检未通过",
    )?;
    let task = maintenance::submit(state, &project, &preview.id, input).await?;
    report["installTaskId"] = json!(task);
    persist(directory, report)?;
    if cleanup_assertions_enabled() {
        require_install_artifact_retained(state, &project, &task, &auth.apk_sha256).await?;
        report["queuedInstallArtifactRetained"] = json!(true);
    }
    if auth.cancel_inflight {
        phase(directory, report, "wait-install-submission-boundary")?;
        tokio::time::timeout(Duration::from_secs(700), async {
            loop {
                let results =
                    task_data::read_results(state.local_store.pool(), &project, &task).await?;
                if results
                    .targets
                    .get(&id)
                    .is_some_and(|target| target.evidence["phase"] == "install_requested")
                {
                    require(
                        results.targets.get("7002").is_some_and(|target| {
                            target.device == ResultState::Pending
                                && target.evidence.get("phase").is_none()
                        }),
                        "第二台屏提前进入执行，取消边界不成立",
                    )?;
                    return Ok::<(), Box<dyn std::error::Error>>(());
                }
                let task_state = state.task_repository.get(&task).await?.state;
                require(
                    !task_state.is_terminal() && task_state != TaskState::FinalizingFailed,
                    "未观察到安装提交阶段，不把取消前已完成记为在途取消",
                )?;
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .map_err(|_| "未观察到安装提交临界点，保留原任务")??;
        if cleanup_assertions_enabled() {
            require_install_artifact_retained(state, &project, &task, &auth.apk_sha256).await?;
            report["inflightInstallArtifactRetained"] = json!(true);
        }
        let started = std::time::Instant::now();
        inxaiot_desk_buddy_lib::interface::commands::task_activity::request_task_cancel(
            state, &task,
        )
        .await
        .map_err(|_| "取消请求未返回，保留原任务")?;
        let elapsed = started.elapsed().as_millis();
        require(elapsed < 10000, "取消请求未及时返回")?;
        report["cancelRequestMilliseconds"] = json!(elapsed);
        report["cancelAfterInstallRequested"] = json!(true);
        persist(directory, report)?;
    }
    wait_task(state, &task).await?;
    let before_recovery_stamp = install_stamp(&device, &auth.ip).await?;
    let command_count = installation_command_count(state, &task).await?;
    require(command_count == 1, "安装命令阶段记录不唯一")?;
    if auth.inject_confirmation_failure {
        let pending = target_result(state, &project, &task, &id).await?;
        require(
            state.task_repository.get(&task).await?.state == TaskState::FinalizingFailed
                && pending.device == ResultState::Succeeded
                && pending.business == ResultState::Unknown
                && state.task_repository.results_protected(&task).await?,
            "本机故障没有保留真实安装成功及待核实边界",
        )?;
        if cleanup_assertions_enabled() {
            require_install_artifact_retained(state, &project, &task, &auth.apk_sha256).await?;
            report["pendingInstallArtifactRetained"] = json!(true);
        }
        let platform: String = sqlx::query_scalar(&format!(
            "SELECT app_version FROM `{business}`.smart_terminal_screen WHERE id=7001"
        ))
        .fetch_one(admin)
        .await?;
        require(platform == "2.1.3", "注入本机故障前平台写入没有成功")?;
        remove_failure_trigger(state).await?;
        report["localFailureTriggerActive"] = json!(false);
        phase(directory, report, "verify-existing-installation")?;
        tasks::verify(state, &project, &task).await?;
        let restored = require_success(state, &project, &task, &id, true).await?;
        require(
            !restored.message.contains("待处理")
                && !restored.message.contains("数据库操作失败")
                && restored.evidence["businessReceipt"]["verified"] == true,
            "恢复后仍保留旧错误说明或未使用核实回执",
        )?;
        require(
            install_stamp(&device, &auth.ip).await? == before_recovery_stamp
                && installation_command_count(state, &task).await? == command_count,
            "恢复过程中重新执行了安装",
        )?;
        report["recoveryDidNotReinstall"] = json!(true);
    } else if auth.cancel_inflight {
        let first = target_result(state, &project, &task, &id).await?;
        let second = target_result(state, &project, &task, "7002").await?;
        require(
            state.task_repository.get(&task).await?.state == TaskState::PartiallySucceeded
                && first.device == ResultState::Succeeded
                && first.business == ResultState::Succeeded
                && first.shared == ResultState::Succeeded
                && second.device == ResultState::Cancelled
                && second.business == ResultState::NotRequired
                && second.shared == ResultState::Succeeded
                && second.evidence.get("phase").is_none()
                && !state.task_repository.results_protected(&task).await?,
            "取消后已提交安装或未开始目标的结果不符合安全收尾",
        )?;
        let secondary = queued_secondary(auth);
        identity(&device, &secondary).await?;
        let read = read_config(state, &project, "7002").await?;
        require(
            sha(read.config.as_ref().unwrap()) == report["secondaryBaseline"]["allConfigSha256"]
                && stable_controls(&read) == report["secondaryBaseline"]["controls"]
                && read.kiosk.as_ref().unwrap().activity_ready == Some(false)
                && sha(&json!(install_stamp(&device, &secondary.ip).await?))
                    == report["secondaryBaseline"]["installStampSha256"],
            "第二台未执行屏的配置、运行状态或安装状态发生变化",
        )?;
        let second_intents:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_write_intent WHERE screen_id='7002' AND operation_type='install_version'")
            .fetch_one(state.local_store.pool()).await?;
        require(second_intents == 0, "未开始屏产生了安装版本写入意图")?;
        report["cancelledQueuedScreenUnchanged"] = json!(true);
        report["inflightInstallationCompletedOnce"] = json!(true);
    } else {
        require_success(state, &project, &task, &id, true).await?;
    }
    if cleanup_assertions_enabled() {
        require_install_cleanup_and_evidence(state, &project, &task, auth).await?;
        report["successfulInstallArtifactRemoved"] = json!(true);
        report["installMetadataResultsLogsAndSourcePreserved"] = json!(true);
        persist(directory, report)?;
    }
    let versions: (String, String) = sqlx::query_as(&format!(
        "SELECT app_version,version FROM `{business}`.smart_terminal_screen WHERE id=7001"
    ))
    .fetch_one(admin)
    .await?;
    require(
        versions == ("2.1.3".into(), "keep-h5".into()),
        "应用版本回写或H5版本边界不正确",
    )?;
    let confirmed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_write_intent WHERE operation_type='install_version' AND state='confirmed'")
        .fetch_one(state.local_store.pool()).await?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    require(
        confirmed == 1 && repo.asset(&project, &id).await?.app_version.as_deref() == Some("2.1.3"),
        "本机意图或平台缓存未确认",
    )?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM operation_target_result WHERE operation_id=?")
            .bind(&task)
            .fetch_one(&ops)
            .await?;
    require(
        count == if auth.cancel_inflight { 2 } else { 1 },
        "共享结果未按原任务每目标保存一次",
    )?;
    report["installPassed"] = json!(true);
    report["installCommandCount"] = json!(command_count);
    let after_install = read_config(state, &project, &id).await?;
    require(
        after_install.config == baseline.config
            && stable_controls(&after_install) == report["originalControls"],
        "覆盖安装改变了原配置或系统策略",
    )?;

    phase(directory, report, "configuration-roundtrip")?;
    report["restoreRequired"] = json!(true);
    persist(directory, report)?;
    let mut patch = AppConfigPatch::default();
    for key in [NAME, AUDIO] {
        patch
            .set
            .insert(key.into(), report["attemptedValues"][key].clone());
    }
    let changed = config_task(state, auth, &project, &id, patch).await?;
    report["configurationTaskId"] = json!(changed);
    let actual = read_config(state, &project, &id).await?;
    require(
        sha(&without_changed_fields(actual.config.as_ref().unwrap()))
            == report["otherConfigSha256"]
            && stable_controls(&actual) == report["originalControls"],
        "配置修改波及非测试字段或系统策略",
    )?;
    for key in [NAME, AUDIO] {
        require(
            field_value(actual.config.as_ref().unwrap(), key)
                == Some(&report["attemptedValues"][key]),
            "配置未按本次字段生效",
        )?;
    }
    restore_configuration(state, auth, report, directory).await?;
    identity(&device, auth).await?;
    if cleanup_assertions_enabled() {
        phase(directory, report, "diagnostics-and-startup-cleanup")?;
        report["diagnosticTaskId"] =
            json!(diagnostic_survives_startup_cleanup(state, &project, &id, directory).await?);
        report["realDiagnosticReadExportAndLogSurviveStartupCleanup"] = json!(true);
    }
    report["functionalPassed"] = json!(true);
    phase(directory, report, "functional-verified")?;
    ops.close().await;
    Ok(())
}

#[tokio::test]
#[ignore = "显式授权42或82指定硬件、同版正式APK、142唯一随机库；安装一次并配置往返恢复，不访问生产库"]
async fn registered_install_and_configuration_roundtrip_with_optional_local_confirmation_recovery()
-> TestResult<()> {
    let auth = authorization()?;
    let cfg = project_test_config::database();
    require(
        cfg.host == "192.168.3.142" && cfg.port == 3306,
        "仅允许142测试数据库",
    )?;
    let directory = root()
        .join(".review-tools/batch-install-fix-20261024")
        .join(format!("real-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&directory)?;
    let token = uuid::Uuid::now_v7().simple().to_string();
    let business = format!("inxaiot_desk_buddy_bfixb_{token}");
    let shared = format!("inxaiot_desk_buddy_bfixw_{token}");
    let mut report = json!({"passed":false,"phase":"created","target":auth.ip,"serialSha256":sha(&json!(auth.serial)),
        "versionCode":auth.code,"mode":if auth.cancel_inflight {"cancel-inflight"} else if auth.inject_confirmation_failure {"local-confirm-failure"} else {"normal"},
        "businessSchema":business,"sharedSchema":shared,"schemaMarker":token,"restoreRequired":false,
        "schemasRemoved":false,"localFailureTriggerActive":false,"productionDataUsed":false});
    persist(&directory, &report)?;
    println!("隔离证据：{}", directory.display());
    let options = sqlx::mysql::MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(cfg.port)
        .username(&cfg.username)
        .password(&cfg.password)
        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
        .disable_statement_logging();
    let admin = sqlx::mysql::MySqlPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(options.clone())
        .await?;
    create_schema(&admin, &business, &token).await?;
    create_schema(&admin, &shared, &token).await?;
    let state = support::state_at(&directory.join("local"), true).await;
    let platform = TestPlatform {
        cfg,
        options,
        admin,
        business,
        shared,
    };
    let outcome = exercise(&auth, &directory, &mut report, &state, &platform).await;
    if let Err(error) = &outcome {
        report["passed"] = json!(false);
        report["failedPhase"] = report["phase"].clone();
        report["failureReason"] = json!(safe_error(error.as_ref()));
        persist(&directory, &report)?;
        if report["localFailureTriggerActive"] == true {
            report["localFailureTriggerRemovedAfterFailure"] =
                json!(remove_failure_trigger(&state).await.is_ok());
        }
        if report["restoreRequired"] == true {
            let recovery = restore_configuration(&state, &auth, &mut report, &directory).await;
            report["automaticRestorePassed"] = json!(recovery.is_ok());
            if let Err(error) = recovery {
                report["restoreFailure"] = json!(safe_error(error.as_ref()));
            }
        }
    }
    // 有失败则保留隔离库和恢复依据，绝不为了清库重放安装或覆盖未知设备配置。
    support::close(state).await;
    if outcome.is_ok() && report["restoreRequired"] == false {
        for schema in [&platform.shared, &platform.business] {
            marker(&platform.admin, schema, &token).await?;
            sqlx::query(&format!("DROP DATABASE `{schema}`"))
                .execute(&platform.admin)
                .await?;
        }
        report["schemasRemoved"] = json!(true);
        report["passed"] = json!(true);
        report["phase"] = json!("completed");
    }
    platform.admin.close().await;
    persist(&directory, &report)?;
    if outcome.is_err() {
        return Err(format!(
            "实机阶段未通过，证据及恢复责任已保留：{}",
            directory.display()
        )
        .into());
    }
    println!(
        "正式安装、配置往返、平台/本机确认及共享结果通过；故障恢复模式={}，原配置和44H5保留。",
        auth.inject_confirmation_failure
    );
    Ok(())
}
