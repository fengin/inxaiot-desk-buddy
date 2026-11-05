#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::AppError,
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::{ResultState, ScreenFields},
            operation::ScreenOperationInput,
            rules::{normalize_mac, valid_mac},
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools, XIAOXIN_PACKAGE},
            maintenance, sleep_policy, task_data,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;

// 从当前测试说明取地址；这些额外白名单用于防止误操作其他屏，不回退历史目标。
const TARGET_GUARDS: &[(&str, &str, &str, &str)] = &[
    ("4", "4寸屏", "192.168.3.74", "A47A4AK02040"),
    ("10", "10寸屏", "192.168.3.42", "9c000c3c3643071205d"),
];
const SETTINGS: &[(&str, &str)] = &[
    ("global", "stay_on_while_plugged_in"),
    ("system", "screen_off_timeout"),
    ("secure", "sleep_timeout"),
    ("secure", "screensaver_enabled"),
    ("secure", "screensaver_activate_on_sleep"),
    ("secure", "screensaver_activate_on_dock"),
    ("system", "screen_brightness"),
    ("system", "screen_brightness_mode"),
];
const GLOBAL_KEY: &str = "global.stay_on_while_plugged_in";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Backup {
    name: String,
    size: String,
    ip: String,
    serial: String,
    boot_id: String,
    app_pids: Vec<u32>,
    settings: BTreeMap<String, String>,
}

fn require(condition: bool, message: &str) -> TestResult<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn save(directory: &Path, report: &Value) -> TestResult<()> {
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(report)?,
    )?;
    Ok(())
}

async fn read(device: &AdbDevice, ip: &str, args: &[&str]) -> TestResult<String> {
    let mut command = vec!["shell"];
    command.extend_from_slice(args);
    let output = device
        .adb(
            ip,
            &command,
            Duration::from_secs(15),
            CancellationToken::new(),
        )
        .await?;
    require(
        output.success && output.stderr.trim().is_empty(),
        "设备只读命令未可靠完成，停止本轮验证",
    )?;
    Ok(output.stdout.trim().to_string())
}

async fn identity(device: &AdbDevice, ip: &str, expected: &str) -> TestResult<String> {
    require(
        read(device, ip, &["getprop", "ro.serialno"]).await? == expected,
        "实际序列号与本轮授权目标不符，未继续操作",
    )?;
    let boot_id = read(device, ip, &["cat", "/proc/sys/kernel/random/boot_id"]).await?;
    uuid::Uuid::parse_str(&boot_id)?;
    Ok(boot_id)
}

async fn settings(device: &AdbDevice, ip: &str) -> TestResult<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();
    for (namespace, key) in SETTINGS {
        values.insert(
            format!("{namespace}.{key}"),
            read(device, ip, &["settings", "get", namespace, key]).await?,
        );
    }
    Ok(values)
}

async fn app_pids(device: &AdbDevice, ip: &str) -> TestResult<Vec<u32>> {
    let output = device
        .adb(
            ip,
            &["shell", "pidof", XIAOXIN_PACKAGE],
            Duration::from_secs(10),
            CancellationToken::new(),
        )
        .await?;
    if !output.success && output.stdout.trim().is_empty() && output.stderr.trim().is_empty() {
        return Ok(Vec::new());
    }
    require(
        output.success && output.stderr.trim().is_empty() && !output.stdout.trim().is_empty(),
        "小新进程状态未确认",
    )?;
    let mut values = Vec::new();
    for value in output.stdout.split_whitespace() {
        require(
            value.bytes().all(|character| character.is_ascii_digit()),
            "小新进程编号格式未确认",
        )?;
        let pid = value.parse::<u32>()?;
        require(pid > 0, "小新进程编号无效")?;
        values.push(pid);
    }
    values.sort_unstable();
    values.dedup();
    Ok(values)
}

fn input(ids: Vec<String>) -> ScreenOperationInput {
    ScreenOperationInput {
        action: "sleep_off".into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: String::new(),
        reinstall: false,
        concurrency: 2,
        expected_targets: BTreeMap::new(),
        retry_of_operation_id: None,
    }
}

