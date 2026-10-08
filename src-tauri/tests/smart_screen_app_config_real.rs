#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::{
        app_config::AppConfigPatch,
        model::{ResultState, ScreenFields},
        operation::{ScreenOperationInput, ScreenResults},
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
use std::{collections::BTreeMap, time::Duration};

async fn execute(
    state: &FormalAppState,
    project: &str,
    patches: BTreeMap<String, AppConfigPatch>,
) -> Result<ScreenResults, Box<dyn std::error::Error>> {
    let input = ScreenOperationInput {
        action: "app_config".into(),
        target_ids: patches.keys().cloned().collect(),
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 2,
        expected_targets: Default::default(),
        retry_of_operation_id: None,
    };
    let preview =
        maintenance::preflight_with_config(state, project, input.clone(), Some(patches)).await?;
    for row in &preview.items {
        if row.state != "ready" {
            return Err(format!("配置检查未通过：{}", row.reason).into());
        }
    }
    let task = tasks::submit(state, project, &preview.id, input).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let state = state.task_repository.get(&task).await.unwrap().state;
            if state.is_terminal()
                || state
                    == inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await?;
    let results = task_data::read_results(state.local_store.pool(), project, &task).await?;
    for row in results.targets.values() {
        eprintln!("配置任务结果：{}", row.message);
        if !matches!(row.device, ResultState::Succeeded | ResultState::Skipped) {
            return Err(format!("配置任务未完成：{}", row.message).into());
        }
        if row.business != ResultState::NotRequired || row.shared != ResultState::NotRequired {
            return Err("本机屏配置不应产生平台业务或共享写入".into());
        }
    }
    Ok(results)
}
fn patch(set: Value, clear: Vec<String>) -> AppConfigPatch {
    serde_json::from_value(json!({"set":set,"clear":clear})).unwrap()
}
fn restore(original: &Value, fields: &[String]) -> AppConfigPatch {
    let mut result = AppConfigPatch::default();
    for field in fields {
        let value =
            inxaiot_desk_buddy_lib::domain::smart_screen::app_config::field_value(original, field)
                .unwrap();
        if value.is_null() {
            result.clear.push(field.clone());
        } else {
            result.set.insert(field.clone(), value.clone());
        }
    }
    result
}

// 产品约束是一批只指定一套环境；屏实际环境不同时按环境分组，不调整产品规则。
async fn execute_by_environment(
    state: &FormalAppState,
    project: &str,
    patches: BTreeMap<String, AppConfigPatch>,
) -> Result<ScreenResults, Box<dyn std::error::Error>> {
    let mut groups: BTreeMap<String, BTreeMap<String, AppConfigPatch>> = BTreeMap::new();
    for (id, patch) in patches {
        let environment = patch
            .fields()
            .iter()
            .find_map(|field| {
                let parts = field.split('.').collect::<Vec<_>>();
                (parts.len() == 3).then(|| parts[1].to_string())
            })
            .unwrap_or_default();
        groups.entry(environment).or_default().insert(id, patch);
    }
    let mut results = ScreenResults {
        targets: BTreeMap::new(),
        finished: true,
    };
    for group in groups.into_values() {
        results
            .targets
            .extend(execute(state, project, group).await?.targets);
    }
    Ok(results)
}

fn valid_http_value(value: &Value) -> bool {
    value.as_str().is_some_and(|url| {
        url.len() <= 4096
            && !url.chars().any(char::is_whitespace)
            && reqwest::Url::parse(url).is_ok_and(|parsed| {
                ["http", "https"].contains(&parsed.scheme()) && parsed.host_str().is_some()
            })
    })
}

fn writable_projection(config: &Value) -> Value {
    let mut output = json!({"customDeviceName":config["customDeviceName"],"environments":{"current":config["environments"]["current"]}});
    for env in ["test", "pre", "prod"] {
        output["environments"][env] = json!({});
        for field in ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"] {
            output["environments"][env][field] = config["environments"][env][field].clone();
        }
    }
    output
}

fn readonly_differences(before: &Value, after: &Value) -> Vec<String> {
    let mut fields = Vec::new();
    for env in ["test", "pre", "prod"] {
        if let Some(values) = before["environments"][env].as_object() {
            for (field, value) in values {
                if !["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"].contains(&field.as_str())
                    && after["environments"][env][field] != *value
                {
                    fields.push(format!("environments.{env}.{field}"));
                }
            }
        }
    }
    fields
}

