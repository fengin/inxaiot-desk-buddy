//! 阶段B正式配置回归；必须由主操作者在阶段A交接后显式运行。
//! 仅新建本机项目和持久证据目录，不登录平台、不使用项目数据库、不清数据或撤销Owner。
#[path = "common/screen_test_support.rs"]
mod support;

use futures_util::{StreamExt, stream};
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::AppResult,
    domain::{
        common::task::TaskState,
        smart_screen::{
            app_config::{AppConfigPatch, AppConfigRead, field_value},
            app_config_schema::{ConfigSchema, FieldType, values_equal},
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
            home, home_activation, maintenance, sleep_policy, task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const TARGETS: [(&str, &str); 3] = [
    ("192.168.3.74", "4"),
    ("192.168.3.82", "4"),
    ("192.168.3.42", "10"),
];
const ENVIRONMENTS: [&str; 3] = ["test", "pre", "prod"];
const ADDRESS_FIELDS: [&str; 3] = ["otaUrl", "wsUrl", "h5Url"];
const AUDIO: [&str; 8] = [
    "micSampleRate",
    "vadThreshold",
    "minSilenceDuration",
    "minSpeechDuration",
    "vadWindowSize",
    "vadPreCacheDurationMs",
    "keywordsScore",
    "keywordsThreshold",
];
const REMAINING_SOURCE_CASE: &str = "run-01a1a53b-8163-71d3-a701-8ca9e48d8270";
const CONTROLS_SOURCE_CASE: &str = "run-01a1a551-4dae-7c61-b33f-c6fa019d4902";
const AWAKE_CONTROLS_SOURCE_CASE: &str = "run-01a1a599-1aac-7581-bc42-95d553ffb3c3";

fn require(value: bool, message: &str) -> TestResult<()> {
    if value {
        Ok(())
    } else {
        Err(message.to_string().into())
    }
}
fn safe_error(error: &dyn std::fmt::Display) -> String {
    let text = error.to_string();
    let text = regex::Regex::new(r#"(?i)(?:https?|wss?)://[^\s\"'<>]+"#)
        .unwrap()
        .replace_all(&text, "[地址已隐藏]");
    let text = regex::Regex::new(r"(?i)(payload|result)\s*(?:=|:s:)\s*[A-Za-z0-9+/=]{20,}")
        .unwrap()
        .replace_all(&text, "[配置报文已隐藏]");
    regex::Regex::new(
        r"(?i)(password|passwd|token|secret|authorization|authcode|authkey)\s*[:=]\s*[^\s,;]+",
    )
    .unwrap()
    .replace_all(&text, "[凭据已隐藏]")
    .chars()
    .take(1200)
    .collect()
}

#[derive(Clone, Serialize, Deserialize)]
struct Target {
    ip: String,
    size: String,
    serial: String,
    supplied_mac: String,
    wired_mac: String,
    phase_a_initial_wired_mac: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Baseline {
    target: Target,
    id: String,
    read: AppConfigRead,
    writable: Vec<String>,
    missing_fields: Vec<String>,
    boot: String,
    app_pids: String,
    task_kind: String,
    wakefulness: String,
    display: String,
    system_settings: BTreeMap<String, String>,
}
struct Context {
    state: Arc<FormalAppState>,
    device: AdbDevice,
    directory: PathBuf,
    project: String,
    baselines: Vec<Baseline>,
    attempts: BTreeMap<String, BTreeMap<String, Vec<Value>>>,
    report: Value,
}
fn write_json(path: &Path, value: &impl Serialize) -> TestResult<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("证据目录缺失")?)?;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
impl Context {
    fn persist(&self) -> TestResult<()> {
        write_json(
            &self.directory.join("attempts-private.json"),
            &self.attempts,
        )?;
        write_json(&self.directory.join("result.json"), &self.report)?;
        Ok(())
    }
    fn uncovered(&mut self, id: &str, phase: &str, reason: &str) {
        self.report["uncovered"]
            .as_array_mut()
            .unwrap()
            .push(json!({"screenId":id,"phase":phase,"reason":reason}));
    }
    fn record(&mut self, id: &str, phase: &str, detail: Value) {
        self.report["checks"]
            .as_array_mut()
            .unwrap()
            .push(json!({"screenId":id,"phase":phase,"result":detail}));
    }
    fn remember(&mut self, id: &str, patch: &AppConfigPatch) -> TestResult<()> {
        let fields = self.attempts.entry(id.into()).or_default();
        for (field, value) in &patch.set {
            fields.entry(field.clone()).or_default().push(value.clone());
        }
        for field in &patch.clear {
            fields.entry(field.clone()).or_default().push(Value::Null);
        }
        for (field, value) in [
            ("$home", patch.home_enabled),
            ("$kiosk", patch.kiosk_enabled),
        ] {
            if let Some(value) = value {
                fields.entry(field.into()).or_default().push(json!(value));
            }
        }
        self.report["restoreRequired"] = json!(true);
        self.persist()
    }
}

fn authorized_targets() -> TestResult<Vec<Target>> {
    require(
        std::env::var("INX_APP_CONFIG_REGRESSION_REAL").as_deref() == Ok("YES"),
        "阶段B未获显式执行门禁；未连接设备",
    )?;
    require(
        std::env::var("INX_APP_CONFIG_REGRESSION_HANDOFF").as_deref() == Ok("PHASE_A_FINISHED"),
        "阶段A尚未交接；未连接设备",
    )?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()?;
    let path = PathBuf::from(
        std::env::var("INX_APP_CONFIG_REGRESSION_PHASE_A_BASELINE")
            .map_err(|_| "缺少阶段A基线路径")?,
    )
    .canonicalize()?;
    require(
        path.starts_with(
            root.join(".review-tools/combined-real-20261104")
                .canonicalize()?,
        ) && path.file_name().is_some_and(|name| name == "baseline.json"),
        "阶段A基线不属于本轮受控证据目录",
    )?;
    let value: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    require(
        value["readOnly"] == true && value["deviceWrites"] == 0,
        "阶段A身份基线不是只读记录",
    )?;
    let rows = value["targets"].as_array().ok_or("阶段A缺少三屏身份")?;
    require(rows.len() == 3, "阶段A身份范围不是三屏")?;
    let final_path = PathBuf::from(
        std::env::var("INX_APP_CONFIG_REGRESSION_PHASE_A_RESULT")
            .map_err(|_| "缺少阶段A最终收尾结果路径，未建立阶段B写基线")?,
    )
    .canonicalize()?;
    require(
        final_path.starts_with(
            root.join(".review-tools/combined-real-20261104")
                .canonicalize()?,
        ) && final_path
            .file_name()
            .is_some_and(|name| name == "result.json"),
        "阶段A收尾文件不属于本轮证据目录",
    )?;
    let final_result: Value = serde_json::from_slice(&std::fs::read(final_path)?)?;
    require(
        final_result["restored"] == true,
        "阶段A尚未成功收尾，禁止建立阶段B写基线",
    )?;
    let restored = final_result["restoration"]
        .as_array()
        .ok_or("阶段A没有逐屏收尾证明")?;
    let mut targets = Vec::new();
    for (ip, size) in TARGETS {
        let row = rows
            .iter()
            .find(|row| row["ip"] == ip && row["size"] == size)
            .ok_or("阶段A身份缺少目标")?;
        let read = |key: &str| -> TestResult<String> {
            require(row["commands"][key]["exit"] == 0, "阶段A身份命令未成功")?;
            Ok(row["commands"][key]["stdout"]
                .as_str()
                .ok_or("阶段A身份值缺失")?
                .trim()
                .into())
        };
        let supplied_mac = row["suppliedMac"]
            .as_str()
            .filter(|v| valid_mac(v))
            .ok_or("阶段A用户MAC不可靠")?;
        let wifi = read("wifiMac")?;
        let initial_wired_mac = read("wiredMac")?;
        let serial = read("serial")?;
        let final_row = restored
            .iter()
            .find(|row| row["ip"] == ip && row["restored"] == true)
            .ok_or("阶段A有屏尚未恢复，禁止进入阶段B")?;
        require(
            final_row["after"]["serial"] == serial,
            "阶段A最终物理屏身份与初始序列号不符",
        )?;
        let wired_mac = final_row["after"]["wiredMac"]
            .as_str()
            .filter(|value| valid_mac(value))
            .ok_or("阶段A收尾没有可靠当前有线MAC")?;
        require(
            !serial.is_empty()
                && valid_mac(&wired_mac)
                && valid_mac(&initial_wired_mac)
                && normalize_mac(&wifi) == normalize_mac(supplied_mac),
            "阶段A实测身份与用户MAC不一致",
        )?;
        targets.push(Target {
            ip: ip.into(),
            size: size.into(),
            serial,
            supplied_mac: normalize_mac(supplied_mac),
            wired_mac: normalize_mac(&wired_mac),
            phase_a_initial_wired_mac: normalize_mac(&initial_wired_mac),
        });
    }
    Ok(targets)
}
async fn physical_identity(device: &AdbDevice, target: &Target) -> TestResult<()> {
    let cancel = CancellationToken::new();
    device.connect(&target.ip, cancel.clone()).await?;
    let serial = device
        .shell(&target.ip, &["getprop", "ro.serialno"], cancel.clone())
        .await?;
    require(
        serial.trim() == target.serial,
        "设备序列号与阶段A不一致，未继续",
    )?;
    let addresses = device.shell(&target.ip, &["for p in /sys/class/net/*; do if [ -r \"$p/address\" ]; then cat \"$p/address\"; fi; done"], cancel).await?;
    require(
        addresses
            .lines()
            .any(|mac| normalize_mac(mac.trim()) == target.supplied_mac),
        "用户给定MAC未出现在该屏网卡中，未继续",
    )
}
async fn identity(device: &AdbDevice, target: &Target) -> TestResult<()> {
    physical_identity(device, target).await?;
    let wired = device
        .shell(
            &target.ip,
            &["cat", "/sys/class/net/eth0/address"],
            CancellationToken::new(),
        )
        .await?;
    require(
        normalize_mac(wired.trim()) == target.wired_mac,
        "有线MAC与阶段B初始身份不一致，停止正常配置与QA界面准备",
    )
}
fn real_sdk_tools() -> TestResult<AndroidTools> {
    let specified = PathBuf::from(
        std::env::var_os("INX_ADB_PATH")
            .ok_or("阶段B必须显式INX_ADB_PATH指向真实SDK，不能沿用阶段A审计包装器")?,
    )
    .canonicalize()?;
    require(
        specified.file_name().is_some_and(|name| name == "adb.exe")
            && specified
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "platform-tools")
            && !specified.to_string_lossy().contains(".review-tools"),
        "阶段B只允许SDK platform-tools/adb.exe，不记录可能含原地址的AppConfig命令参数",
    )?;
    let tools = AndroidTools::discover()?;
    require(
        tools.adb.canonicalize()? == specified,
        "实际ADB工具与本轮显式SDK不一致",
    )?;
    Ok(tools)
}
async fn runtime(device: &AdbDevice, ip: &str) -> TestResult<(String, String)> {
    let pid = device
        .shell(ip, &["pidof", "chat.xiaoxin.app"], CancellationToken::new())
        .await?;
    let boot = device
        .shell(
            ip,
            &["cat", "/proc/sys/kernel/random/boot_id"],
            CancellationToken::new(),
        )
        .await?;
    require(
        uuid::Uuid::parse_str(boot.trim()).is_ok(),
        "设备启动编号不可确认",
    )?;
    Ok((pid.trim().into(), boot.trim().into()))
}
async fn system_settings(device: &AdbDevice, ip: &str) -> TestResult<BTreeMap<String, String>> {
    let mut settings = BTreeMap::new();
    for (namespace, key) in [
        ("global", "stay_on_while_plugged_in"),
        ("system", "screen_off_timeout"),
        ("secure", "sleep_timeout"),
        ("secure", "screensaver_enabled"),
        ("secure", "screensaver_activate_on_sleep"),
        ("secure", "screensaver_activate_on_dock"),
        ("system", "screen_brightness"),
        ("system", "screen_brightness_mode"),
    ] {
        settings.insert(
            format!("{namespace}.{key}"),
            device
                .shell(
                    ip,
                    &["settings", "get", namespace, key],
                    CancellationToken::new(),
                )
                .await?,
        );
    }
    Ok(settings)
}
async fn read_one(context: &Context, baseline: &Baseline) -> TestResult<AppConfigRead> {
    identity(&context.device, &baseline.target).await?;
    let rows = app_config::read(&context.state, &context.project, &[baseline.id.clone()]).await?;
    require(
        rows.len() == 1 && rows[0].screen_id == baseline.id && rows[0].config.is_some(),
        "正式配置读取未取得唯一可靠结果",
    )?;
    Ok(rows.into_iter().next().unwrap())
}
fn input(ids: Vec<String>, concurrency: u32) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "app_config".into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency,
        expected_targets: Default::default(),
        retry_of_operation_id: None,
    }
}
fn patch_changed(patch: &AppConfigPatch, row: &AppConfigRead) -> bool {
    !row.config
        .as_ref()
        .is_some_and(|value| patch.matches(value))
        || patch.home_enabled.is_some_and(|value| {
            !row.home.as_ref().is_some_and(|home| {
                home.enabled == Some(value) && (!value || home.active == Some(true))
            })
        })
        || patch.kiosk_enabled.is_some_and(|value| {
            !row.kiosk.as_ref().is_some_and(|kiosk| {
                kiosk.configured == Some(true)
                    && kiosk.enabled == Some(value)
                    && kiosk.active == Some(value)
            })
        })
        || patch.ensure_device_owner == Some(true)
            && !row
                .owner
                .as_ref()
                .is_some_and(|owner| owner.state == "self")
}
async fn local_task_count(context: &Context) -> TestResult<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_task WHERE local_project_id=? AND domain_type='smart_screen'",
    )
    .bind(&context.project)
    .fetch_one(context.state.local_store.pool())
    .await?)
}
async fn target_task_ids(context: &Context, id: &str) -> TestResult<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT DISTINCT t.id FROM local_task t JOIN local_task_target target ON target.local_task_id=t.id WHERE t.local_project_id=? AND t.domain_type='smart_screen' AND t.operation_type='app_config' AND target.resource_type='smart_screen' AND target.resource_key=? ORDER BY t.created_at,t.id")
        .bind(&context.project).bind(id).fetch_all(context.state.local_store.pool()).await?)
}
async fn settle(context: &Context, id: &str) -> TestResult<()> {
    for task_id in target_task_ids(context, id).await? {
        let task = context.state.task_repository.get(&task_id).await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            let current = context.state.task_repository.get(&task.id).await?;
            if !context.state.job_supervisor.contains(&task.id).await
                && !matches!(
                    current.state,
                    TaskState::Queued | TaskState::Running | TaskState::Cancelling
                )
            {
                break;
            }
            require(
                tokio::time::Instant::now() < deadline,
                "原配置任务尚未收尾，未并发恢复或重放",
            )?;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let results =
            task_data::read_results(context.state.local_store.pool(), &context.project, &task.id)
                .await?;
        if results
            .targets
            .get(id)
            .is_some_and(|row| matches!(row.device, ResultState::Pending | ResultState::Unknown))
            || context
                .state
                .task_repository
                .results_protected(&task.id)
                .await?
        {
            tasks::verify(&context.state, &context.project, &task.id).await?;
            let results = task_data::read_results(
                context.state.local_store.pool(),
                &context.project,
                &task.id,
            )
            .await?;
            require(
                !context
                    .state
                    .task_repository
                    .results_protected(&task.id)
                    .await?
                    && results.targets.get(id).is_none_or(|row| {
                        !matches!(row.device, ResultState::Pending | ResultState::Unknown)
                    }),
                "原结果仍待核实，保留恢复责任，未重放",
            )?;
        }
    }
    Ok(())
}
async fn submit(
    context: &mut Context,
    baseline: &Baseline,
    phase: &str,
    patch: AppConfigPatch,
) -> TestResult<Option<ScreenTargetResult>> {
    let mut unsafe_fields = patch
        .fields()
        .into_iter()
        .filter_map(|field| restoration_issue(baseline, &field).map(|reason| (field, reason)))
        .collect::<Vec<_>>();
    if patch.home_enabled.is_some()
        && baseline
            .read
            .home
            .as_ref()
            .and_then(|home| home.enabled)
            .is_none()
    {
        unsafe_fields.push((
            "homeEnabled".into(),
            "原主屏策略未确认，不能精确恢复".into(),
        ));
    }
    if patch.kiosk_enabled.is_some() && !kiosk_pair_reversible(&baseline.read) {
        unsafe_fields.push((
            "kioskEnabled".into(),
            "原锁定键缺失或值未确认，不能建立不可逆新键".into(),
        ));
    }
    if patch.ensure_device_owner == Some(true)
        && !baseline
            .read
            .owner
            .as_ref()
            .is_some_and(|owner| owner.state == "self")
    {
        unsafe_fields.push((
            "ensureDeviceOwner".into(),
            "仅核实既有self，不为测试新建Owner".into(),
        ));
    }
    if !unsafe_fields.is_empty() {
        for (field, reason) in &unsafe_fields {
            context.uncovered(&baseline.id, field, reason);
        }
        context.record(&baseline.id,phase,json!({"state":"uncovered","reason":"原值不可由正式协议精确恢复，未提交","fields":unsafe_fields.iter().map(|(field,_)|field).collect::<Vec<_>>(),"executionTasksCreated":0}));
        context.persist()?;
        return Ok(None);
    }
    patch.validate()?;
    settle(context, &baseline.id).await?;
    let before = read_one(context, baseline).await?;
    if patch.ensure_device_owner == Some(true) {
        require(
            patch.fields().is_empty()
                && patch.home_enabled.is_none()
                && patch.kiosk_enabled.is_none(),
            "QA Owner只读检查不与其它补丁混用",
        )?;
        if before
            .owner
            .as_ref()
            .is_some_and(|owner| owner.state == "self")
        {
            context.record(&baseline.id,phase,json!({"state":"read_only_confirmed","currentOwner":"self","ownerWriteRequested":false,"executionTasksCreated":0}));
        } else {
            context.uncovered(
                &baseline.id,
                "ensureDeviceOwner",
                "当前Owner不再为self，记录状态变化，绝不发送设置Owner补丁",
            );
        }
        context.persist()?;
        return Ok(None);
    }
    let task_count = local_task_count(context).await?;
    if !patch_changed(&patch, &before) {
        require(
            local_task_count(context).await? == task_count,
            "同值过滤意外产生任务",
        )?;
        context.record(
            &baseline.id,
            phase,
            json!({"passed":true,"evidenceSource":"qa_difference_filter","productPreflightSkipVerified":false,"realDesktopClickVerified":false,"executionTasksCreated":0}),
        );
        context.persist()?;
        return Ok(None);
    }
    let restart = patch.restart_required_with_capabilities(
        before.config.as_ref().unwrap(),
        before.capabilities.as_ref().ok_or("当前字段能力未确认")?,
    );
    let creates_activity =
        restart || patch.home_enabled == Some(true) || patch.kiosk_enabled == Some(true);
    if creates_activity && !["home", "standard"].contains(&baseline.task_kind.as_str()) {
        context.uncovered(
            &baseline.id,
            phase,
            "原本应用任务类型为absent或未确认；不能精确恢复，未执行重启或激活阶段",
        );
        context.persist()?;
        return Ok(None);
    }
    if creates_activity
        && baseline.read.kiosk.as_ref().is_some_and(|kiosk| {
            kiosk.enabled.is_some() && kiosk.active.is_some() && kiosk.enabled != kiosk.active
        })
    {
        context.uncovered(
            &baseline.id,
            phase,
            "原锁定保存与运行状态不一致，活动重建后无法精确还原，未执行该阶段",
        );
        context.persist()?;
        return Ok(None);
    }
    let request = input(vec![baseline.id.clone()], 1);
    let preview = maintenance::preflight_with_config(
        &context.state,
        &context.project,
        request.clone(),
        Some(BTreeMap::from([(baseline.id.clone(), patch.clone())])),
    )
    .await?;
    require(
        preview.items.len() == 1 && preview.items[0].state == "ready",
        "正式预检不允许本阶段设置，未提交任务",
    )?;
    context.remember(&baseline.id, &patch)?;
    let (pid, boot) = runtime(&context.device, &baseline.target.ip).await?;
    let task = tasks::submit(&context.state, &context.project, &preview.id, request).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let current = context.state.task_repository.get(&task).await?;
            if current.state.is_terminal() || current.state == TaskState::FinalizingFailed {
                return Ok::<(), inxaiot_desk_buddy_lib::core::error::AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    })
    .await??;
    settle(context, &baseline.id).await?;
    let result = task_data::read_results(context.state.local_store.pool(), &context.project, &task)
        .await?
        .targets
        .remove(&baseline.id)
        .ok_or("任务缺少本屏结果")?;
    require(
        result.business == ResultState::NotRequired && result.shared == ResultState::NotRequired,
        "本机配置任务出现非本机结果归属，停止测试",
    )?;
    if !matches!(result.device, ResultState::Succeeded | ResultState::Skipped) {
        context.record(&baseline.id,"formal_task_failed",json!({"taskId":task,"selectedPhase":phase,"device":result.device,"fields":patch.fields(),
            "message":safe_error(&result.message),"evidencePhase":result.evidence["phase"],"save":result.evidence["config"]["save"],"restart":result.evidence["config"]["restart"],"readback":result.evidence["config"]["readback"]}));
        context.persist()?;
        return Err(format!("正式配置任务未成功：{}", safe_error(&result.message)).into());
    }
    let after = read_one(context, baseline).await?;
    let config = after.config.as_ref().unwrap();
    require(patch.matches(config), "独立配置回读与目标不同")?;
    let (after_pid, after_boot) = runtime(&context.device, &baseline.target.ip).await?;
    require(boot == after_boot, "小新配置操作意外重启系统")?;
    if let Some(value) = patch.home_enabled {
        require(
            after.home.as_ref().is_some_and(|home| {
                home.enabled == Some(value) && (!value || home.active == Some(true))
            }),
            "主屏保存或激活回读未通过",
        )?;
    }
    if let Some(value) = patch.kiosk_enabled {
        require(
            after.kiosk.as_ref().is_some_and(|kiosk| {
                kiosk.configured == Some(true)
                    && kiosk.enabled == Some(value)
                    && kiosk.active == Some(value)
            }),
            "锁定保存或运行回读未通过",
        )?;
    }
    let restart = patch.restart_required_with_capabilities(
        before.config.as_ref().unwrap(),
        before.capabilities.as_ref().ok_or("缺少实际字段能力")?,
    );
    require(
        if restart {
            pid != after_pid && result.evidence["config"]["restart"] == "succeeded"
        } else {
            result.evidence["config"]["restart"] == "not_required"
        },
        "应用重启结果不符合本阶段字段规则",
    )?;
    context.record(&baseline.id, phase, json!({"passed":true,"taskId":task,"fields":patch.fields(),"restartRequired":restart,"applicationPidChanged":pid!=after_pid,"systemBootUnchanged":true,"platformBusinessAndSharedWrites":false}));
    context.persist()?;
    Ok(Some(result))
}

