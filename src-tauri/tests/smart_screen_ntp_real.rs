#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{model::ScreenFields, ntp::NtpPatch, operation::ScreenOperationInput},
    },
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{ntp, task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::json;
use std::{collections::BTreeMap, time::Duration};

// 本测试只覆盖明确授权的三台屏。错误时间准备及人工断电由独立步骤完成，
// 测试失败或再次运行时不会自动改时间、断网或追加其他设备。
#[tokio::test]
#[ignore = "需要明确指定142时间源和set/clear操作，会通过正式任务设置并按需软重启86、87、61三台屏"]
async fn configure_and_verify_authorized_screens() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("INX_SCREEN_TEST_NTP_SERVER").as_deref() != Ok("192.168.3.142") {
        return Err("必须明确设置 INX_SCREEN_TEST_NTP_SERVER=192.168.3.142".into());
    }
    let mode = std::env::var("INX_SCREEN_TEST_NTP_ACTION")?;
    let server = match mode.as_str() {
        "set" => "192.168.3.142",
        "clear" => "",
        _ => return Err("操作必须明确为 set 或 clear".into()),
    };
    let authorized = [("192.168.3.86", "4"), ("192.168.3.87", "4"), ("192.168.3.61", "10")];
    let requested = std::env::var("INX_SCREEN_TEST_NTP_TARGETS").ok();
    let expected = match requested {
        Some(requested) => {
            let ips = requested.split(',').map(str::trim).collect::<Vec<_>>();
            if ips.is_empty() || ips.iter().collect::<std::collections::BTreeSet<_>>().len() != ips.len()
                || ips.iter().any(|ip| !authorized.iter().any(|(known, _)| known == ip))
            {
                return Err("目标子集只能明确选择61、86、87，不能包含重复或其他地址".into());
            }
            authorized.into_iter().filter(|(ip, _)| ips.contains(ip)).collect::<Vec<_>>()
        }
        None => authorized.to_vec(),
    };
    let live = support::live_screens()?;
    let directory = support::evidence_dir(&format!("ntp-{mode}"))?;
    let source_case = std::env::var_os("INX_SCREEN_TEST_NTP_SOURCE_CASE").map(std::path::PathBuf::from);
    let source = if let Some(source) = source_case {
        let source = source.canonicalize()?;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap()
            .join(".review-tools/ntp-20261010/formal").canonicalize()?;
        if !source.starts_with(root) || !source.join("local.db").is_file() {
            return Err("接续只能使用本轮formal证据目录中的隔离本机库".into());
        }
        Some(source)
    } else { None };
    let state = support::state_at(source.as_deref().unwrap_or(&directory), true).await;
    let mut report = json!({"passed":false,"mode":mode,"server":server});
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        let source_report = if let Some(source) = &source {
            Some(serde_json::from_slice::<serde_json::Value>(&std::fs::read(source.join("result.json"))?)?)
        } else { None };
        let project = if let Some(source) = &source_report {
            source["project"].as_str().ok_or("原项目编号缺失")?.to_string()
        } else {
            Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id
        };
        report["project"] = json!(project);
        report["sourceCase"] = json!(source);
        let repo = ScreenRepository::new(state.local_store.pool().clone());
        let original_plan = if let Some(source) = &source_report {
            Some(task_data::read_plan(state.local_store.pool(), &project, source["task"].as_str().ok_or("原任务编号缺失")?).await?.0)
        } else { None };
        let mut ids = Vec::new();
        for (ip, size) in expected {
            let screen = live
                .iter()
                .find(|screen| screen.ip == ip && screen.size == size)
                .ok_or("测试说明中的目标IP或尺寸与本轮授权不符，未执行")?;
            ids.push(if let Some(plan) = &original_plan {
                let target = plan.targets.iter().find(|target| target.fields.ip == ip && target.fields.size == size)
                    .ok_or("原任务不包含指定地址及尺寸，未接续")?;
                let current = repo.asset(&project, &target.id).await?;
                if current.fields.ip != ip || current.fields.size != size { return Err("原设备资料已变化，未接续".into()); }
                target.id.clone()
            } else {
                repo.save_local(
                    &project,
                    &ScreenFields {
                        name: screen.name.clone(),
                        ip: screen.ip.clone(),
                        size: screen.size.clone(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await?
            });
        }
        let before = ntp::read(&state, &project, &ids).await?;
        if before.iter().any(|row| row.config.is_none()) {
            return Err("至少一台屏原设置读取失败，未提交".into());
        }
        report["before"] = serde_json::to_value(&before)?;
        std::fs::write(directory.join("progress.json"), serde_json::to_vec_pretty(&report)?)?;
        let input: ScreenOperationInput = serde_json::from_value(json!({
            "action":"ntp", "targetIds":ids, "concurrency":3,
            "retryOfOperationId":source_report.as_ref().and_then(|source|source["task"].as_str()),
        }))?;
        let patches = input
            .target_ids
            .iter()
            .map(|id| (id.clone(), NtpPatch { server: server.into() }))
            .collect::<BTreeMap<_, _>>();
        let preview = ntp::preflight(&state, &project, input.clone(), patches).await?;
        report["preview"] = serde_json::to_value(&preview)?;
        if preview.items.iter().any(|item| item.state != "ready") {
            return Err("NTP设置预检未全部通过，未提交".into());
        }
        let task = tasks::submit(&state, &project, &preview.id, input.clone()).await?;
        report["task"] = json!(task);
        std::fs::write(directory.join("progress.json"), serde_json::to_vec_pretty(&report)?)?;
        tokio::time::timeout(Duration::from_secs(420), async {
            loop {
                let record = state.task_repository.get(&task).await.unwrap();
                if record.state.is_terminal() || record.state == TaskState::FinalizingFailed {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        })
        .await?;
        let results = task_data::read_results(state.local_store.pool(), &project, &task).await?;
        report["results"] = serde_json::to_value(&results)?;
        let after = ntp::read(&state, &project, &ids).await?;
        report["after"] = serde_json::to_value(&after)?;
        for row in &after {
            let config = row.config.as_ref().ok_or("执行后设置未读取成功")?;
            let original = before.iter().find(|old| old.screen_id == row.screen_id).unwrap();
            let original = original.config.as_ref().unwrap();
            if config.server != server
                || !config.auto_time
                || config.time_zone != original.time_zone
                || config.auto_time_zone != original.auto_time_zone
            {
                return Err("地址、自动校时或原时区设置回读不符合要求".into());
            }
        }
        let task_state = state.task_repository.get(&task).await?.state;
        report["taskState"] = json!(format!("{task_state:?}"));
        for target in results.targets.values() {
            eprintln!("{}：{}", target.screen_id, target.message);
        }
        // 清空恢复固件默认的网络源可能不可达，配置生效与实际授时分别验收。
        if mode == "set" && task_state != TaskState::Succeeded {
            return Err("指定内网时间源的实际授时尚未确认，请检查保留的逐台结果".into());
        }
        if mode == "clear"
            && results.targets.values().any(|target| {
                !matches!(target.evidence["ntp"]["save"].as_str(), Some("succeeded" | "unchanged"))
                    || target.evidence["ntp"]["activation"] != "succeeded"
            })
        {
            return Err("清空自定义地址的保存或生效未通过".into());
        }
        if mode == "set" {
            let patches = ids.iter().map(|id| (id.clone(), NtpPatch { server: server.into() })).collect();
            let preview = ntp::preflight(&state, &project, input.clone(), patches).await?;
            if preview.items.iter().any(|item| item.state != "ready") {
                return Err("同值验证预检未通过".into());
            }
            let repeated = tasks::submit(&state, &project, &preview.id, input).await?;
            report["sameValueTask"] = json!(repeated);
            std::fs::write(directory.join("progress.json"), serde_json::to_vec_pretty(&report)?)?;
            tokio::time::timeout(Duration::from_secs(180), async {
                loop {
                    let task = state.task_repository.get(&repeated).await.unwrap();
                    if task.state.is_terminal() || task.state == TaskState::FinalizingFailed { break; }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }).await?;
            let repeated_results = task_data::read_results(state.local_store.pool(), &project, &repeated).await?;
            report["sameValueResults"] = serde_json::to_value(&repeated_results)?;
            if state.task_repository.get(&repeated).await?.state != TaskState::Succeeded {
                return Err("已生效同值验证未通过".into());
            }
            for (id, target) in &repeated_results.targets {
                if target.evidence["ntp"]["save"] != "unchanged"
                    || target.evidence["ntp"]["rebootRequired"] != false
                    || target.evidence["ntp"]["afterBootId"] != results.targets[id].evidence["ntp"]["afterBootId"]
                {
                    return Err("已生效同值发生重复写入或重启".into());
                }
            }
        }
        Ok(())
    }
    .await;
    report["passed"] = json!(outcome.is_ok());
    report["error"] = json!(outcome.as_ref().err().map(|error| error.to_string()));
    support::close(state).await;
    std::fs::write(directory.join("result.json"), serde_json::to_vec_pretty(&report)?)?;
    eprintln!("本轮证据：{}", directory.display());
    outcome
}