fn safe_case_error(error: &dyn std::fmt::Display) -> String {
    let text = error.to_string();
    let hidden = regex::Regex::new(r#"(?i)(?:https?|wss?)://[^\s\"'<>]+"#)
        .unwrap()
        .replace_all(&text, "[地址已隐藏]");
    regex::Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}(?::\d+)?\b")
        .unwrap()
        .replace_all(&hidden, "[IP已隐藏]")
        .chars()
        .take(1000)
        .collect()
}

async fn application_pid(ip: &str) -> Result<String, Box<dyn std::error::Error>> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    let pid = device
        .shell(
            ip,
            &["pidof", "chat.xiaoxin.app"],
            tokio_util::sync::CancellationToken::new(),
        )
        .await?;
    let pid = pid.trim();
    if pid.is_empty()
        || !pid
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_whitespace())
    {
        return Err("未读取到小新应用进程，不能验证同值设置不重启".into());
    }
    Ok(pid.into())
}

fn require_restart_readback(
    result: &ScreenResults,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if result.targets[id].evidence["config"]["restart"] != "succeeded"
        || result.targets[id].evidence["config"]["readback"] != "succeeded"
    {
        return Err("当前环境修改后未完成重启回读".into());
    }
    Ok(())
}

#[tokio::test]
#[ignore = "在测试说明首台4寸屏临时设置不可用网页地址并重启，随后通过ADB配置任务还原"]
async fn unavailable_web_address_can_be_corrected_through_adb()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = support::evidence_dir("url-recovery")?;
    std::fs::create_dir_all(&dir)?;
    let state = support::state_at(&dir, true).await;
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
                name: "网页地址恢复验收".into(),
                ip: support::live_screen("4")?.ip,
                size: "4".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let before = app_config::read(&state, &project, &[id.clone()]).await?[0]
        .config
        .clone()
        .ok_or("未读取原配置")?;
    std::fs::write(
        dir.join("original-config.json"),
        serde_json::to_vec(&before)?,
    )?;
    let current = before["environments"]["current"]
        .as_str()
        .ok_or("未读取当前环境")?;
    let field = format!("environments.{current}.h5Url");
    if before["environments"][current]["h5Url"] == "" {
        return Err("原手动地址为空字符串，本用例不改写其存储形式".into());
    }
    let outcome = execute(
        &state,
        &project,
        BTreeMap::from([(
            id.clone(),
            patch(
                json!({field.clone():"http://127.0.0.1:9/config-recovery-check"}),
                vec![],
            ),
        )]),
    )
    .await;
    let restored = execute(
        &state,
        &project,
        BTreeMap::from([(id.clone(), restore(&before, &[field]))]),
    )
    .await;
    if let Err(error) = restored {
        return Err(format!("网页配置还原未通过，保留本机验证记录：{error}").into());
    }
    let after = app_config::read(&state, &project, &[id]).await?[0]
        .config
        .clone()
        .ok_or("还原后未读到配置")?;
    std::fs::write(
        dir.join("readonly-field-changes.json"),
        serde_json::to_vec_pretty(&readonly_differences(&before, &after))?,
    )?;
    if writable_projection(&after) != writable_projection(&before) {
        return Err("还原后配置与原值不同".into());
    }
    support::close(state).await;
    outcome?;
    eprintln!("网页地址不可用时ADB配置通道仍可用，已通过工作台任务恢复原地址并重启确认");
    Ok(())
}