fn selected_fields(row: &AppConfigRead) -> TestResult<Vec<String>> {
    if let Some(schema) =
        ConfigSchema::from_capabilities(row.capabilities.as_ref().ok_or("缺少配置能力")?)?
    {
        return Ok(schema
            .fields
            .iter()
            .filter(|field| field.writable)
            .map(|field| field.key.clone())
            .collect());
    }
    let mut fields = vec!["customDeviceName".into(), "environments.current".into()];
    if row
        .config
        .as_ref()
        .unwrap()
        .get("autoStartVoiceEngine")
        .is_some()
    {
        fields.push("autoStartVoiceEngine".into());
    }
    for environment in ENVIRONMENTS {
        for field in ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"] {
            fields.push(format!("environments.{environment}.{field}"));
        }
    }
    Ok(fields)
}
fn restoration_issue(baseline: &Baseline, field: &str) -> Option<String> {
    let Some(config) = baseline.read.config.as_ref() else {
        return Some("原配置没有可靠快照".into());
    };
    let Some(value) = field_value(config, field) else {
        return Some("原字段缺失，不能把缺失当作null清空恢复".into());
    };
    if !baseline.writable.iter().any(|key| key == field) {
        return Some("设备未声明该实际字段可写，未修改".into());
    }
    let patch = if value.is_null() {
        AppConfigPatch {
            clear: vec![field.into()],
            ..Default::default()
        }
    } else {
        set(field, value.clone())
    };
    if patch.validate().is_err() {
        return Some("原值不能通过正式补丁恢复，未改写该字段".into());
    }
    match baseline
        .read
        .capabilities
        .as_ref()
        .map(ConfigSchema::from_capabilities)
    {
        Some(Ok(Some(schema))) if schema.validate_patch(&patch, config).is_err() => {
            Some("原值不满足屏当前写入定义，不能精确恢复".into())
        }
        Some(Err(_)) | None => Some("设备字段能力未确认，未修改".into()),
        _ => None,
    }
}
fn restore_patch(
    original: &Value,
    current: &Value,
    fields: &[String],
    attempts: &BTreeMap<String, Vec<Value>>,
) -> TestResult<AppConfigPatch> {
    let mut patch = AppConfigPatch::default();
    for field in fields {
        let original = field_value(original, field).ok_or("恢复原字段缺失")?;
        let actual = field_value(current, field).ok_or("恢复当前字段缺失")?;
        if values_equal(original, actual) {
            continue;
        }
        require(
            attempts
                .get(field)
                .is_some_and(|values| values.iter().any(|value| values_equal(value, actual))),
            "实际值既不是原值也不是本轮目标，未覆盖外部修改",
        )?;
        if original.is_null() {
            patch.clear.push(field.clone());
        } else {
            patch.set.insert(field.clone(), original.clone());
        }
    }
    patch.validate()?;
    Ok(patch)
}
async fn restore_fields(
    context: &mut Context,
    baseline: &Baseline,
    fields: &[String],
) -> TestResult<()> {
    let current = read_one(context, baseline).await?;
    let mut known = Vec::new();
    for field in fields {
        if field_value(baseline.read.config.as_ref().unwrap(), field).is_none() {
            context.uncovered(
                &baseline.id,
                field,
                "原字段缺失，保留缺失；不生成null或清空恢复请求",
            );
            require(
                field_value(current.config.as_ref().unwrap(), field).is_none(),
                "原缺失字段被外部补入，未猜测删除",
            )?;
        } else {
            known.push(field.clone());
        }
    }
    let empty = BTreeMap::new();
    let patch = restore_patch(
        baseline.read.config.as_ref().unwrap(),
        current.config.as_ref().unwrap(),
        &known,
        context.attempts.get(&baseline.id).unwrap_or(&empty),
    )?;
    if !patch.is_empty() {
        submit(context, baseline, "restore_fields", patch).await?;
    }
    Ok(())
}
fn set(field: &str, value: Value) -> AppConfigPatch {
    AppConfigPatch {
        set: BTreeMap::from([(field.into(), value)]),
        ..Default::default()
    }
}