async fn wait_task(state: &FormalAppState, task: &str) -> TestResult<TaskState> {
    Ok(tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let record = state.task_repository.get(task).await?;
            if record.state.is_terminal() || record.state == TaskState::FinalizingFailed {
                return Ok::<_, AppError>(record.state);
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await??)
}

async fn trial(
    state: &FormalAppState,
    device: &AdbDevice,
    directory: &Path,
    backups: &mut Vec<Backup>,
    mutation_requested: &mut bool,
    report: &mut Value,
) -> TestResult<()> {
    let descriptions = support::live_screens()?;
    for (size, name, approved_ip, serial) in TARGET_GUARDS {
        let candidates = descriptions
            .iter()
            .filter(|screen| screen.size == *size && screen.name == *name)
            .collect::<Vec<_>>();
        require(
            candidates.len() == 1,
            "当前测试说明未明确列出唯一授权名称和尺寸，未连接设备",
        )?;
        let live = candidates[0];
        require(
            live.ip == *approved_ip,
            "当前测试说明地址与本轮授权目标不符，不回退旧地址或扫描网络",
        )?;
        device.connect(&live.ip, CancellationToken::new()).await?;
        let boot_id = identity(device, &live.ip, serial).await?;
        let values = settings(device, &live.ip).await?;
        require(
            values.get(GLOBAL_KEY).map(String::as_str) == Some("0"),
            "原接电保持唤醒值不是0，未开始实机写入",
        )?;
        backups.push(Backup {
            name: live.name.clone(),
            size: live.size.clone(),
            ip: live.ip.clone(),
            serial: (*serial).into(),
            boot_id,
            app_pids: app_pids(device, &live.ip).await?,
            settings: values,
        });
        report["actualTargets"] = serde_json::to_value(&backups)?;
        save(directory, report)?;
    }
    require(backups.len() == 2, "本轮目标必须为明确的4寸和10寸各一台")?;
    report["backupCompleteBeforeSubmit"] = json!(true);
    report["phase"] = json!("baselines_saved");
    save(directory, report)?;

    let project = Stage75Adapter::new(state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repository = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    let mut identities = Vec::new();
    for before in backups.iter() {
        let id = repository
            .save_local(
                &project,
                &ScreenFields {
                    name: before.name.clone(),
                    ip: before.ip.clone(),
                    size: before.size.clone(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await?;
        let asset = repository.asset(&project, &id).await?;
        let observed = device
            .inspect(&asset, "inspect", CancellationToken::new())
            .await?;
        let wired = observed
            .observed_mac
            .as_deref()
            .filter(|mac| valid_mac(mac))
            .ok_or("没有可靠且唯一的有线MAC，未执行写任务")?;
        require(
            observed.adb_available
                && identity(device, &before.ip, &before.serial).await? == before.boot_id,
            "设备身份或启动编号在资产准备期间变化，未开始写入",
        )?;
        let mut fields = asset.fields.clone();
        fields.mac = normalize_mac(wired);
        repository
            .save_local(&project, &fields, Some(&id), Some(asset.revision))
            .await?;
        identities.push(json!({"screenId":id,"ip":before.ip,"size":before.size,"serial":before.serial,
            "wiredMac":fields.mac,"macSource":observed.mac_source,"descriptionWifiMacUsedAsWiredIdentity":false}));
        ids.push(id);
    }
    report["projectId"] = json!(project);
    report["assetIdentities"] = json!(identities);
    let request = input(ids.clone());
    let preview = maintenance::preflight(state, &project, request.clone()).await?;
    report["initialPreflight"] = serde_json::to_value(&preview)?;
    save(directory, report)?;
    require(
        preview.items.len() == 2 && preview.items.iter().all(|item| item.state == "ready"),
        "关闭屏睡眠混合批预检未全部通过",
    )?;
    for before in backups.iter() {
        require(
            identity(device, &before.ip, &before.serial).await? == before.boot_id,
            "预检期间设备启动编号发生变化，停止验证",
        )?;
        require(
            settings(device, &before.ip).await? == before.settings,
            "预检后设置与备份不同，不覆盖外部变更",
        )?;
    }
    report["preflightReadOnlyVerified"] = json!(true);
    report["productMutationRequested"] = json!(true);
    report["phase"] = json!("product_task_submitting");
    save(directory, report)?;
    *mutation_requested = true;
    let task = maintenance::submit(state, &project, &preview.id, request).await?;
    report["taskId"] = json!(task);
    save(directory, report)?;
    let final_state = wait_task(state, &task).await?;
    let results = task_data::read_results(state.local_store.pool(), &project, &task).await?;
    report["taskState"] = json!(final_state.as_str());
    report["productResults"] = serde_json::to_value(&results)?;
    report["phase"] = json!("product_task_results_recorded");
    save(directory, report)?;
    require(
        final_state == TaskState::Succeeded && results.finished && results.targets.len() == 2,
        "关闭屏睡眠正式队列任务未完整成功；保留原任务结果，不重放写入",
    )?;
    for id in &ids {
        let result = results.targets.get(id).ok_or("任务缺少目标结果")?;
        require(
            result.device == ResultState::Succeeded
                && result.evidence["sleepOff"]["saved"] == true
                && result.evidence["sleepOff"]["writeRequested"] == true
                && result.evidence["sleepOff"]["targetMask"] == 7,
            "目标关闭屏睡眠的保存或回读证据不完整",
        )?;
    }

    let mut observations = Vec::new();
    for before in backups.iter() {
        let boot_id = identity(device, &before.ip, &before.serial).await?;
        let status = sleep_policy::read(device, &before.ip, CancellationToken::new()).await?;
        status.check_conditions()?;
        require(
            boot_id == before.boot_id,
            "维护期间出现系统重启，未把本轮结果记为无重启通过",
        )?;
        require(
            status.saved == Some(7) && status.effective_mask == 7 && status.stay_on,
            "设备实际保存值或生效策略未达到本次目标",
        )?;
        let mut expected = before.settings.clone();
        expected.insert(GLOBAL_KEY.into(), "7".into());
        require(
            settings(device, &before.ip).await? == expected,
            "原超时、屏保、亮度或其他备份设置发生变化",
        )?;
        let pids = app_pids(device, &before.ip).await?;
        observations.push(
            json!({"ip":before.ip,"status":status,"appPidsBefore":before.app_pids,
            "appPidsAfter":pids,"bootId":boot_id,"uiReadinessRequired":false}),
        );
        report["afterExecution"] = json!(observations);
        save(directory, report)?;
        require(
            pids == before.app_pids,
            "小新进程状态发生变化，本轮不能确认未重启；不主动重启或恢复APP",
        )?;
    }
    let repeated = maintenance::preflight(state, &project, input(ids)).await?;
    report["repeatPreflight"] = serde_json::to_value(&repeated)?;
    save(directory, report)?;
    require(
        repeated.items.len() == 2 && repeated.items.iter().all(|item| item.state == "skip"),
        "已关闭的屏没有全部跳过",
    )?;
    let executable_plans: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM local_screen_task_data WHERE local_project_id=? AND local_task_id=?",
    )
    .bind(&project)
    .bind(&repeated.id)
    .fetch_one(state.local_store.pool())
    .await?;
    require(executable_plans == 0, "全部已关闭时仍生成了可执行预览")?;
    report["allAlreadyDisabledSkipped"] = json!(true);
    report["appRestartOrSystemRebootRequested"] = json!(false);
    report["privateApplicationDataReadOrCleared"] = json!(false);
    Ok(())
}

async fn restore_one(device: &AdbDevice, before: &Backup, evidence: &mut Value) -> TestResult<()> {
    evidence["ip"] = json!(before.ip);
    evidence["expectedSerial"] = json!(before.serial);
    evidence["restored"] = json!(false);
    let boot_id = identity(device, &before.ip, &before.serial).await?;
    let current = read(
        device,
        &before.ip,
        &["settings", "get", "global", "stay_on_while_plugged_in"],
    )
    .await?;
    evidence["currentBeforeRestore"] = json!(current);
    require(
        current == "0" || current == "7",
        "接电设置出现外部新值，未覆盖，需继续人工核对恢复",
    )?;
    if current != "0" {
        evidence["writeRequested"] = json!(true);
        evidence["requestedOriginalValue"] = json!("0");
        // 只恢复本轮唯一授权修改的全局键；不恢复未曾修改的超时或APP状态。
        match device
            .adb(
                &before.ip,
                &[
                    "shell",
                    "settings",
                    "put",
                    "global",
                    "stay_on_while_plugged_in",
                    "0",
                ],
                Duration::from_secs(15),
                CancellationToken::new(),
            )
            .await
        {
            Ok(output) => {
                evidence["writeResponseConfirmed"] = json!(
                    output.success
                        && output.stdout.trim().is_empty()
                        && output.stderr.trim().is_empty()
                )
            }
            Err(error) => evidence["writeResponseError"] = json!(error.to_string()),
        }
    } else {
        evidence["writeRequested"] = json!(false);
    }
    let after = settings(device, &before.ip).await?;
    evidence["settingsAfter"] = serde_json::to_value(&after)?;
    evidence["bootId"] = json!(boot_id);
    require(
        after.get(GLOBAL_KEY).map(String::as_str) == Some("0"),
        "原接电设置尚未恢复为0",
    )?;
    require(
        after == before.settings,
        "原接电设置已恢复，但其他备份项存在外部差异；不回写其他设置",
    )?;
    evidence["restored"] = json!(true);
    evidence["allOriginalSettingsRetained"] = json!(true);
    evidence["restorationScope"] =
        json!("仅恢复本轮global设置；未发唤醒、睡眠、APP重启或系统重启命令");
    Ok(())
}

#[tokio::test]
#[ignore = "需INX_SCREEN_SLEEP_REAL=1；当前说明明确4/10寸屏及serial，隔离本机任务执行关闭睡眠后恢复原global0，不改超时、不重启"]
async fn formal_sleep_off_queue_two_sizes_skip_after_success_and_restore_originals()
-> TestResult<()> {
    require(
        std::env::var("INX_SCREEN_SLEEP_REAL").as_deref() == Ok("1"),
        "必须显式设置INX_SCREEN_SLEEP_REAL=1才允许实机QA",
    )?;
    let directory = support::evidence_dir("sleep-off-formal-queue")?;
    eprintln!("关闭屏睡眠正式后端实机QA证据：{}", directory.display());
    let state = support::state_at(&directory, true).await;
    let mut report = json!({"passed":false,"restored":false,"restoreRequested":true,
        "qaRestoreIsProductBehavior":false,"productionInstanceOpened":false,"platformConnectionsRequested":false,
        "applicationDataStore":"本轮新建隔离SQLite及MemorySecretStore","actualTargets":[],"restoration":[]});
    let mut backups = Vec::new();
    let mut mutation_requested = false;
    let device = match AndroidTools::discover() {
        Ok(tools) => AdbDevice::new(tools),
        Err(error) => {
            support::close(state).await;
            return Err(error.into());
        }
    };
    let outcome = trial(
        &state,
        &device,
        &directory,
        &mut backups,
        &mut mutation_requested,
        &mut report,
    )
    .await;
    report["productVerificationPassed"] = json!(outcome.is_ok());
    if let Err(error) = &outcome {
        report["error"] = json!(error.to_string());
    }

    // 先停止本隔离实例的任务，避免QA恢复后仍有排队或在途任务迟到写入7。
    let stopped = state.task_queue.shutdown(Duration::from_secs(30)).await;
    report["isolatedQueueStoppedBeforeRestore"] = json!(true);
    report["queueStopOutcomes"] = json!(
        stopped
            .iter()
            .map(|(id, result)| json!({"taskId":id,"outcome":format!("{result:?}")}))
            .collect::<Vec<_>>()
    );
    let mut restore_errors = Vec::new();
    report["phase"] = json!("qa_restore_original_global");
    if let Err(error) = save(&directory, &report) {
        restore_errors.push(format!("恢复前证据保存失败：{error}"));
    }
    if mutation_requested {
        for before in &backups {
            let mut evidence = json!({"scope":"实机QA收尾，仅恢复原global值"});
            if let Err(error) = restore_one(&device, before, &mut evidence).await {
                evidence["error"] = json!(error.to_string());
                restore_errors.push(format!("{}：{error}", before.ip));
            }
            let previous = report["restoration"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let mut rows = previous;
            rows.push(evidence);
            report["restoration"] = json!(rows);
            if let Err(error) = save(&directory, &report) {
                restore_errors.push(format!("恢复证据保存失败：{error}"));
            }
        }
    } else {
        report["restoreNotNeeded"] = json!(true);
    }
    report["restored"] = json!(restore_errors.is_empty());
    report["restoreErrors"] = json!(restore_errors);
    report["passed"] = json!(outcome.is_ok() && restore_errors.is_empty());
    report["phase"] = json!("qa_finished");
    report["finishedAt"] = json!(
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?
    );
    let saved = save(&directory, &report);
    support::close(state).await;
    saved?;
    if !restore_errors.is_empty() {
        return Err(format!(
            "实机QA未完整恢复，须按证据继续处理：{}",
            directory.display()
        )
        .into());
    }
    outcome
}