#[tokio::test]
#[ignore = "按测试说明验证三屏配置同值、名称、开关、明确清空和环境切换并还原，包含真实小新重启"]
async fn configuration_batch_save_restart_readback_and_restore()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = support::evidence_dir("configuration-batch")?;
    std::fs::create_dir_all(&dir)?;
    let state = support::state_at(&dir, true).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    let mut addresses = BTreeMap::new();
    for screen in support::live_screens()? {
        ids.push(
            repo.save_local(
                &project,
                &ScreenFields {
                    name: screen.name,
                    ip: screen.ip.clone(),
                    size: screen.size,
                    ..Default::default()
                },
                None,
                None,
            )
            .await?,
        );
        addresses.insert(ids.last().unwrap().clone(), screen.ip);
    }
    let before = app_config::read(&state, &project, &ids).await?;
    let mut originals = BTreeMap::new();
    for row in before {
        originals.insert(row.screen_id, row.config.ok_or(row.message)?);
    }
    std::fs::write(
        dir.join("original-config.json"),
        serde_json::to_vec(&originals)?,
    )?;
    let reread = app_config::read(&state, &project, &ids).await?;
    for row in reread {
        if row.config.as_ref() != originals.get(&row.screen_id) {
            return Err("重复读取改变了配置".into());
        }
    }
    let mut inactive = BTreeMap::new();
    let mut restoration = BTreeMap::new();
    for (id, config) in &originals {
        let current = config["environments"]["current"]
            .as_str()
            .ok_or("当前运行环境缺失")?;
        let candidate = ["test", "pre", "prod"]
            .into_iter()
            .find(|env| {
                *env != current
                    && valid_http_value(&config["environments"][*env]["otaUrl"])
                    && (config["environments"][*env]["h5Url"].is_null()
                        || valid_http_value(&config["environments"][*env]["h5Url"]))
            })
            .ok_or("没有可切换且能精确还原手动地址的已有环境，尚未修改配置")?;
        inactive.insert(id.clone(), candidate.to_string());
        restoration.insert(
            id.clone(),
            vec![
                "environments.current".to_string(),
                "customDeviceName".to_string(),
                format!("environments.{candidate}.h5Url"),
                format!("environments.{candidate}.h5ReadyCheckEnabled"),
                format!("environments.{current}.h5ReadyCheckEnabled"),
            ],
        );
    }
    let mut checks = Vec::new();
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        let mut original_pids = BTreeMap::new();
        let mut same_patches = BTreeMap::new();
        for id in &ids {
            original_pids.insert(id.clone(), application_pid(&addresses[id]).await?);
            let current = originals[id]["environments"]["current"].as_str().unwrap();
            same_patches.insert(
                id.clone(),
                restore(
                    &originals[id],
                    &[format!("environments.{current}.h5ReadyCheckEnabled")],
                ),
            );
        }
        let same = execute_by_environment(&state, &project, same_patches).await?;
        for id in &ids {
            let result = &same.targets[id];
            if result.device != ResultState::Skipped
                || result.evidence["config"]["save"] != "unchanged"
                || result.evidence["config"]["restart"] != "not_required"
                || application_pid(&addresses[id]).await? != original_pids[id]
            {
                return Err("同值设置重复保存或重启，或应用进程发生变化".into());
            }
        }
        checks.push("三台同值提交不保存、不重启，应用进程保持");
        let names = ids
            .iter()
            .enumerate()
            .map(|(i, id)| {
                (
                    id.clone(),
                    patch(json!({"customDeviceName":format!("配置验证-{i}")}), vec![]),
                )
            })
            .collect();
        let result = execute(&state, &project, names).await?;
        if !result
            .targets
            .values()
            .all(|r| r.evidence["config"]["restart"] == "not_required")
        {
            return Err("名称修改不应重启".into());
        }
        checks.push("三台批量名称修改无需重启");
        let inactive_patches = ids
            .iter()
            .map(|id| {
                let environment = &inactive[id];
                let field = format!("environments.{environment}.h5ReadyCheckEnabled");
                let value = !originals[id]["environments"][environment]["h5ReadyCheckEnabled"]
                    .as_bool()
                    .unwrap();
                (id.clone(), patch(json!({field:value}), vec![]))
            })
            .collect();
        let result = execute_by_environment(&state, &project, inactive_patches).await?;
        if !result
            .targets
            .values()
            .all(|r| r.evidence["config"]["restart"] == "not_required")
        {
            return Err("非当前环境修改不应重启".into());
        }
        checks.push("三台分别修改未启用环境，无需重启");
        for id in &ids {
            let current = originals[id]["environments"]["current"].as_str().unwrap();
            let field = format!("environments.{current}.h5ReadyCheckEnabled");
            let value = !originals[id]["environments"][current]["h5ReadyCheckEnabled"]
                .as_bool()
                .unwrap();
            let result = execute(
                &state,
                &project,
                BTreeMap::from([(id.clone(), patch(json!({field:value}), vec![]))]),
            )
            .await?;
            require_restart_readback(&result, id)?;
            // 切换环境前先恢复两套开关，确保切换使用屏原有环境内容。
            for environment in [current, inactive[id].as_str()] {
                let field = format!("environments.{environment}.h5ReadyCheckEnabled");
                execute(
                    &state,
                    &project,
                    BTreeMap::from([(id.clone(), restore(&originals[id], &[field]))]),
                )
                .await?;
            }
        }
        checks.push("三台当前环境修改均重启并回读，随后还原开关");
        for id in &ids {
            let environment = &inactive[id];
            let field = format!("environments.{environment}.h5Url");
            // 使用屏已有的实际 H5 地址，不注入新的坏地址，也不启动该环境。
            let existing = originals[id]["environments"][environment]["effectiveH5Url"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or("没有原有H5地址可用于明确清空测试")?;
            execute(
                &state,
                &project,
                BTreeMap::from([(id.clone(), patch(json!({field.clone():existing}), vec![]))]),
            )
            .await?;
            let cleared = execute(
                &state,
                &project,
                BTreeMap::from([(id.clone(), patch(json!({}), vec![field.clone()]))]),
            )
            .await?;
            let result = &cleared.targets[id];
            let after = &result.evidence["config"]["after"];
            if result.evidence["config"]["save"] != "saved"
                || result.evidence["config"]["restart"] != "not_required"
                || !after["environments"][environment]["h5Url"].is_null()
                || after["environments"][environment]["otaH5Url"]
                    != originals[id]["environments"][environment]["otaH5Url"]
            {
                return Err("明确清空手动H5地址没有正确保留服务端地址或意外重启".into());
            }
            execute(
                &state,
                &project,
                BTreeMap::from([(id.clone(), restore(&originals[id], &[field]))]),
            )
            .await?;
        }
        checks.push("三台明确清空非当前环境手动H5地址，保留下发地址且不重启，随后还原");
        for id in &ids {
            let changed = execute(
                &state,
                &project,
                BTreeMap::from([(
                    id.clone(),
                    patch(json!({"environments.current":inactive[id]}), vec![]),
                )]),
            )
            .await?;
            require_restart_readback(&changed, id)?;
            if changed.targets[id].evidence["config"]["after"]["environments"]["current"]
                != inactive[id]
            {
                return Err("切换环境后未读到目标环境".into());
            }
            let restored = execute(
                &state,
                &project,
                BTreeMap::from([(
                    id.clone(),
                    restore(&originals[id], &["environments.current".into()]),
                )]),
            )
            .await?;
            require_restart_readback(&restored, id)?;
        }
        checks.push("三台逐台切换原有环境、重启回读并立即切回原环境");
        Ok(())
    }
    .await;
    let outcome_error = outcome.as_ref().err().map(|error| safe_case_error(error));
    // 不因某台某字段失败而跳过其他字段、其他屏；先回原环境，再恢复本轮可能修改的字段。
    let mut cleanup_errors = Vec::new();
    for id in &ids {
        for field in &restoration[id] {
            let current = match app_config::read(&state, &project, &[id.clone()]).await {
                Ok(mut rows) => match rows.pop().and_then(|row| row.config) {
                    Some(config) => config,
                    None => {
                        cleanup_errors.push(format!("屏{id}还原前未读取到配置"));
                        continue;
                    }
                },
                Err(error) => {
                    cleanup_errors
                        .push(format!("屏{id}还原前读取失败：{}", safe_case_error(&error)));
                    continue;
                }
            };
            if inxaiot_desk_buddy_lib::domain::smart_screen::app_config::field_value(
                &current, field,
            ) == inxaiot_desk_buddy_lib::domain::smart_screen::app_config::field_value(
                &originals[id],
                field,
            ) {
                continue;
            }
            let restore_patch = restore(&originals[id], &[field.clone()]);
            if let Err(error) = restore_patch.validate() {
                cleanup_errors.push(format!(
                    "屏{id}字段{field}原值不受当前写入协议支持，未强行改写：{}",
                    safe_case_error(&error)
                ));
                continue;
            }
            if let Err(error) = execute(
                &state,
                &project,
                BTreeMap::from([(id.clone(), restore_patch)]),
            )
            .await
            {
                cleanup_errors.push(format!(
                    "屏{id}字段{field}还原失败：{}",
                    safe_case_error(&error)
                ));
            }
        }
    }
    let mut readonly_changes = BTreeMap::new();
    match app_config::read(&state, &project, &ids).await {
        Ok(rows) => {
            for row in rows {
                if let Some(config) = row.config.as_ref() {
                    readonly_changes.insert(
                        row.screen_id.clone(),
                        readonly_differences(&originals[&row.screen_id], config),
                    );
                }
                if row.config.as_ref().map(writable_projection)
                    != originals.get(&row.screen_id).map(writable_projection)
                {
                    cleanup_errors.push(format!(
                        "屏{}配置还原后仍与原值不同，原值保存在本机",
                        row.screen_id
                    ));
                }
            }
        }
        Err(error) => cleanup_errors.push(format!("还原后读取失败：{}", safe_case_error(&error))),
    }
    if cleanup_errors.is_empty() {
        eprintln!("{} 台屏配置已完整还原；平台业务库未使用", ids.len());
    }
    support::close(state).await;
    std::fs::write(
        dir.join("checks.json"),
        serde_json::to_vec_pretty(
            &json!({"passed":outcome.is_ok() && cleanup_errors.is_empty(),"screenCount":ids.len(),"checks":checks,"restored":cleanup_errors.is_empty(),"outcome_error":outcome_error,"cleanup_errors":cleanup_errors,"readonly_field_changes":readonly_changes}),
        )?,
    )?;
    if let Some(error) = outcome_error {
        return Err(format!(
            "配置测试未通过：{error}；{}",
            if cleanup_errors.is_empty() {
                "本轮修改已还原".into()
            } else {
                format!("另有还原错误：{}", cleanup_errors.join("；"))
            }
        )
        .into());
    }
    if !cleanup_errors.is_empty() {
        return Err(format!("测试还原未完成：{}", cleanup_errors.join("；")).into());
    }
    outcome
}