#[derive(Default)]
struct ReadCounts {
    active: AtomicUsize,
    peak: AtomicUsize,
}
struct ActiveRead(Arc<ReadCounts>);
impl Drop for ActiveRead {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn concurrency_checks(context: &mut Context) -> TestResult<()> {
    for size in ["4", "10"] {
        let baselines = context
            .baselines
            .iter()
            .filter(|item| item.target.size == size)
            .cloned()
            .collect::<Vec<_>>();
        let ids = baselines
            .iter()
            .map(|item| item.id.clone())
            .collect::<Vec<_>>();
        for concurrency in [1usize, 2, 3] {
            let counts = Arc::new(ReadCounts::default());
            let work = |id: String| {
                let counts = counts.clone();
                let state = context.state.clone();
                let project = context.project.clone();
                async move {
                    let active = counts.active.fetch_add(1, Ordering::SeqCst) + 1;
                    counts.peak.fetch_max(active, Ordering::SeqCst);
                    let _active = ActiveRead(counts);
                    let rows = app_config::read(&state, &project, &[id.clone()]).await?;
                    require(
                        rows.len() == 1 && rows[0].screen_id == id,
                        "逐屏读取返回ID不一致",
                    )?;
                    Ok::<_, Box<dyn std::error::Error>>((id, rows.into_iter().next().unwrap()))
                }
            };
            let mut pending =
                stream::iter(ids.iter().cloned().map(work)).buffer_unordered(concurrency);
            let mut output = BTreeMap::new();
            while let Some(row) = pending.next().await {
                let (id, row) = row?;
                output.insert(id, row);
            }
            drop(pending);
            require(
                output.len() == ids.len()
                    && baselines.iter().all(|item| {
                        output
                            .get(&item.id)
                            .is_some_and(|row| row.config == item.read.config)
                    }),
                "并发读取内容与基线不一致",
            )?;
            let peak = counts.peak.load(Ordering::SeqCst);
            require(
                peak <= concurrency.min(ids.len()),
                "读取峰值超过冻结并发或本组设备数",
            )?;
            context.record(size,"single_id_workers",json!({"passed":true,"requestedConcurrency":concurrency,"actualPeakReadCalls":peak,"screenCount":ids.len(),"resultIdsMatch":true,"configurationUnchanged":true}));
        }
    }
    let ids = context
        .baselines
        .iter()
        .map(|item| item.id.clone())
        .collect::<Vec<_>>();
    let calls = Arc::new(AtomicUsize::new(0));
    let probe = calls.clone();
    let read = app_config::read_with_device_factory(
        &context.state,
        &context.project,
        &ids,
        || -> AppResult<AdbDevice> {
            probe.fetch_add(1, Ordering::SeqCst);
            Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict(
                "不应构建设备端口".into(),
            ))
        },
    )
    .await;
    let patches = context
        .baselines
        .iter()
        .map(|item| {
            (
                item.id.clone(),
                set("customDeviceName", json!("混合门禁不得写入")),
            )
        })
        .collect();
    let probe = calls.clone();
    let preview = maintenance::preflight_with_device_factory(
        &context.state,
        &context.project,
        input(ids, 3),
        Some(patches),
        None,
        || -> AppResult<AdbDevice> {
            probe.fetch_add(1, Ordering::SeqCst);
            Err(inxaiot_desk_buddy_lib::core::error::AppError::Conflict(
                "不应构建设备端口".into(),
            ))
        },
    )
    .await;
    require(
        read.is_err() && preview.is_err() && calls.load(Ordering::SeqCst) == 0,
        "混合尺寸没有在设备端口之前拒绝",
    )?;
    context.record(
        "all",
        "mixed_size_gate",
        json!({"passed":true,"deviceFactoryCalls":0,"executionTasksCreated":0}),
    );
    context.persist()
}

async fn batch_name_checks(context: &mut Context) -> TestResult<()> {
    for size in ["4", "10"] {
        let selected = context
            .baselines
            .iter()
            .filter(|baseline| baseline.target.size == size)
            .cloned()
            .collect::<Vec<_>>();
        let mut group = Vec::new();
        for baseline in selected {
            if let Some(reason) = restoration_issue(&baseline, "customDeviceName") {
                context.uncovered(&baseline.id, "customDeviceName", &reason);
            } else {
                group.push(baseline);
            }
        }
        if group.is_empty() {
            continue;
        }
        let patches = group
            .iter()
            .map(|baseline| {
                (
                    baseline.id.clone(),
                    set(
                        "customDeviceName",
                        json!(format!(
                            "实施回归-{}",
                            &uuid::Uuid::now_v7().to_string()[..8]
                        )),
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let request = input(
            group.iter().map(|baseline| baseline.id.clone()).collect(),
            group.len() as u32,
        );
        let preview = maintenance::preflight_with_config(
            &context.state,
            &context.project,
            request.clone(),
            Some(patches.clone()),
        )
        .await?;
        require(
            preview.items.len() == group.len()
                && preview.items.iter().all(|item| item.state == "ready"),
            "同尺寸改名批次预检未通过",
        )?;
        let mut runtimes = BTreeMap::new();
        for baseline in &group {
            context.remember(&baseline.id, &patches[&baseline.id])?;
            runtimes.insert(
                baseline.id.clone(),
                runtime(&context.device, &baseline.target.ip).await?,
            );
        }
        let task = tasks::submit(&context.state, &context.project, &preview.id, request).await?;
        tokio::time::timeout(Duration::from_secs(300), async {
            loop {
                let state = context.state.task_repository.get(&task).await?.state;
                if state.is_terminal() || state == TaskState::FinalizingFailed {
                    return Ok::<(), inxaiot_desk_buddy_lib::core::error::AppError>(());
                }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        })
        .await??;
        let results =
            task_data::read_results(context.state.local_store.pool(), &context.project, &task)
                .await?;
        for baseline in &group {
            settle(context, &baseline.id).await?;
            let result = results
                .targets
                .get(&baseline.id)
                .ok_or("批次缺少目标结果")?;
            let row = read_one(context, baseline).await?;
            require(
                result.device == ResultState::Succeeded
                    && result.business == ResultState::NotRequired
                    && result.shared == ResultState::NotRequired
                    && result.evidence["config"]["restart"] == "not_required"
                    && patches[&baseline.id].matches(row.config.as_ref().unwrap())
                    && runtime(&context.device, &baseline.target.ip).await?
                        == runtimes[&baseline.id],
                "同尺寸批次改名、逐台结果或不重启核对失败",
            )?;
            context.record(&baseline.id,"same_size_batch_name",json!({"passed":true,"taskId":task,"batchSize":group.len(),"taskResultPerScreen":true,"pidAndBootUnchanged":true}));
        }
        for baseline in &group {
            restore_fields(context, baseline, &["customDeviceName".into()]).await?;
        }
    }
    context.persist()
}

fn address_candidate(config: &Value, environment: &str, field: &str) -> Option<Value> {
    let current = field_value(config, &format!("environments.{environment}.{field}"))?;
    let existing = current
        .as_str()
        .filter(|value| !value.is_empty())
        .or_else(|| {
            ENVIRONMENTS.iter().find_map(|env| {
                config["environments"][*env][field]
                    .as_str()
                    .filter(|value| !value.is_empty())
            })
        })?;
    let mut url = reqwest::Url::parse(existing).ok()?;
    if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") || url.host_str().is_none() {
        return None;
    }
    // 仅非运行环境使用同地址的片段变体；不会改变服务端主机、端口或请求路径。
    url.set_fragment(Some(if url.fragment() == Some("workbench-regression") {
        "workbench-regression-2"
    } else {
        "workbench-regression"
    }));
    let value = json!(url.as_str());
    (!values_equal(current, &value)).then_some(value)
}
async fn endpoint_available(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let Ok(url) = reqwest::Url::parse(text) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return false;
    }
    let Ok(client) = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .redirect(reqwest::redirect::Policy::none())
        .build()
    else {
        return false;
    };
    client
        .head(url)
        .send()
        .await
        .is_ok_and(|response| response.status().as_u16() < 500)
}
async fn choose_inactive_environment(
    context: &mut Context,
    baseline: &Baseline,
    current: &str,
) -> TestResult<Option<&'static str>> {
    let original = baseline.read.config.as_ref().unwrap();
    let mut offline = None;
    for environment in ENVIRONMENTS
        .into_iter()
        .filter(|environment| *environment != current)
    {
        let issues = ADDRESS_FIELDS
            .into_iter()
            .filter_map(|field| {
                let key = format!("environments.{environment}.{field}");
                restoration_issue(baseline, &key).map(|reason| (key, reason))
            })
            .collect::<Vec<_>>();
        if !issues.is_empty() {
            for (field, reason) in issues {
                context.uncovered(
                    &baseline.id,
                    &field,
                    &format!("非当前环境候选无法完整恢复：{reason}"),
                );
            }
            continue;
        }
        let target = &original["environments"][environment];
        if endpoint_available(&target["otaUrl"]).await
            && (target["h5Url"].is_null() || endpoint_available(&target["h5Url"]).await)
        {
            return Ok(Some(environment));
        }
        if offline.is_none() {
            offline = Some(environment);
        }
    }
    if offline.is_none() {
        context.uncovered(
            &baseline.id,
            "inactive_environment",
            "两套非当前环境均无法完整恢复，具体缺项已逐字段记录；未改写地址或切换环境",
        );
    }
    Ok(offline)
}

async fn ordinary_checks(
    context: &mut Context,
    baseline: &Baseline,
    include_name_qa: bool,
) -> TestResult<()> {
    let original = baseline.read.config.as_ref().unwrap();
    let current = original["environments"]["current"]
        .as_str()
        .filter(|value| ENVIRONMENTS.contains(value))
        .ok_or("原运行环境不可靠")?;
    if include_name_qa {
        if let Some(name) = field_value(original, "customDeviceName") {
            let same = if name.is_null() {
                AppConfigPatch {
                    clear: vec!["customDeviceName".into()],
                    ..Default::default()
                }
            } else {
                set("customDeviceName", name.clone())
            };
            require(
                submit(context, baseline, "same_value_no_task", same)
                    .await?
                    .is_none(),
                "同值目标没有在提交前过滤",
            )?;
        } else {
            context.uncovered(
                &baseline.id,
                "customDeviceName",
                "原名称字段缺失，未生成同值或修改请求",
            );
        }
    }
    if let Some(inactive) = choose_inactive_environment(context, baseline, current).await? {
        for field in ADDRESS_FIELDS {
            let key = format!("environments.{inactive}.{field}");
            if !baseline.writable.contains(&key) {
                context.uncovered(&baseline.id, &key, "设备未声明字段可写");
                continue;
            }
            let Some(original_value) = field_value(original, &key) else {
                context.uncovered(&baseline.id, &key, "原字段缺失，未视作null或修改");
                continue;
            };
            let original_patch = if original_value.is_null() {
                AppConfigPatch {
                    clear: vec![key.clone()],
                    ..Default::default()
                }
            } else {
                set(&key, original_value.clone())
            };
            if original_patch.validate().is_err() {
                context.uncovered(
                    &baseline.id,
                    &key,
                    "原地址不能通过正式协议精确恢复，未修改该字段",
                );
                continue;
            }
            if field == "wsUrl" {
                ws_roundtrip(context, baseline, &key).await?;
                continue;
            }
            if let Some(value) = address_candidate(original, inactive, field) {
                let candidate = set(&key, value);
                if candidate.validate().is_err() {
                    context.uncovered(&baseline.id, &key, "同主机地址变体不满足实际写入协议");
                    continue;
                }
                submit(context, baseline, "inactive_address", candidate).await?;
                if field != "otaUrl" {
                    submit(
                        context,
                        baseline,
                        "explicit_clear",
                        AppConfigPatch {
                            clear: vec![key.clone()],
                            ..Default::default()
                        },
                    )
                    .await?;
                }
                restore_fields(context, baseline, &[key]).await?;
            } else {
                context.uncovered(
                    &baseline.id,
                    &key,
                    "没有已有地址可生成不改变服务器的合法变体",
                );
            }
        }
        let target = &original["environments"][inactive];
        let target_available = endpoint_available(&target["otaUrl"]).await
            && (target["h5Url"].is_null() || endpoint_available(&target["h5Url"]).await);
        let mut copied_fields = Vec::new();
        let mut available = target_available;
        if !target_available {
            let active = &original["environments"][current];
            let mut copy = AppConfigPatch::default();
            let mut restorable = true;
            for field in ADDRESS_FIELDS {
                let key = format!("environments.{inactive}.{field}");
                let Some(original_value) = field_value(original, &key) else {
                    context.uncovered(&baseline.id, &key, "原字段缺失，不能通过清空恢复");
                    restorable = false;
                    break;
                };
                let restore = if original_value.is_null() {
                    AppConfigPatch {
                        clear: vec![key.clone()],
                        ..Default::default()
                    }
                } else {
                    set(&key, original_value.clone())
                };
                if !baseline.writable.contains(&key) || restore.validate().is_err() {
                    restorable = false;
                    break;
                }
                let source_key = format!("environments.{current}.{field}");
                let Some(value) = field_value(original, &source_key) else {
                    context.uncovered(
                        &baseline.id,
                        &source_key,
                        "原当前服务字段缺失，未猜测null复制到其它环境",
                    );
                    restorable = false;
                    break;
                };
                if value.is_null() {
                    copy.clear.push(key.clone());
                } else {
                    copy.set.insert(key.clone(), value.clone());
                }
                copied_fields.push(key);
            }
            if restorable
                && copy.validate().is_ok()
                && endpoint_available(&active["otaUrl"]).await
                && (active["h5Url"].is_null() || endpoint_available(&active["h5Url"]).await)
            {
                submit(
                    context,
                    baseline,
                    "prepare_environment_with_existing_current_services",
                    copy,
                )
                .await?;
                available = true;
            } else {
                copied_fields.clear();
                context.uncovered(
                    &baseline.id,
                    "environment_switch_preparation",
                    "原目标环境必填地址不可正式恢复，或原当前服务也未确认可达；未复制新服务器",
                );
            }
        }
        if available {
            submit(
                context,
                baseline,
                "environment_switch",
                set("environments.current", json!(inactive)),
            )
            .await?;
            restore_fields(context, baseline, &["environments.current".into()]).await?;
            if !copied_fields.is_empty() {
                restore_fields(context, baseline, &copied_fields).await?;
            }
        } else {
            context.uncovered(
                &baseline.id,
                "environment_switch",
                "已有非运行环境地址未确认可达，未切换或注入新服务",
            );
        }
    }
    current_h5_checks(context, baseline).await
}
async fn ws_roundtrip(context: &mut Context, baseline: &Baseline, key: &str) -> TestResult<()> {
    if let Some(reason) = restoration_issue(baseline, key) {
        context.uncovered(&baseline.id, key, &reason);
        return Ok(());
    }
    let original = baseline.read.config.as_ref().unwrap();
    let value = field_value(original, key).ok_or("原WS字段缺失")?;
    let before = context.report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["screenId"] == baseline.id && row["result"]["taskId"].is_string())
        .count();
    if value.as_str().is_some_and(|value| !value.is_empty()) {
        submit(
            context,
            baseline,
            "ws_explicit_clear",
            AppConfigPatch {
                clear: vec![key.into()],
                ..Default::default()
            },
        )
        .await?;
        restore_fields(context, baseline, &[key.into()]).await?;
    } else if value.is_null() {
        let current = original["environments"]["current"]
            .as_str()
            .ok_or("当前环境未知")?;
        let sources = std::iter::once(current).chain(
            ENVIRONMENTS
                .into_iter()
                .filter(|environment| *environment != current),
        );
        let mut candidate = None;
        for environment in sources {
            for field in ["effectiveWsUrl", "otaWsUrl"] {
                let Some(text) = original["environments"][environment][field].as_str() else {
                    continue;
                };
                if reqwest::Url::parse(text).is_ok_and(|url| {
                    matches!(url.scheme(), "ws" | "wss")
                        && url.host_str().is_some()
                        && url.fragment().is_none()
                }) {
                    candidate = Some(text.to_string());
                    break;
                }
            }
            if candidate.is_some() {
                break;
            }
        }
        let Some(candidate) = candidate else {
            context.uncovered(
                &baseline.id,
                key,
                "原有效或OTA下发WS也无合法候选，不注入新服务器",
            );
            return Ok(());
        };
        // 使用已有URI原样保存；不添加WS不允许的片段，不输出可能含鉴权参数的地址。
        submit(
            context,
            baseline,
            "ws_existing_effective_input",
            set(key, json!(candidate)),
        )
        .await?;
        submit(
            context,
            baseline,
            "ws_explicit_clear",
            AppConfigPatch {
                clear: vec![key.into()],
                ..Default::default()
            },
        )
        .await?;
        restore_fields(context, baseline, &[key.into()]).await?;
    } else {
        context.uncovered(&baseline.id, key, "原WS不是可恢复的null或非空地址");
        return Ok(());
    }
    let after = context.report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["screenId"] == baseline.id && row["result"]["taskId"].is_string())
        .count();
    if after - before < 2 {
        context.uncovered(
            &baseline.id,
            key,
            "未形成两个实际WS写/清/恢复任务，不计为完整输入清空覆盖",
        );
        return Ok(());
    }
    context.record(&baseline.id,"ws_input_clear_readback",json!({"passed":true,"field":key,"existingServiceUriOnly":true,"actualTasks":after-before,"originalNull":value.is_null()}));
    context.persist()
}
async fn ws_supplement(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    let current = baseline.read.config.as_ref().unwrap()["environments"]["current"]
        .as_str()
        .ok_or("原环境未知")?;
    let key = ENVIRONMENTS
        .into_iter()
        .filter(|environment| *environment != current)
        .map(|environment| format!("environments.{environment}.wsUrl"))
        .find(|key| restoration_issue(baseline, key).is_none());
    if let Some(key) = key {
        ws_roundtrip(context, baseline, &key).await
    } else {
        context.uncovered(
            &baseline.id,
            "ws_supplement",
            "无可精确恢复的非当前环境WS字段",
        );
        Ok(())
    }
}
async fn current_h5_checks(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    let original = baseline.read.config.as_ref().unwrap();
    let current = original["environments"]["current"]
        .as_str()
        .filter(|value| ENVIRONMENTS.contains(value))
        .ok_or("原运行环境不可靠，未修改H5检查")?;
    let h5 = format!("environments.{current}.h5ReadyCheckEnabled");
    if let Some(value) = field_value(original, &h5).and_then(Value::as_bool) {
        submit(
            context,
            baseline,
            "current_h5_check",
            set(&h5, json!(!value)),
        )
        .await?;
        require(
            submit(
                context,
                baseline,
                "current_h5_same_value",
                set(&h5, json!(!value)),
            )
            .await?
            .is_none(),
            "同值H5开关重复提交",
        )?;
        restore_fields(context, baseline, &[h5]).await?;
    } else {
        context.uncovered(
            &baseline.id,
            "current_h5_check",
            "原网页检查开关不可可靠恢复",
        );
    }
    Ok(())
}