#[tokio::test]
#[ignore = "仅用于恢复明确指定的本轮配置验收目录：先核实旧未知任务，再按差异恢复原可写配置"]
async fn restore_configuration_baseline_from_evidence() -> Result<(), Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::domain::common::task::TaskState;
    use sqlx::Row;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".review-tools/full-regression-20261008/screen")
        .canonicalize()?;
    let requested = std::path::PathBuf::from(
        std::env::var_os("INX_SCREEN_RESTORE_CASE")
            .ok_or("缺少 INX_SCREEN_RESTORE_CASE，不执行恢复")?,
    );
    let directory = if requested.is_absolute() {
        requested
    } else {
        root.join(requested)
    }
    .canonicalize()?;
    if !directory.starts_with(&root)
        || !directory.is_dir()
        || !directory
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("configuration-batch-"))
        || !directory.join("local.db").is_file()
        || !directory.join("original-config.json").is_file()
    {
        return Err("仅允许恢复本轮证据目录内带原配置和本机数据库的真实配置验收记录".into());
    }
    let originals: BTreeMap<String, Value> =
        serde_json::from_slice(&std::fs::read(directory.join("original-config.json"))?)?;
    let state = support::state_at(&directory, true).await;
    let mut verified_tasks = Vec::new();
    let mut restored_fields: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut readonly_changes = BTreeMap::new();
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        let rows = sqlx::query(
            "SELECT local_project_id,id,fields_json FROM local_screen WHERE removed=0 ORDER BY id",
        )
        .fetch_all(state.local_store.pool())
        .await?;
        let allowed = support::live_screens()?;
        if rows.len() != originals.len() || rows.len() != allowed.len() {
            return Err("原配置、原本机屏和本轮允许的屏数量不一致，未执行恢复".into());
        }
        let project: String = rows
            .first()
            .ok_or("原本机记录为空")?
            .try_get("local_project_id")?;
        let mut ids = Vec::new();
        for row in rows {
            let id: String = row.try_get("id")?;
            let fields: ScreenFields =
                serde_json::from_str(&row.try_get::<String, _>("fields_json")?)?;
            if row.try_get::<String, _>("local_project_id")? != project
                || !originals.contains_key(&id)
                || !allowed
                    .iter()
                    .any(|screen| screen.ip == fields.ip && screen.size == fields.size)
            {
                return Err("原配置的ID、项目、IP或尺寸与本轮测试目标不匹配，未执行恢复".into());
            }
            ids.push(id);
        }
        let snapshot = ScreenRepository::new(state.local_store.pool().clone())
            .snapshot(&project)
            .await?;
        if snapshot
            .screens
            .iter()
            .any(|screen| screen.source != "local")
        {
            return Err("恢复用例只允许原本机未注册测试屏".into());
        }

        // tasks::verify只核实已发出的操作和补本机结果；不会重放saveConfig或自动再次重启。
        for task in state.task_repository.list_recent(&project, 500).await? {
            if task.operation_type != "app_config" {
                continue;
            }
            let results =
                task_data::read_results(state.local_store.pool(), &project, &task.id).await?;
            let unknown = results
                .targets
                .values()
                .any(|row| matches!(row.device, ResultState::Pending | ResultState::Unknown));
            if task.state == TaskState::FinalizingFailed
                || state.task_repository.results_protected(&task.id).await?
                || unknown
            {
                if matches!(
                    task.state,
                    TaskState::Queued | TaskState::Running | TaskState::Cancelling
                ) {
                    return Err("原配置任务仍处于执行状态，请先核实其执行进程；未执行还原".into());
                }
                tasks::verify(&state, &project, &task.id).await?;
                let confirmed =
                    task_data::read_results(state.local_store.pool(), &project, &task.id).await?;
                if state.task_repository.get(&task.id).await?.state == TaskState::FinalizingFailed
                    || state.task_repository.results_protected(&task.id).await?
                    || confirmed.targets.values().any(|row| {
                        matches!(row.device, ResultState::Pending | ResultState::Unknown)
                    })
                {
                    return Err("旧配置任务仍有未知结果，停止还原；不得重复保存或重启原操作".into());
                }
                verified_tasks.push(task.id);
            }
        }
        for id in &ids {
            // 先恢复原环境，再分别恢复各套配置；一次请求只包含一个明确字段。
            let mut fields = vec![
                "environments.current".to_string(),
                "customDeviceName".to_string(),
            ];
            for env in ["test", "pre", "prod"] {
                for field in ["otaUrl", "wsUrl", "h5Url", "h5ReadyCheckEnabled"] {
                    fields.push(format!("environments.{env}.{field}"));
                }
            }
            let mut current = app_config::read(&state, &project, &[id.clone()])
                .await?
                .into_iter()
                .next()
                .and_then(|row| row.config)
                .ok_or("还原前没有读取到当前配置，已停止")?;
            for field in fields {
                if inxaiot_desk_buddy_lib::domain::smart_screen::app_config::field_value(
                    &current, &field,
                ) == inxaiot_desk_buddy_lib::domain::smart_screen::app_config::field_value(
                    &originals[id],
                    &field,
                ) {
                    continue;
                }
                let correction = restore(&originals[id], &[field.clone()]);
                correction.validate().map_err(|error| {
                    format!(
                        "原值不能经当前协议精确还原，字段{field}，已停止：{}",
                        safe_case_error(&error)
                    )
                })?;
                // 这是操作者明确授权的反向修改；仅它确实需要生效时允许正常重启。
                let corrected =
                    execute(&state, &project, BTreeMap::from([(id.clone(), correction)])).await?;
                current = corrected.targets[id].evidence["config"]["after"].clone();
                if !current.is_object() {
                    return Err("还原操作没有完整回读结果，已停止后续恢复".into());
                }
                restored_fields.entry(id.clone()).or_default().push(field);
            }
        }
        for row in app_config::read(&state, &project, &ids).await? {
            let current = row.config.ok_or("还原结束后没有读取到配置")?;
            readonly_changes.insert(
                row.screen_id.clone(),
                readonly_differences(&originals[&row.screen_id], &current),
            );
            if writable_projection(&current) != writable_projection(&originals[&row.screen_id]) {
                return Err("还原后的可写配置仍与备份不一致".into());
            }
        }
        Ok(())
    }
    .await;
    support::close(state).await;
    let error = outcome.as_ref().err().map(|error| safe_case_error(error));
    std::fs::write(
        directory.join("restore-report.json"),
        serde_json::to_vec_pretty(&json!({
            "passed":outcome.is_ok(),"verifiedOldTasks":verified_tasks,"restoredFields":restored_fields,
            "readonlyFieldChanges":readonly_changes,"error":error,"originalOperationReplayed":false
        }))?,
    )?;
    if let Some(error) = error {
        return Err(format!("恢复原配置未完成：{error}").into());
    }
    eprintln!("三台屏可写配置已与本机备份核对一致；旧未知任务先核实，原配置操作未重复执行");
    Ok(())
}