async fn voice_checks(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    let original = baseline.read.config.as_ref().unwrap();
    if let Some(value) = original["autoStartVoiceEngine"].as_bool() {
        submit(
            context,
            baseline,
            "voice_startup",
            set("autoStartVoiceEngine", json!(!value)),
        )
        .await?;
        require(
            submit(
                context,
                baseline,
                "voice_same_value",
                set("autoStartVoiceEngine", json!(!value)),
            )
            .await?
            .is_none(),
            "同值语音开关重复提交",
        )?;
        restore_fields(context, baseline, &["autoStartVoiceEngine".into()]).await?;
    } else {
        context.uncovered(
            &baseline.id,
            "voice_startup",
            "设备没有可可靠恢复的语音启动开关",
        );
    }
    let Some(schema) = ConfigSchema::from_capabilities(
        baseline.read.capabilities.as_ref().ok_or("缺少字段能力")?,
    )?
    else {
        context.uncovered(&baseline.id, "voice_schema", "设备未提供音频字段定义");
        return Ok(());
    };
    let mut patch = AppConfigPatch::default();
    for leaf in AUDIO {
        let key = format!("audioConfig.{leaf}");
        let Some(field) = schema.field(&key).filter(|field| field.writable) else {
            context.uncovered(&baseline.id, &key, "设备未声明该音频字段可写");
            continue;
        };
        let Some(current) = field_value(original, &key).filter(|value| field.accepts(value)) else {
            context.uncovered(&baseline.id, &key, "音频原值不能按设备定义精确恢复");
            continue;
        };
        let mut candidates = field
            .options
            .as_ref()
            .map(|options| {
                options
                    .iter()
                    .map(|option| option.value.clone())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(number) = current.as_f64() {
            let step = field.step.unwrap_or(1.0);
            for value in [number + step, number - step] {
                candidates.push(if field.field_type == FieldType::Integer {
                    json!(value as i64)
                } else {
                    json!((value * 1_000_000.0).round() / 1_000_000.0)
                });
            }
        }
        candidates.push(field.default_value.clone());
        if let Some(candidate) = candidates
            .into_iter()
            .find(|value| field.accepts(value) && !values_equal(value, current))
        {
            patch.set.insert(key, candidate);
        } else {
            context.uncovered(&baseline.id, &key, "没有范围内的不同候选值");
        }
    }
    let fields = patch.fields();
    if !patch.is_empty() {
        submit(context, baseline, "voice_schema_values", patch).await?;
        restore_fields(context, baseline, &fields).await?;
    }
    Ok(())
}

async fn qa_standard_task(
    context: &mut Context,
    baseline: &Baseline,
    phase: &str,
    target_policy: bool,
) -> TestResult<()> {
    settle(context, &baseline.id).await?;
    let before = read_one(context, baseline).await?;
    require(
        before
            .owner
            .as_ref()
            .is_some_and(|owner| owner.state == "self")
            && before
                .kiosk
                .as_ref()
                .is_some_and(|kiosk| kiosk.enabled == Some(false) && kiosk.active == Some(false)),
        "只有既有Owner且已正式解锁后才准备本应用单Main任务",
    )?;
    // 用正式策略关闭固定，避免系统复用原HOME；不改系统默认桌面或其它应用任务。
    submit(
        context,
        baseline,
        "qa_prepare_clear_fixed_home",
        AppConfigPatch {
            home_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    identity(&context.device, &baseline.target).await?;
    inxaiot_desk_buddy_lib::infrastructure::smart_screen::kiosk::wake(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    let current = read_one(context, baseline).await?;
    require(
        current.kiosk.as_ref().is_some_and(|kiosk| {
            kiosk.enabled == Some(false)
                && kiosk.active == Some(false)
                && kiosk.activity_ready == Some(true)
        }),
        "QA预置尚未确认本应用有效前台与解锁，未移除任务",
    )?;
    let plan = home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    context
        .attempts
        .entry(baseline.id.clone())
        .or_default()
        .entry("$runtime".into())
        .or_default()
        .push(json!("standard"));
    context.report["restoreRequired"] = json!(true);
    context.record(&baseline.id,"qa_own_task_removal_requested",json!({"taskKind":plan.kind,"taskId":plan.task_id,"qaPreparationOnly":true,"otherApplicationsTouched":false}));
    context.persist()?;
    if plan.kind != "absent" {
        require(
            ["home", "standard"].contains(&plan.kind.as_str()),
            "未确认本应用单Main任务类型，拒绝移除",
        )?;
        home_activation::require_capability(&context.device, &baseline.target.ip).await?;
        let id = plan.task_id.ok_or("本应用任务编号缺失")?;
        identity(&context.device, &baseline.target).await?;
        home_activation::prepare(
            &context.device,
            &baseline.target.ip,
            id,
            &uuid::Uuid::now_v7().to_string(),
        )
        .await?;
        require(
            home_activation::wait_removed(
                &context.device,
                &baseline.target.ip,
                id,
                Duration::from_secs(15),
                CancellationToken::new(),
            )
            .await?,
            "受限Provider已请求但尚未确认原本应用任务退出，不重复移除",
        )?;
        context.record(
            &baseline.id,
            "qa_own_task_removal_verified",
            json!({"oldTaskId":id,"oldTaskRemoved":true,"qaPreparationOnly":true}),
        );
        context.persist()?;
    }
    identity(&context.device, &baseline.target).await?;
    context
        .device
        .shell(
            &baseline.target.ip,
            &["am", "start", "-W", "-n", home::COMPONENT],
            CancellationToken::new(),
        )
        .await?;
    let started = home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    context.record(&baseline.id,"qa_plain_start_task_readback",json!({"kind":started.kind,"taskId":started.task_id,
        "ownTaskCount":if started.kind=="absent"{0}else{1},"uniquenessVerifiedByFormalQuery":true,"qaPreparationOnly":true}));
    context.persist()?;
    require(
        started.kind == "standard",
        "普通显式启动未形成真实standard，停止本次预置",
    )?;
    if target_policy {
        context.remember(
            &baseline.id,
            &AppConfigPatch {
                home_enabled: Some(true),
                ..Default::default()
            },
        )?;
        match home::set_home(&context.device, &baseline.target.ip).await? {
            home::HomeWrite::Applied => {}
            home::HomeWrite::Rejected(_) => return Err("QA仅保存主屏策略被拒绝，停止预置".into()),
        }
        let policy = home::wait_for_target(
            &context.device,
            &baseline.target.ip,
            true,
            Duration::from_secs(20),
        )
        .await?;
        require(policy.matches_target(true), "QA主屏策略未独立回读通过")?;
    }
    let final_plan = home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    let after = read_one(context, baseline).await?;
    let unchanged = baseline.writable.iter().all(|field| {
        field_value(before.config.as_ref().unwrap(), field)
            .zip(field_value(after.config.as_ref().unwrap(), field))
            .is_some_and(|(old, now)| values_equal(old, now))
    });
    require(
        final_plan.kind == "standard"
            && after.home.as_ref().is_some_and(|home| {
                home.enabled == Some(target_policy) && home.active == Some(false)
            })
            && unchanged,
        "仅保存策略后standard/未激活或普通配置保持条件未满足",
    )?;
    context.record(&baseline.id,phase,json!({"passed":true,"qaPreparationOnly":true,"taskKind":"standard","taskId":final_plan.task_id,
        "homePolicySavedWithoutHomeEvent":target_policy,"ordinaryConfigurationUnchanged":true,"applicationDataCleared":false,"ownerChanged":false}));
    context.persist()
}

async fn independent_home_activation(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    let current = read_one(context, baseline).await?;
    if !current
        .owner
        .as_ref()
        .is_some_and(|owner| owner.state == "self")
        || !current
            .home
            .as_ref()
            .is_some_and(|home| home.supported && home.enabled.is_some())
        || !["home", "standard"].contains(&baseline.task_kind.as_str())
    {
        context.uncovered(
            &baseline.id,
            "independent_home_activation",
            "原Owner/主屏策略/本应用任务类型未确认，不能构造并精确恢复",
        );
        return Ok(());
    }
    if baseline.read.kiosk.as_ref().is_some_and(|kiosk| {
        kiosk.enabled.is_some() && kiosk.active.is_some() && kiosk.enabled != kiosk.active
    }) {
        context.uncovered(
            &baseline.id,
            "independent_home_activation",
            "原锁定保存与运行不一致，不能精确恢复；不执行主屏或锁定写阶段",
        );
        return Ok(());
    }
    let kiosk = current.kiosk.as_ref().ok_or("缺少锁定状态")?;
    if kiosk.enabled != Some(false) || kiosk.active != Some(false) {
        if !kiosk_pair_reversible(&baseline.read) {
            context.uncovered(
                &baseline.id,
                "independent_home_activation",
                "锁定未确认关闭且原键缺失，不能为测试建立不可逆新键",
            );
            return Ok(());
        }
        submit(
            context,
            baseline,
            "unlock_before_independent_home",
            AppConfigPatch {
                kiosk_enabled: Some(false),
                ..Default::default()
            },
        )
        .await?;
    }
    submit(
        context,
        baseline,
        "prepare_fixed_home_policy",
        AppConfigPatch {
            home_enabled: Some(true),
            ..Default::default()
        },
    )
    .await?;
    qa_standard_task(context, baseline, "prepare_standard_with_fixed_home", true).await?;
    let before = read_one(context, baseline).await?;
    require(
        before
            .home
            .as_ref()
            .is_some_and(|home| home.enabled == Some(true) && home.active == Some(false)),
        "未形成已固定但未激活的独立勾选前提",
    )?;
    context.record(
        &baseline.id,
        "independent_formal_submit_requested",
        json!({"policyAlreadyTrue":true,"taskKind":"standard","qaPreparationOnly":false}),
    );
    context.persist()?;
    let result = submit(
        context,
        baseline,
        "independent_activate_home_only",
        AppConfigPatch {
            home_enabled: Some(true),
            ..Default::default()
        },
    )
    .await?
    .ok_or("独立激活被误当作无需修改")?;
    require(
        result.evidence["config"]["save"] == "not_required"
            && result.evidence["config"]["restart"] == "not_required"
            && result.evidence["home"]["save"] == "unchanged"
            && result.evidence["home"]["taskBefore"]["kind"] == "standard"
            && result.evidence["home"]["preparation"] == "verified"
            && result.evidence["home"]["taskRemovalReadback"] == "succeeded"
            && result.evidence["home"]["runtimeReadback"] == "succeeded",
        "独立切主屏没有保持策略只读或缺少原普通任务退出/新主屏证据",
    )?;
    context.record(&baseline.id,"independent_home_evidence",json!({"passed":true,"ordinaryConfigurationSaved":false,"ordinaryApplicationRestarted":false,
        "homePolicySave":"unchanged","ordinaryTaskRemoved":true,"newHomeTaskVerified":true}));
    context.persist()
}

fn kiosk_pair_reversible(row: &AppConfigRead) -> bool {
    row.kiosk.as_ref().is_some_and(|kiosk| {
        kiosk.supported
            && kiosk.configured == Some(true)
            && kiosk.enabled.is_some()
            && kiosk.active == kiosk.enabled
    })
}
fn reversible_controls(row: &AppConfigRead) -> bool {
    row.owner
        .as_ref()
        .is_some_and(|owner| owner.state == "self")
        && row
            .home
            .as_ref()
            .is_some_and(|home| home.supported && home.enabled.is_some())
        && kiosk_pair_reversible(row)
}
async fn control_checks(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    if baseline
        .read
        .owner
        .as_ref()
        .is_some_and(|owner| owner.state == "self")
    {
        require(
            submit(
                context,
                baseline,
                "owner_existing_self_no_task",
                AppConfigPatch {
                    ensure_device_owner: Some(true),
                    ..Default::default()
                },
            )
            .await?
            .is_none(),
            "已有Owner不应为测试重复设置",
        )?;
    }
    if !["home", "standard"].contains(&baseline.task_kind.as_str()) {
        context.uncovered(
            &baseline.id,
            "home_kiosk_controls",
            "原本应用无活动或任务类型未确认，不执行依赖激活的主屏/锁定序列",
        );
        return Ok(());
    }
    if !reversible_controls(&baseline.read) {
        context.uncovered(
            &baseline.id,
            "home_kiosk_controls",
            "Owner不是既有self、锁定原键缺失或运行态不一致，不能精确反向恢复；保持原状仅只读核对",
        );
        return Ok(());
    }
    submit(
        context,
        baseline,
        "unlock_before_home",
        AppConfigPatch {
            kiosk_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    submit(
        context,
        baseline,
        "home_off",
        AppConfigPatch {
            home_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    submit(
        context,
        baseline,
        "lock_on",
        AppConfigPatch {
            kiosk_enabled: Some(true),
            ..Default::default()
        },
    )
    .await?;
    let locked = read_one(context, baseline).await?;
    let mut dependency = AppConfigPatch {
        home_enabled: Some(true),
        ..Default::default()
    };
    let mut dependency_checkable = locked
        .home
        .as_ref()
        .is_some_and(|home| home.active == Some(false));
    if locked
        .home
        .as_ref()
        .is_some_and(|home| home.active == Some(true))
    {
        if let Some(value) = locked
            .config
            .as_ref()
            .and_then(|config| config["autoStartVoiceEngine"].as_bool())
        {
            dependency
                .set
                .insert("autoStartVoiceEngine".into(), json!(!value));
            dependency_checkable = dependency.restart_required_with_capabilities(
                locked.config.as_ref().unwrap(),
                locked.capabilities.as_ref().ok_or("缺少真实配置能力")?,
            );
        }
    }
    if dependency_checkable {
        let count = local_task_count(context).await?;
        let request = input(vec![baseline.id.clone()], 1);
        let checked = maintenance::preflight_with_config(
            &context.state,
            &context.project,
            request,
            Some(BTreeMap::from([(baseline.id.clone(), dependency.clone())])),
        )
        .await?;
        require(
            checked.items.len() == 1
                && checked.items[0].state == "blocked"
                && checked.items[0].reason.contains("主屏需要重新激活")
                && checked.items[0].reason.contains("请先关闭锁定")
                && local_task_count(context).await? == count,
            "锁定依赖未在提交前阻止主屏激活",
        )?;
        context.record(
        &baseline.id,
        "locked_home_dependency",
        json!({"passed":true,"executionTasksCreated":0,"preflightOnly":true,"virtualCheckFields":dependency.fields(),"virtualFieldsNeverSubmitted":true}),
    );
    } else {
        context.uncovered(
            &baseline.id,
            "locked_home_dependency",
            "当前HOME已活跃且没有可靠的需重启虚拟字段；不把其它blocked算作锁定依赖",
        );
    }
    submit(
        context,
        baseline,
        "unlock_and_activate_home",
        AppConfigPatch {
            home_enabled: Some(true),
            kiosk_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    require(
        submit(
            context,
            baseline,
            "home_same_value",
            AppConfigPatch {
                home_enabled: Some(true),
                ..Default::default()
            },
        )
        .await?
        .is_none(),
        "已激活主屏重复提交",
    )?;
    submit(
        context,
        baseline,
        "lock_on_after_home",
        AppConfigPatch {
            kiosk_enabled: Some(true),
            ..Default::default()
        },
    )
    .await?;
    submit(
        context,
        baseline,
        "lock_off",
        AppConfigPatch {
            kiosk_enabled: Some(false),
            ..Default::default()
        },
    )
    .await?;
    Ok(())
}

async fn restore_runtime(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    let current = home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    if current.kind == baseline.task_kind {
        return Ok(());
    }
    require(
        ["home", "standard"].contains(&baseline.task_kind.as_str()),
        "原任务类型不可精确重建，保留运行态恢复责任",
    )?;
    if baseline.task_kind == "standard" {
        qa_standard_task(
            context,
            baseline,
            "restore_original_standard_task",
            baseline
                .read
                .home
                .as_ref()
                .and_then(|home| home.enabled)
                .ok_or("原主屏策略缺失")?,
        )
        .await?;
    } else {
        submit(
            context,
            baseline,
            "restore_original_home_task",
            AppConfigPatch {
                home_enabled: Some(true),
                ..Default::default()
            },
        )
        .await?;
        if baseline.read.home.as_ref().and_then(|home| home.enabled) == Some(false) {
            submit(
                context,
                baseline,
                "restore_original_unfixed_home_policy",
                AppConfigPatch {
                    home_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        }
    }
    let after = home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    require(after.kind == baseline.task_kind, "原本应用任务类型尚未恢复")
}

async fn restore_power_runtime(context: &mut Context, baseline: &Baseline) -> TestResult<bool> {
    let target = (baseline.wakefulness.as_str(), baseline.display.as_str());
    require(
        matches!(target, ("Asleep", "OFF") | ("Awake", "ON")),
        "原电源运行态是未知或Dozing等状态，不猜测恢复",
    )?;
    physical_identity(&context.device, &baseline.target).await?;
    let current = sleep_policy::read(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    require(
        matches!(
            (current.wakefulness.as_str(), current.display.as_str()),
            ("Asleep", "OFF") | ("Awake", "ON")
        ) || (target == ("Asleep", "OFF")
            && current.wakefulness == "Dreaming"
            && current.display == "ON"),
        "当前电源运行态未明确，不发送猜测性按键",
    )?;
    let changed = (current.wakefulness.as_str(), current.display.as_str()) != target;
    if changed {
        let key = if target == ("Asleep", "OFF") {
            "KEYCODE_SLEEP"
        } else {
            "KEYCODE_WAKEUP"
        };
        context.record(&baseline.id,"qa_power_restore_requested",json!({"key":key,"baselineWakefulness":baseline.wakefulness,"baselineDisplay":baseline.display,"qaRestorationOnly":true}));
        context.persist()?;
        context
            .device
            .shell(
                &baseline.target.ip,
                &["input", "keyevent", key],
                CancellationToken::new(),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let after = sleep_policy::read(
                &context.device,
                &baseline.target.ip,
                CancellationToken::new(),
            )
            .await?;
            if (after.wakefulness.as_str(), after.display.as_str()) == target {
                break;
            }
            require(
                tokio::time::Instant::now() < deadline,
                "QA收尾电源运行态没有恢复；配置原值不据此冒称完成",
            )?;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    context.record(&baseline.id,"qa_power_restored",json!({"passed":true,"wakefulness":baseline.wakefulness,"display":baseline.display,"sleepAndBrightnessSettingsWritten":false}));
    context.persist()?;
    Ok(changed)
}

fn can_restore_environment_fields(
    original: Option<&str>,
    current: Option<&str>,
    environment: &str,
) -> bool {
    let original = original.filter(|value| ENVIRONMENTS.contains(value));
    let current = current.filter(|value| ENVIRONMENTS.contains(value));
    match (original, current) {
        (Some(original), Some(current)) => original == current || current != environment,
        _ => false,
    }
}
fn power_matches(baseline: &Baseline, power: &sleep_policy::SleepPolicyStatus) -> bool {
    matches!(
        (baseline.wakefulness.as_str(), baseline.display.as_str()),
        ("Asleep", "OFF") | ("Awake", "ON")
    ) && power.wakefulness == baseline.wakefulness
        && power.display == baseline.display
}

fn qa_keep_awake_masks(baseline: &Baseline) -> TestResult<(u32, u32)> {
    let original = baseline
        .system_settings
        .get("global.stay_on_while_plugged_in")
        .ok_or("缺少原接电唤醒设置，不能准备QA保醒")?
        .trim()
        .parse::<u32>()?;
    Ok((original, original | 7))
}

#[test]
fn qa_guard_keep_awake_requires_restorable_original_and_preserves_unknown_bits() {
    let mut baseline = qa_guard_fixture();
    assert!(qa_keep_awake_masks(&baseline).is_err());
    baseline
        .system_settings
        .insert("global.stay_on_while_plugged_in".into(), "16\r\n".into());
    assert_eq!(qa_keep_awake_masks(&baseline).unwrap(), (16, 23));
    baseline
        .system_settings
        .insert("global.stay_on_while_plugged_in".into(), "null".into());
    assert!(qa_keep_awake_masks(&baseline).is_err());
}

async fn qa_keep_awake(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    require(
        baseline.target.ip == "192.168.3.74",
        "QA保醒只允许已批准补测的74屏",
    )?;
    settle(context, &baseline.id).await?;
    identity(&context.device, &baseline.target).await?;
    let (original, target) = qa_keep_awake_masks(baseline)?;
    let before = sleep_policy::read(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    require(
        before.saved == Some(original),
        "QA保醒写前原设置已变化，未覆盖",
    )?;
    context
        .attempts
        .entry(baseline.id.clone())
        .or_default()
        .entry("$qaStayOn".into())
        .or_default()
        .push(json!({"original":original,"target":target}));
    context.report["restoreRequired"] = json!(true);
    context.record(
        &baseline.id,
        "qa_keep_awake_requested",
        json!({"originalMask":original,"targetMask":target,
        "qaPreparationOnly":true,"screenOffTimeoutWritten":false,"screensaverWritten":false}),
    );
    context.persist()?;
    identity(&context.device, &baseline.target).await?;
    sleep_policy::set_mask(
        &context.device,
        &baseline.target.ip,
        &before,
        CancellationToken::new(),
    )
    .await?;
    let saved = sleep_policy::read(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    require(
        saved.saved == Some(target) && saved.already_disabled(),
        "QA接电保醒保存/运行回读未确认",
    )?;
    inxaiot_desk_buddy_lib::infrastructure::smart_screen::kiosk::wake(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    let awake = sleep_policy::read(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await?;
    require(
        awake.saved == Some(target)
            && awake.stay_on
            && awake.wakefulness == "Awake"
            && awake.display == "ON",
        "QA保醒尚未确认Awake/ON，未继续控制测试",
    )?;
    context.record(
        &baseline.id,
        "qa_keep_awake_verified",
        json!({"passed":true,"savedMask":target,"mStayOn":awake.stay_on,
        "wakefulness":awake.wakefulness,"display":awake.display,"qaPreparationOnly":true}),
    );
    context.persist()
}

async fn qa_restore_keep_awake(context: &mut Context, baseline: &Baseline) -> TestResult<()> {
    if !context
        .attempts
        .get(&baseline.id)
        .is_some_and(|fields| fields.contains_key("$qaStayOn"))
    {
        return Ok(());
    }
    let (original, target) = qa_keep_awake_masks(baseline)?;
    identity(&context.device, &baseline.target).await?;
    let current = context
        .device
        .shell(
            &baseline.target.ip,
            &["settings", "get", "global", "stay_on_while_plugged_in"],
            CancellationToken::new(),
        )
        .await?;
    let current = current.trim().parse::<u32>()?;
    require(
        current == original || current == target,
        "QA保醒收尾发现非本轮值，保留恢复责任而不覆盖外部变化",
    )?;
    if current != original {
        context.record(
            &baseline.id,
            "qa_keep_awake_restore_requested",
            json!({"targetMask":original,"qaRestorationOnly":true}),
        );
        context.persist()?;
        identity(&context.device, &baseline.target).await?;
        context
            .device
            .shell(
                &baseline.target.ip,
                &[
                    "settings",
                    "put",
                    "global",
                    "stay_on_while_plugged_in",
                    &original.to_string(),
                ],
                CancellationToken::new(),
            )
            .await?;
    }
    let after = context
        .device
        .shell(
            &baseline.target.ip,
            &["settings", "get", "global", "stay_on_while_plugged_in"],
            CancellationToken::new(),
        )
        .await?;
    require(
        after.trim().parse::<u32>()? == original,
        "QA接电保醒原值回读未通过",
    )?;
    context.record(
        &baseline.id,
        "qa_keep_awake_restored",
        json!({"passed":true,"savedMask":original,"qaRestorationOnly":true}),
    );
    context.persist()
}

async fn restore_one(context: &mut Context, baseline: &Baseline) -> TestResult<Value> {
    settle(context, &baseline.id).await?;
    let mut errors = Vec::new();
    let attempted = context
        .attempts
        .get(&baseline.id)
        .cloned()
        .unwrap_or_default();
    let touched = !attempted.is_empty();
    let owner_self = baseline
        .read
        .owner
        .as_ref()
        .is_some_and(|owner| owner.state == "self");
    let control_before = match read_one(context, baseline).await {
        Ok(row) => Some(row),
        Err(error) => {
            errors.push(safe_error(&error));
            None
        }
    };
    let kiosk_diff = control_before
        .as_ref()
        .and_then(|row| row.kiosk.as_ref())
        .zip(baseline.read.kiosk.as_ref())
        .is_some_and(|(now, old)| {
            now.configured != old.configured
                || now.enabled != old.enabled
                || now.active != old.active
        });
    let kiosk_needs_restore =
        attempted.contains_key("$kiosk") && kiosk_pair_reversible(&baseline.read) && kiosk_diff;
    if kiosk_needs_restore && owner_self {
        if let Err(error) = submit(
            context,
            baseline,
            "restore_changed_kiosk_unlock",
            AppConfigPatch {
                kiosk_enabled: Some(false),
                ..Default::default()
            },
        )
        .await
        {
            errors.push(safe_error(&error));
        }
    }
    if let Err(error) = restore_fields(context, baseline, &["environments.current".into()]).await {
        errors.push(safe_error(&error));
    }
    // 未确认回原环境时，绝不把可能仍运行的临时环境还原为旧的不可用地址。
    let environment_read = match read_one(context, baseline).await {
        Ok(row) => Some(row),
        Err(error) => {
            errors.push(safe_error(&error));
            None
        }
    };
    let original_environment = baseline
        .read
        .config
        .as_ref()
        .and_then(|value| field_value(value, "environments.current"))
        .and_then(Value::as_str);
    let current_environment = environment_read
        .as_ref()
        .and_then(|row| row.config.as_ref())
        .and_then(|value| field_value(value, "environments.current"))
        .and_then(Value::as_str);
    for environment in ENVIRONMENTS {
        let fields = baseline
            .writable
            .iter()
            .filter(|field| field.starts_with(&format!("environments.{environment}.")))
            .cloned()
            .collect::<Vec<_>>();
        if !can_restore_environment_fields(original_environment, current_environment, environment) {
            errors.push(format!(
                "运行环境尚未确认回原值，保留{environment}环境的可用地址并保留恢复责任"
            ));
            context.record(&baseline.id,"restore_environment_deferred",json!({"environment":environment,"state":"needs_restore","keptCurrentUsableAddresses":true}));
            continue;
        }
        if let Err(error) = restore_fields(context, baseline, &fields).await {
            errors.push(safe_error(&error));
        }
    }
    let global = baseline
        .writable
        .iter()
        .filter(|field| !field.starts_with("environments."))
        .cloned()
        .collect::<Vec<_>>();
    if let Err(error) = restore_fields(context, baseline, &global).await {
        errors.push(safe_error(&error));
    }
    if touched && owner_self {
        if attempted.contains_key("$home") {
            if let Some(enabled) = baseline.read.home.as_ref().and_then(|home| home.enabled) {
                if let Err(error) = submit(
                    context,
                    baseline,
                    "restore_changed_home_policy",
                    AppConfigPatch {
                        home_enabled: Some(enabled),
                        ..Default::default()
                    },
                )
                .await
                {
                    errors.push(safe_error(&error));
                }
            }
        }
        if let Err(error) = restore_runtime(context, baseline).await {
            errors.push(safe_error(&error));
        }
        if kiosk_needs_restore {
            let enabled = baseline.read.kiosk.as_ref().unwrap().enabled.unwrap();
            if let Err(error) = submit(
                context,
                baseline,
                "restore_changed_kiosk",
                AppConfigPatch {
                    kiosk_enabled: Some(enabled),
                    ..Default::default()
                },
            )
            .await
            {
                errors.push(safe_error(&error));
            }
        }
    }
    // 所有耗时的配置、任务及设置查询先完成；收尾最后核实真实电源状态。
    let final_read = match read_one(context, baseline).await {
        Ok(row) => Some(row),
        Err(error) => {
            errors.push(safe_error(&error));
            None
        }
    };
    let configuration = final_read
        .as_ref()
        .and_then(|row| row.config.as_ref())
        .is_some_and(|config| {
            baseline.writable.iter().all(|field| {
                match (
                    field_value(baseline.read.config.as_ref().unwrap(), field),
                    field_value(config, field),
                ) {
                    (Some(a), Some(b)) => values_equal(a, b),
                    _ => false,
                }
            }) && baseline
                .missing_fields
                .iter()
                .all(|field| field_value(config, field).is_none())
        });
    let owner = final_read
        .as_ref()
        .and_then(|row| row.owner.as_ref())
        .zip(baseline.read.owner.as_ref())
        .is_some_and(|(after, before)| {
            after.state == before.state && after.component == before.component
        });
    let home_policy = final_read
        .as_ref()
        .and_then(|row| row.home.as_ref())
        .zip(baseline.read.home.as_ref())
        .is_some_and(|(after, before)| after.enabled == before.enabled);
    let kiosk = final_read
        .as_ref()
        .and_then(|row| row.kiosk.as_ref())
        .zip(baseline.read.kiosk.as_ref())
        .is_some_and(|(after, before)| {
            after.configured == before.configured
                && after.enabled == before.enabled
                && after.active == before.active
        });
    let final_kind = match home_activation::query(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await
    {
        Ok(plan) => Some(plan.kind),
        Err(error) => {
            errors.push(safe_error(&error));
            None
        }
    };
    let task_kind = final_kind.as_deref() == Some(baseline.task_kind.as_str());
    let runtime_ids = match runtime(&context.device, &baseline.target.ip).await {
        Ok(value) => Some(value),
        Err(error) => {
            errors.push(safe_error(&error));
            None
        }
    };
    let boot_unchanged = runtime_ids
        .as_ref()
        .is_some_and(|(_, boot)| boot == &baseline.boot);
    let pid_changed = runtime_ids
        .as_ref()
        .map(|(pid, _)| pid != &baseline.app_pids);
    if let Err(error) = qa_restore_keep_awake(context, baseline).await {
        errors.push(safe_error(&error));
    }
    let settings_unchanged = match system_settings(&context.device, &baseline.target.ip).await {
        Ok(values) => values == baseline.system_settings,
        Err(error) => {
            errors.push(safe_error(&error));
            false
        }
    };
    let requests = |context: &Context| {
        context.report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["screenId"] == baseline.id && row["phase"] == "qa_power_restore_requested"
            })
            .count()
    };
    let before_requests = requests(context);
    if let Err(error) = restore_power_runtime(context, baseline).await {
        errors.push(safe_error(&error));
    }
    let mut corrected = requests(context) > before_requests;
    let mut final_home = home::query_runtime(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await
    .ok();
    let mut final_power = sleep_policy::read(
        &context.device,
        &baseline.target.ip,
        CancellationToken::new(),
    )
    .await
    .ok();
    // 自然息屏若恰好发生在最后轻量读取间隙，整个收尾最多明确修正一次，再立即只读确认。
    if !corrected
        && final_power.as_ref().is_some_and(|power| {
            matches!(
                (power.wakefulness.as_str(), power.display.as_str()),
                ("Awake", "ON") | ("Asleep", "OFF")
            ) && !power_matches(baseline, power)
        })
    {
        match restore_power_runtime(context, baseline).await {
            Ok(_) => {
                final_home = home::query_runtime(
                    &context.device,
                    &baseline.target.ip,
                    CancellationToken::new(),
                )
                .await
                .ok();
                final_power = sleep_policy::read(
                    &context.device,
                    &baseline.target.ip,
                    CancellationToken::new(),
                )
                .await
                .ok();
            }
            Err(error) => errors.push(safe_error(&error)),
        }
    }
    corrected = requests(context) > before_requests;
    if final_home.is_none() {
        errors.push("最终主屏运行态读取未确认".into());
    }
    if final_power.is_none() {
        errors.push("最终电源运行态读取未确认".into());
    }
    let power = final_power
        .as_ref()
        .is_some_and(|power| power_matches(baseline, power));
    let home_runtime = final_home
        .as_ref()
        .zip(baseline.read.home.as_ref())
        .is_some_and(|(after, before)| {
            before
                .active
                .is_some_and(|active| after.verified() == active)
        });
    Ok(
        json!({"screenId":baseline.id,"passed":configuration&&owner&&home_policy&&home_runtime&&task_kind&&kiosk&&boot_unchanged&&power&&settings_unchanged&&errors.is_empty(),
        "allWritableFieldsRestored":configuration,"allPresentWritableFieldsRestored":configuration,"originalMissingFieldsRemainMissing":configuration,"ownerUnchanged":owner,
        "homePolicyRestored":home_policy,"homeRuntimeRestored":home_runtime,"originalTaskKind":baseline.task_kind,"finalTaskKind":final_kind,
        "taskKindRestored":task_kind,"applicationPidChanged":pid_changed,"kioskSavedAndRuntimeRestored":kiosk,"systemBootUnchanged":boot_unchanged,
        "powerRuntimeRestored":power,"powerCorrectionRequested":corrected,"finalWakefulness":final_power.as_ref().map(|power|&power.wakefulness),
        "finalDisplay":final_power.as_ref().map(|power|&power.display),"sleepAndBrightnessSettingsUnchanged":settings_unchanged,"errors":errors}),
    )
}

async fn probe_sleeping_task_kind(
    device: &AdbDevice,
    directory: &Path,
    baseline: &Baseline,
) -> TestResult<Value> {
    require(
        baseline.wakefulness == "Asleep" && baseline.display == "OFF",
        "任务类型未确认且不是明确休眠，未猜测任务或写设置",
    )?;
    let path = directory.join(format!("power-probe-{}.json", baseline.target.ip));
    let mut record = json!({"screenId":baseline.id,"originalWakefulness":baseline.wakefulness,"originalDisplay":baseline.display,
        "restoreRequired":true,"qaProbeOnly":true,"configurationWrites":false});
    write_json(&path, &record)?;
    let observed: TestResult<String> = async {
        identity(device, &baseline.target).await?;
        device
            .shell(
                &baseline.target.ip,
                &["input", "keyevent", "KEYCODE_WAKEUP"],
                CancellationToken::new(),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            if let Ok(plan) =
                home_activation::query(device, &baseline.target.ip, CancellationToken::new()).await
            {
                if ["home", "standard", "absent"].contains(&plan.kind.as_str()) {
                    return Ok(plan.kind);
                }
            }
            require(
                tokio::time::Instant::now() < deadline,
                "唤醒后任务类型仍未确认",
            )?;
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    .await;
    // 探测失败也先恢复明确的原睡眠运行态，绝不因错误提前离开。
    let restored: TestResult<()> = async {
        physical_identity(device, &baseline.target).await?;
        device
            .shell(
                &baseline.target.ip,
                &["input", "keyevent", "KEYCODE_SLEEP"],
                CancellationToken::new(),
            )
            .await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            let power =
                sleep_policy::read(device, &baseline.target.ip, CancellationToken::new()).await?;
            if power.wakefulness == "Asleep" && power.display == "OFF" {
                break;
            }
            require(
                tokio::time::Instant::now() < deadline,
                "任务探测后原睡眠运行态尚未恢复",
            )?;
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        require(
            system_settings(device, &baseline.target.ip).await? == baseline.system_settings,
            "任务探测改变了睡眠或亮度配置",
        )
    }
    .await;
    record["taskKind"] = json!(observed.as_ref().ok());
    record["probeError"] = json!(observed.as_ref().err().map(|error| safe_error(error)));
    record["restored"] = json!(restored.is_ok());
    record["restoreRequired"] = json!(restored.is_err());
    record["restoreError"] = json!(restored.as_ref().err().map(|error| safe_error(error)));
    write_json(&path, &record)?;
    Ok(record)
}

async fn resolve_task_kinds(context: &mut Context, only: &str) -> TestResult<()> {
    for index in 0..context.baselines.len() {
        let baseline = context.baselines[index].clone();
        if only != "all" && !baseline.target.ip.ends_with(&format!(".{only}")) {
            continue;
        }
        if baseline.task_kind != "unconfirmed" {
            continue;
        }
        if baseline.wakefulness != "Asleep" || baseline.display != "OFF" {
            context.uncovered(
                &baseline.id,
                "task_kind",
                "清醒任务类型仍未确认，保持只读及确定不创建活动的字段；不执行重启或激活阶段",
            );
            continue;
        }
        // 三屏原值、Context、attempts和result均已持久化，探测失败由统一finally继续收尾。
        context.report["restoreRequired"] = json!(true);
        context.persist()?;
        let result =
            probe_sleeping_task_kind(&context.device, &context.directory, &baseline).await?;
        if let Some(kind) = result["taskKind"].as_str() {
            context.baselines[index].task_kind = kind.into();
        }
        write_json(
            &context.directory.join("baseline-private.json"),
            &context.baselines,
        )?;
        context.record(&baseline.id, "sleeping_task_kind_probe", result.clone());
        context.persist()?;
        require(
            result["restored"] == true,
            "唤醒探测的原睡眠态尚未恢复，进入统一恢复并保留恢复责任",
        )?;
        require(
            result["taskKind"].is_string(),
            "唤醒后仍无法确认原任务类型，未开始功能写入",
        )?;
    }
    Ok(())
}

async fn prepare(
    state: Arc<FormalAppState>,
    directory: PathBuf,
    targets: Vec<Target>,
) -> TestResult<Context> {
    let device = AdbDevice::new(real_sdk_tools()?);
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut baselines = Vec::new();
    for target in targets {
        identity(&device, &target).await?;
        let id = repo
            .save_local(
                &project,
                &ScreenFields {
                    name: format!("{}寸阶段B隔离回归", target.size),
                    ip: target.ip.clone(),
                    size: target.size.clone(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await?;
        let asset = repo.asset(&project, &id).await?;
        let observed = device
            .inspect(&asset, "inspect", CancellationToken::new())
            .await?;
        let wired = observed
            .observed_mac
            .as_deref()
            .filter(|value| valid_mac(value))
            .ok_or("未取得实际有线身份")?;
        require(
            normalize_mac(wired) == target.wired_mac,
            "实际有线MAC与阶段A不一致",
        )?;
        let mut fields = asset.fields.clone();
        fields.mac = normalize_mac(wired);
        repo.save_local(&project, &fields, Some(&id), Some(asset.revision))
            .await?;
        let rows = app_config::read(&state, &project, &[id.clone()]).await?;
        require(
            rows.len() == 1 && rows[0].config.is_some(),
            "配置基线读取失败，尚未修改",
        )?;
        let read = rows.into_iter().next().unwrap();
        let (writable, missing_fields): (Vec<_>, Vec<_>) = selected_fields(&read)?
            .into_iter()
            .partition(|field| field_value(read.config.as_ref().unwrap(), field).is_some());
        let power = sleep_policy::read(&device, &target.ip, CancellationToken::new()).await?;
        let system_settings = system_settings(&device, &target.ip).await?;
        let (app_pids, boot) = runtime(&device, &target.ip).await?;
        let task_kind = home_activation::query(&device, &target.ip, CancellationToken::new())
            .await
            .map(|plan| plan.kind)
            .unwrap_or_else(|_| "unconfirmed".into());
        baselines.push(Baseline {
            target,
            id,
            read,
            writable,
            missing_fields,
            boot,
            app_pids,
            task_kind,
            wakefulness: power.wakefulness,
            display: power.display,
            system_settings,
        });
        write_json(&directory.join("baseline-private.json"), &baselines)?;
    }
    let identities=baselines.iter().map(|baseline|json!({"screenId":baseline.id,"ip":baseline.target.ip,"size":baseline.target.size,
        "serialAndUserWifiMacMatched":true,"wiredMacChangedSincePhaseAStart":baseline.target.wired_mac!=baseline.target.phase_a_initial_wired_mac,
        "baselineTaskKind":baseline.task_kind,"baselineWakefulness":baseline.wakefulness,"baselineDisplay":baseline.display})).collect::<Vec<_>>();
    let mut context = Context {
        state,
        device,
        directory,
        project: project.clone(),
        baselines,
        attempts: Default::default(),
        report: json!({"passed":false,"restored":false,"restoreRequired":false,"stage":"B","projectId":project,"identities":identities,"realDesktopClickVerified":false,"sameValueEvidenceSource":"qa_difference_filter","sameValueProductImplementationVerifiedByThisRunner":false,"adbArgumentAuditEnabled":false,"workbenchPlatformBusinessOrSharedDatabaseWrites":false,"workbenchProductionDatabaseAccess":false,"existingAddressHeadProbes":true,"zeroPlatformNetworkClaim":false,"ownerRemovedOrCreated":false,"applicationDataCleared":false,"checks":[],"uncovered":[],"restoration":[]}),
    };
    for baseline in context.baselines.clone() {
        for field in baseline.missing_fields {
            context.uncovered(
                &baseline.id,
                &field,
                "设备声明可写但实际原字段缺失；保持缺失，不写null或清空",
            );
        }
    }
    context.persist()?;
    Ok(context)
}

async fn load_restore_context(
    state: Arc<FormalAppState>,
    directory: PathBuf,
    authorized: &[Target],
) -> TestResult<Context> {
    let baselines: Vec<Baseline> =
        serde_json::from_slice(&std::fs::read(directory.join("baseline-private.json"))?)?;
    let attempts =
        serde_json::from_slice(&std::fs::read(directory.join("attempts-private.json"))?)?;
    let mut report: Value = serde_json::from_slice(&std::fs::read(directory.join("result.json"))?)?;
    let project = report["projectId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or("恢复项目编号缺失")?
        .to_string();
    require(
        baselines.len() == 3 && report["stage"] == "B",
        "恢复文件不是本轮三屏阶段B",
    )?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    for baseline in &baselines {
        require(
            authorized.iter().any(|target| {
                target.ip == baseline.target.ip
                    && target.size == baseline.target.size
                    && target.serial == baseline.target.serial
                    && target.supplied_mac == baseline.target.supplied_mac
                    && target.wired_mac == baseline.target.wired_mac
            }),
            "恢复身份与阶段A授权不同",
        )?;
        let asset = repo.asset(&project, &baseline.id).await?;
        require(
            asset.source == "local"
                && asset.fields.ip == baseline.target.ip
                && asset.fields.size == baseline.target.size
                && normalize_mac(&asset.fields.mac) == baseline.target.wired_mac,
            "恢复资产不是原本机项目目标",
        )?;
    }
    report["restoreOnly"] = json!(true);
    report["passed"] = json!(false);
    Ok(Context {
        state,
        device: AdbDevice::new(real_sdk_tools()?),
        directory,
        project,
        baselines,
        attempts,
        report,
    })
}

fn remaining_source(root: &Path, plan: &str) -> TestResult<Option<Value>> {
    if plan == "full" {
        return Ok(None);
    }
    let source = match plan {
        "remaining_after_53b" => REMAINING_SOURCE_CASE,
        "controls_after_551" => CONTROLS_SOURCE_CASE,
        "controls_awake_after_599" => AWAKE_CONTROLS_SOURCE_CASE,
        _ => return Err("不支持的QA续测计划".into()),
    };
    let path = root
        .join(".review-tools/app-config-regression-20261104")
        .join(source)
        .join("result.json");
    let bytes = std::fs::read(&path)?;
    let report: Value = serde_json::from_slice(&bytes)?;
    require(
        report["stage"] == "B" && report["restored"] == true,
        "先前B尚未完整恢复，未开始续测",
    )?;
    let rows = report["restoration"]
        .as_array()
        .ok_or("先前B缺少逐屏恢复证明")?;
    require(
        rows.len() == 3 && rows.iter().all(|row| row["passed"] == true),
        "先前B逐屏恢复尚未完成",
    )?;
    Ok(Some(
        json!({"case":source,"resultSha256":hex::encode(Sha256::digest(&bytes)),"functionalPassed":report["functionalVerificationPassed"],"restored":true,
        "preservedFailure":report["error"],"preservedUncovered":report["uncovered"]}),
    ))
}
fn selected_stages(plan: &str, ip: &str) -> Vec<&'static str> {
    if plan == "controls_awake_after_599" {
        return vec![
            "QA临时保醒并恢复",
            "主屏同等前置",
            "原失败锁定开启和关闭",
            "独立切到主屏一次",
        ];
    }
    if plan == "controls_after_551" {
        return vec![
            "既有Owner当前self只读确认",
            "独立切到主屏",
            "主屏固定及锁定开关和依赖",
        ];
    }
    if plan == "remaining_after_53b" && ip == "192.168.3.74" {
        vec![
            "WS补测",
            "当前H5检查",
            "语音启动与音频字段",
            "独立切到主屏",
            "主屏锁定控制",
        ]
    } else {
        vec![
            "非当前地址与WS",
            "环境切换",
            "当前H5检查",
            "语音启动与音频字段",
            "独立切到主屏",
            "主屏锁定控制",
        ]
    }
}
async fn exercise_device(context: &mut Context, baseline: &Baseline, plan: &str) -> TestResult<()> {
    if ["controls_after_551", "controls_awake_after_599"].contains(&plan) {
        if plan == "controls_awake_after_599" {
            qa_keep_awake(context, baseline).await?;
            submit(
                context,
                baseline,
                "home_same_precondition_after_sleep_failure",
                AppConfigPatch {
                    home_enabled: Some(true),
                    ..Default::default()
                },
            )
            .await?;
            submit(
                context,
                baseline,
                "lock_on_after_home",
                AppConfigPatch {
                    kiosk_enabled: Some(true),
                    ..Default::default()
                },
            )
            .await?;
            submit(
                context,
                baseline,
                "lock_off",
                AppConfigPatch {
                    kiosk_enabled: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        } else {
            control_checks(context, baseline).await?;
        }
        let start = context.report["checks"].as_array().unwrap().len();
        let result = independent_home_activation(context, baseline).await;
        if let Err(error) = &result {
            let formal_requested = context.report["checks"].as_array().unwrap()[start..]
                .iter()
                .any(|row| row["phase"] == "independent_formal_submit_requested");
            if !formal_requested {
                context.uncovered(
                    &baseline.id,
                    "independent_home_activation",
                    &format!("QA前置未确认，基本控制结果保留：{}", safe_error(error)),
                );
                let restored = restore_one(context, baseline).await?;
                let completed = restored["passed"] == true;
                context.record(
                    &baseline.id,
                    "independent_preparation_restoration",
                    restored,
                );
                context.persist()?;
                require(completed, "独立前置未覆盖且原状态恢复尚未完成")?;
                return Ok(());
            }
        }
        return result;
    }
    if plan == "remaining_after_53b" && baseline.target.ip == "192.168.3.74" {
        ws_supplement(context, baseline).await?;
        current_h5_checks(context, baseline).await?;
    } else {
        ordinary_checks(context, baseline, plan == "full").await?;
    }
    voice_checks(context, baseline).await?;
    independent_home_activation(context, baseline).await?;
    control_checks(context, baseline).await
}

#[tokio::test]
#[ignore = "阶段A交接后显式REAL=YES、HANDOFF=PHASE_A_FINISHED及阶段A身份基线才允许三屏配置写回归；始终尝试正式任务恢复，不连接平台"]
async fn app_config_full_regression_and_restore() -> TestResult<()> {
    let targets = authorized_targets()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()?;
    let base = root.join(".review-tools/app-config-regression-20261104");
    let plan = std::env::var("INX_APP_CONFIG_REGRESSION_PLAN").unwrap_or_else(|_| "full".into());
    require(
        [
            "full",
            "remaining_after_53b",
            "controls_after_551",
            "controls_awake_after_599",
        ]
        .contains(&plan.as_str()),
        "不支持的QA计划",
    )?;
    let only = std::env::var("INX_APP_CONFIG_REGRESSION_ONLY").unwrap_or_else(|_| "all".into());
    require(
        ["all", "74", "82", "42"].contains(&only.as_str()) && (plan != "full" || only == "all"),
        "目标选择仅支持续测计划中的固定白名单",
    )?;
    require(
        plan != "controls_awake_after_599" || only == "74",
        "保醒精准补测仅允许74，未连接设备",
    )?;
    std::fs::create_dir_all(&base)?;
    require(base.canonicalize()?.starts_with(&root), "证据目录离开仓库")?;
    let restore_case = std::env::var("INX_APP_CONFIG_REGRESSION_RESTORE_CASE").ok();
    let previous = if restore_case.is_none() {
        remaining_source(&root, &plan)?
    } else {
        None
    };
    let directory = if let Some(case) = &restore_case {
        let suffix = case.strip_prefix("run-").ok_or("恢复目录名格式不正确")?;
        require(
            uuid::Uuid::parse_str(suffix).is_ok_and(|value| value.to_string() == suffix),
            "恢复目录仅接受本轮run加UUID",
        )?;
        let path = base.join(case).canonicalize()?;
        require(
            path.starts_with(base.canonicalize()?),
            "恢复目录越过本轮证据范围",
        )?;
        path
    } else {
        let path = base.join(format!("run-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&path)?;
        path
    };
    let state = support::state_at(&directory, true).await;
    let prepared = if restore_case.is_some() {
        load_restore_context(state.clone(), directory.clone(), &targets).await
    } else {
        prepare(state.clone(), directory.clone(), targets).await
    };
    let mut context = match prepared {
        Ok(context) => context,
        Err(error) => {
            support::close(state).await;
            return Err(safe_error(&error).into());
        }
    };
    if restore_case.is_none() {
        context.report["testPlan"] = json!({"name":plan,"only":only,"continuesFrom":previous,"notSelected":if plan=="controls_after_551"{vec!["已过读并发/混尺寸/三屏名字","已过三屏地址/环境/H5检查","已过三屏语音启动和8音频字段"]}else if plan=="remaining_after_53b"{vec!["已过并发读取","已过混尺寸门禁","已过三屏改名","74已过OTA/H5地址及环境切换"]}else{Vec::<&str>::new()}});
        context.report["deviceResults"] = json!([]);
        context.persist()?;
    }
    let outcome: TestResult<()> = async {
        resolve_task_kinds(&mut context,if restore_case.is_some(){"all"}else{&only}).await?;
        if restore_case.is_some() {
            return Ok(());
        }
        if plan=="full"{concurrency_checks(&mut context).await?;batch_name_checks(&mut context).await?;}
        let mut device_errors=Vec::new();
        for baseline in context.baselines.clone() {
            if only!="all"&&!baseline.target.ip.ends_with(&format!(".{only}")){
                context.report["deviceResults"].as_array_mut().unwrap().push(json!({"ip":baseline.target.ip,"state":"not_selected","selectedStages":[]}));continue;
            }
            let gap_start=context.report["uncovered"].as_array().unwrap().len();
            let outcome=exercise_device(&mut context,&baseline,&plan).await;
            let mut recovery=None;
            if let Err(error)=&outcome {
                let detail=json!({"ip":baseline.target.ip,"error":safe_error(error),"laterStagesStopped":true});device_errors.push(detail);
                recovery=Some(match restore_one(&mut context,&baseline).await{Ok(row)=>row,Err(error)=>json!({"screenId":baseline.id,"passed":false,"error":safe_error(&error)})});
            }
            let gaps=context.report["uncovered"].as_array().unwrap().len()-gap_start;
            context.report["deviceResults"].as_array_mut().unwrap().push(json!({"ip":baseline.target.ip,"selectedStages":selected_stages(&plan,&baseline.target.ip),
                "state":if outcome.is_err(){"failed_later_stages_stopped"}else if gaps>0{"completed_with_uncovered"}else{"completed"},"uncoveredCount":gaps,"immediateRestorationAfterFailure":recovery}));
            context.report["deviceErrors"]=json!(device_errors);context.persist()?;
        }
        require(device_errors.is_empty(),"部分设备阶段失败，已停止各自后续并尝试恢复，其它设备独立结果保留")?;
        let independent = context.report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["phase"] == "independent_home_evidence" && row["result"]["passed"] == true
            })
            .count();
        context.report["independentHomeActivationScreens"] = json!(independent);
        require(
            independent > 0,
            "没有任何屏完成独立切到小新主屏分支；不能把全部跳过称为勾选完整通过",
        )?;
        Ok(())
    }
    .await;
    if let Err(error) = &outcome {
        context.report["error"] = json!(safe_error(error));
    }
    if restore_case.is_none() {
        context.report["functionalVerificationPassed"] = json!(outcome.is_ok());
    }
    let mut restoration = Vec::new();
    let mut evidence_errors = Vec::new();
    for baseline in context.baselines.clone() {
        let row = match restore_one(&mut context, &baseline).await {
            Ok(row) => row,
            Err(error) => json!({"screenId":baseline.id,"passed":false,"error":safe_error(&error)}),
        };
        restoration.push(row);
        context.report["restoration"] = json!(restoration);
        if let Err(error) = context.persist() {
            evidence_errors.push(safe_error(&error));
        }
    }
    let restored = restoration.len() == 3 && restoration.iter().all(|row| row["passed"] == true);
    let evidence_ok = evidence_errors.is_empty();
    context.report["restored"] = json!(restored);
    context.report["restoreRequired"] = json!(!restored);
    context.report["passed"] =
        json!(context.report["functionalVerificationPassed"] == true && restored && evidence_ok);
    if restore_case.is_some() {
        context.report["restoreOnlyRecoverySucceeded"] = json!(restored && evidence_ok);
    }
    context.report["evidenceErrors"] = json!(evidence_errors);
    let saved = context.persist();
    support::close(state).await;
    saved?;
    if outcome.is_err() || !restored || !evidence_ok {
        return Err(format!(
            "阶段B未通过或恢复未完成；仅核对本机证据目录 {}",
            directory.display()
        )
        .into());
    }
    println!(
        "{} {}",
        if restore_case.is_some() {
            "APP_CONFIG_RESTORE_COMPLETE"
        } else {
            "APP_CONFIG_REGRESSION_PASS"
        },
        directory.display()
    );
    Ok(())
}

// 仅内存验证QA恢复前置；不初始化应用、不读取SDK、不连接屏或数据库。
fn qa_guard_fixture() -> Baseline {
    Baseline {
        target: Target {
            ip: "192.0.2.1".into(),
            size: "4".into(),
            serial: "fixture".into(),
            supplied_mac: "021122334455".into(),
            wired_mac: "021122334466".into(),
            phase_a_initial_wired_mac: "021122334466".into(),
        },
        id: "fixture".into(),
        read: AppConfigRead {
            screen_id: "fixture".into(),
            read_at: "fixture".into(),
            config: Some(
                json!({"customDeviceName":null,"environments":{"current":"prod","test":{"otaUrl":"http://example.invalid/ota","h5Url":null}}}),
            ),
            capabilities: Some(json!({})),
            home: None,
            owner: None,
            kiosk: None,
            message: String::new(),
        },
        writable: vec![
            "customDeviceName".into(),
            "environments.test.otaUrl".into(),
            "environments.test.wsUrl".into(),
            "environments.test.h5Url".into(),
        ],
        missing_fields: vec!["environments.test.wsUrl".into()],
        boot: "00000000-0000-4000-8000-000000000001".into(),
        app_pids: "1".into(),
        task_kind: "standard".into(),
        wakefulness: "Awake".into(),
        display: "ON".into(),
        system_settings: BTreeMap::new(),
    }
}

#[tokio::test]
#[ignore = "仅在旧570进程已止、原Asleep/OFF和原值已核后，明确一次SLEEP恢复74运行态；独立completion不改旧失败report"]
async fn app_config_restore_known_asleep_74_completion() -> TestResult<()> {
    let targets = authorized_targets()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()?;
    let original = root
        .join(
            ".review-tools/app-config-regression-20261104/run-01a1a570-f8ca-75f0-97bf-b25079741ec4",
        )
        .canonicalize()?;
    let old_bytes = std::fs::read(original.join("result.json"))?;
    let old_sha = hex::encode(Sha256::digest(&old_bytes));
    let state = support::state_at(&original, false).await;
    let mut context = load_restore_context(state.clone(), original.clone(), &targets).await?;
    let baseline = context
        .baselines
        .iter()
        .find(|baseline| baseline.target.ip == "192.168.3.74")
        .cloned()
        .ok_or("缺少74原恢复资料")?;
    require(
        baseline.wakefulness == "Asleep"
            && baseline.display == "OFF"
            && baseline
                .read
                .home
                .as_ref()
                .is_some_and(|home| home.active == Some(false)),
        "原74睡眠/主屏运行态不确定，拒绝猜测",
    )?;
    settle(&context, &baseline.id).await?;
    let current = read_one(&context, &baseline).await?;
    let config = current.config.as_ref().unwrap();
    require(
        baseline.writable.iter().all(|field| {
            field_value(baseline.read.config.as_ref().unwrap(), field)
                .zip(field_value(config, field))
                .is_some_and(|(old, now)| values_equal(old, now))
        }),
        "74原配置未先完整恢复，不能只用睡眠掩盖字段恢复",
    )?;
    let completion = original.join(format!("runtime-completion-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir(&completion)?;
    context.directory = completion.clone();
    context.report = json!({"phase":"qa_restore_known_asleep_74","oldResultSha256":old_sha,"priorFunctionalPassed":false,"productIndependentHomeActivationVerified":false,"checks":[],"uncovered":[]});
    context.persist()?;
    let outcome:TestResult<Value>=async{
        let before=sleep_policy::read(&context.device,&baseline.target.ip,CancellationToken::new()).await?;
        let requested=restore_power_runtime(&mut context,&baseline).await?;
        let after_read=read_one(&context,&baseline).await?;
        let home_state=home::query_runtime(&context.device,&baseline.target.ip,CancellationToken::new()).await?;
        let kind=home_activation::query(&context.device,&baseline.target.ip,CancellationToken::new()).await?;
        let settings=system_settings(&context.device,&baseline.target.ip).await?;
        let (pid,boot)=runtime(&context.device,&baseline.target.ip).await?;
        let power=sleep_policy::read(&context.device,&baseline.target.ip,CancellationToken::new()).await?;
        let configuration=baseline.writable.iter().all(|field|field_value(baseline.read.config.as_ref().unwrap(),field).zip(field_value(after_read.config.as_ref().unwrap(),field)).is_some_and(|(old,now)|values_equal(old,now)));
        require(power_matches(&baseline,&power)&&!home_state.verified()&&kind.kind==baseline.task_kind&&configuration&&settings==baseline.system_settings&&boot==baseline.boot,"一次明确睡眠后，原电源/主屏非激活/配置/系统启动编号未全部核实")?;
        Ok(json!({"passed":true,"oldResultSha256":old_sha,"originalWakefulness":baseline.wakefulness,"originalDisplay":baseline.display,"beforeWakefulness":before.wakefulness,"beforeDisplay":before.display,
            "sleepKeyRequested":requested,"afterWakefulness":power.wakefulness,"afterDisplay":power.display,"homeRuntimeActive":home_state.verified(),"taskKind":kind.kind,"allWritableFieldsUnchanged":configuration,
            "sleepAndBrightnessSettingsUnchanged":settings==baseline.system_settings,"systemBootUnchanged":boot==baseline.boot,"applicationPidChanged":pid!=baseline.app_pids,"productIndependentHomeActivationVerified":false}))
    }.await;
    let record = match &outcome {
        Ok(record) => record.clone(),
        Err(error) => {
            json!({"passed":false,"oldResultSha256":old_sha,"error":safe_error(error),"restoreResponsibilityRetained":true})
        }
    };
    write_json(&completion.join("completion.json"), &record)?;
    support::close(state).await;
    require(
        hex::encode(Sha256::digest(std::fs::read(original.join("result.json"))?)) == old_sha,
        "旧失败report被意外覆盖",
    )?;
    outcome?;
    println!("KNOWN_ASLEEP_74_RESTORED {}", completion.display());
    Ok(())
}

#[test]
fn qa_guard_missing_and_invalid_originals_never_become_clearable_null() {
    let mut baseline = qa_guard_fixture();
    assert!(
        restoration_issue(&baseline, "environments.test.wsUrl")
            .unwrap()
            .contains("缺失")
    );
    assert!(restoration_issue(&baseline, "environments.test.h5Url").is_none());
    assert!(restoration_issue(&baseline, "customDeviceName").is_none());
    baseline.read.config.as_mut().unwrap()["customDeviceName"] = json!("");
    assert!(restoration_issue(&baseline, "customDeviceName").is_some());
    baseline.read.config.as_mut().unwrap()["environments"]["test"]["otaUrl"] = json!("");
    assert!(restoration_issue(&baseline, "environments.test.otaUrl").is_some());
}

#[test]
fn qa_guard_active_temporary_environment_keeps_usable_addresses_until_original_is_confirmed() {
    assert!(!can_restore_environment_fields(
        Some("prod"),
        Some("test"),
        "test"
    ));
    assert!(can_restore_environment_fields(
        Some("prod"),
        Some("test"),
        "pre"
    ));
    assert!(can_restore_environment_fields(
        Some("prod"),
        Some("prod"),
        "test"
    ));
    assert!(!can_restore_environment_fields(Some("prod"), None, "test"));
    assert!(!can_restore_environment_fields(None, Some("test"), "prod"));
}

#[test]
fn qa_guard_kiosk_requires_an_existing_consistent_saved_and_running_pair() {
    use inxaiot_desk_buddy_lib::domain::smart_screen::app_config::AppKioskRead;
    let mut baseline = qa_guard_fixture();
    for (configured, enabled, active, expected) in [
        (true, false, false, true),
        (true, true, true, true),
        (true, true, false, false),
        (true, false, true, false),
        (false, false, false, false),
    ] {
        baseline.read.kiosk = Some(AppKioskRead {
            supported: true,
            configured: Some(configured),
            enabled: Some(enabled),
            active: Some(active),
            ..Default::default()
        });
        assert_eq!(kiosk_pair_reversible(&baseline.read), expected);
    }
}

#[tokio::test]
async fn qa_guard_sqlite_count_and_complete_target_history_include_guards() {
    use inxaiot_desk_buddy_lib::infrastructure::local_sqlite::task_repository::CreateTask;
    let directory = tempfile::tempdir().unwrap();
    let state = support::state_at(directory.path(), false).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let context = Context {
        state: state.clone(),
        device: AdbDevice::new(AndroidTools {
            adb: "never-run-sdk-for-sqlite-guard".into(),
        }),
        directory: directory.path().into(),
        project: project.clone(),
        baselines: Vec::new(),
        attempts: Default::default(),
        report: json!({}),
    };
    for index in 0..502 {
        let id = if index == 0 {
            "qa-old-active".to_string()
        } else {
            format!("qa-new-{index:04}")
        };
        state
            .task_repository
            .create(CreateTask {
                id: id.clone(),
                local_project_id: project.clone(),
                remote_operation_record_id: None,
                domain_type: "smart_screen".into(),
                operation_type: "app_config".into(),
                name: "SQLite恢复保护fixture".into(),
                priority: 0,
                batch_size: 1,
                concurrency: 1,
                payload_ref: None,
                log_path: directory
                    .path()
                    .join("fixture.log")
                    .to_string_lossy()
                    .into_owned(),
                targets: vec![(
                    "smart_screen".into(),
                    if index == 0 {
                        "qa-target"
                    } else {
                        "another-target"
                    }
                    .into(),
                )],
            })
            .await
            .unwrap();
        if index == 0 {
            for (from, to) in [
                (TaskState::Draft, TaskState::Checking),
                (TaskState::Checking, TaskState::Failed),
            ] {
                state
                    .task_repository
                    .transition(&id, from, to, None, None)
                    .await
                    .unwrap();
            }
            state
                .task_repository
                .protect_results(&id, "SQLite旧终态待核实fixture")
                .await
                .unwrap();
        }
    }
    assert_eq!(local_task_count(&context).await.unwrap(), 502);
    let page = state
        .task_repository
        .list_recent(&project, 500)
        .await
        .unwrap();
    assert_eq!(page.len(), 500);
    let own = target_task_ids(&context, "qa-target").await.unwrap();
    assert_eq!(own, vec!["qa-old-active"]);
    assert_eq!(
        target_task_ids(&context, "another-target")
            .await
            .unwrap()
            .len(),
        501
    );
    assert!(
        state
            .task_repository
            .results_protected(&own[0])
            .await
            .unwrap()
    );
    state
        .task_repository
        .create(CreateTask {
            id: "qa-one-more".into(),
            local_project_id: project.clone(),
            remote_operation_record_id: None,
            domain_type: "smart_screen".into(),
            operation_type: "app_config".into(),
            name: "SQLite计数增加fixture".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: directory
                .path()
                .join("fixture.log")
                .to_string_lossy()
                .into_owned(),
            targets: vec![("smart_screen".into(), "another-target".into())],
        })
        .await
        .unwrap();
    assert_eq!(
        state
            .task_repository
            .list_recent(&project, 500)
            .await
            .unwrap()
            .len(),
        500
    );
    assert_eq!(local_task_count(&context).await.unwrap(), 503);
    assert_eq!(
        target_task_ids(&context, "another-target")
            .await
            .unwrap()
            .len(),
        502
    );
    support::close(state).await;
}
