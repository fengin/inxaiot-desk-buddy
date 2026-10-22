//! 配置尺寸门禁只使用隔离本机库。每例在独立子进程禁用ADB发现，防止回归时触达设备。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::AppError,
    domain::{common::task::TaskState, smart_screen::{
        app_config::AppConfigPatch,
        model::{ResultState, ScreenAsset, ScreenFields, ScreenTargetResult},
        operation::{ScreenOperationInput, ScreenPlan, ScreenResults},
    }},
    formal::app_state::FormalAppState,
    infrastructure::{local_sqlite::screen_repository::{ScreenRepository, now},
        smart_screen::{app_config, device::AndroidTools, maintenance, previews, task_data, tasks},
        stage75_adapter::Stage75Adapter},
    runtime::task_queue::TaskEnvelope,
};
use serde_json::json;
use std::{collections::BTreeMap, future::Future, sync::Arc};
use tokio_util::sync::CancellationToken;

const MIXED: &str = "4 寸屏和 10 寸屏不能同时修改配置，请分批选择。";
const UNKNOWN: &str = "请先确认所选屏尺寸";

fn isolated(name: &str, case: impl Future<Output = ()>) {
    if std::env::var("INX_CONFIG_SIZE_TEST_CHILD").as_deref() == Ok(name) {
        assert!(matches!(AndroidTools::discover(), Err(AppError::InvalidConfig(message)) if message == "配置的 ADB 工具不存在"));
        tokio::runtime::Runtime::new().unwrap().block_on(case);
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture", "--test-threads=1"])
        .env("INX_CONFIG_SIZE_TEST_CHILD", name)
        .env("INX_ADB_PATH", temp.path().join("missing-adb.exe"))
        .output().unwrap();
    assert!(output.status.success(), "隔离门禁用例失败：{}\n{}",
        String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
}

async fn setup(sizes: &[&str]) -> (tempfile::TempDir, Arc<FormalAppState>, String, Vec<ScreenAsset>) {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let project = Stage75Adapter::new(&state).create_project(support::local_input()).await.unwrap().project.id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut targets = Vec::new();
    for (index, size) in sizes.iter().enumerate() {
        let id = repo.save_local(&project, &ScreenFields {
            name: format!("尺寸隔离屏{index}"), ip: format!("192.0.2.{}", index + 10),
            size: (*size).into(), ..Default::default()
        }, None, None).await.unwrap();
        targets.push(repo.asset(&project, &id).await.unwrap());
    }
    (temp, state, project, targets)
}

fn input(targets: &[ScreenAsset]) -> ScreenOperationInput {
    ScreenOperationInput { action: "app_config".into(), target_ids: targets.iter().map(|screen| screen.id.clone()).collect(),
        application_id: None, apk: None, app_version: String::new(), abi: "universal".into(),
        reinstall: false, concurrency: 1, expected_targets: Default::default(), retry_of_operation_id: None }
}

fn patches(targets: &[ScreenAsset]) -> BTreeMap<String, AppConfigPatch> {
    targets.iter().map(|screen| (screen.id.clone(), AppConfigPatch {
        set: BTreeMap::from([("autoStartVoiceEngine".into(), json!(false))]), ..Default::default()
    })).collect()
}

fn plan(project: &str, targets: Vec<ScreenAsset>) -> ScreenPlan {
    ScreenPlan { project_id: project.into(), input: input(&targets),
        detail: serde_json::to_value(maintenance::MaintenancePlan { apk: None, observations: Default::default(),
            request_ids: Default::default(), app_config: Some(patches(&targets)), ntp: None }).unwrap(),
        targets, business_project_id: None, data_source_id: None,
        operator: "隔离门禁测试".into(), instance_id: "size-test".into(), created_at: now() }
}

async fn save_preview(state: &FormalAppState, plan: &ScreenPlan) -> String {
    let id = uuid::Uuid::now_v7().to_string();
    previews::save(state, &id, plan, maintenance::PREVIEW).await.unwrap();
    id
}

async fn assert_execution_count(state: &FormalAppState, expected: i64) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task WHERE operation_type='app_config'")
        .fetch_one(state.local_store.pool()).await.unwrap();
    assert_eq!(count, expected);
}

fn assert_size_error(error: AppError, expected: &str) {
    assert!(matches!(error, AppError::InvalidConfig(ref message) if message == expected), "{error}");
}

#[test]
fn read_resolves_and_checks_entire_batch_before_discovering_adb() {
    isolated("read_resolves_and_checks_entire_batch_before_discovering_adb", async {
        for (sizes, message) in [(&["4", "10"][..], MIXED), (&["10", "4"][..], MIXED),
            (&["4", "unknown"][..], UNKNOWN), (&["unknown"][..], UNKNOWN)] {
            let (_temp, state, project, targets) = setup(sizes).await;
            let error = app_config::read(&state, &project, &input(&targets).target_ids).await.unwrap_err();
            assert_size_error(error, message);
            assert_execution_count(&state, 0).await;
            assert!(state.task_repository.list_recent(&project, 10).await.unwrap().is_empty());
            let mut ids = input(&targets).target_ids; ids.push("removed-target".into());
            assert!(matches!(app_config::read(&state, &project, &ids).await.unwrap_err(), AppError::NotFound(_)));
            support::close(state).await;
        }
    });
}

#[test]
fn preflight_rejects_mixed_or_unknown_sizes_without_preview_or_device_access() {
    isolated("preflight_rejects_mixed_or_unknown_sizes_without_preview_or_device_access", async {
        for (sizes, message) in [(&["4", "10"][..], MIXED), (&["10", "unknown"][..], UNKNOWN), (&["unknown"][..], UNKNOWN)] {
            let (_temp, state, project, targets) = setup(sizes).await;
            let error = maintenance::preflight_with_config(&state, &project, input(&targets), Some(patches(&targets))).await.unwrap_err();
            assert_size_error(error, message);
            assert!(state.task_repository.list_recent(&project, 10).await.unwrap().is_empty());
            let plans: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_screen_task_data").fetch_one(state.local_store.pool()).await.unwrap();
            assert_eq!(plans, 0);
            assert_execution_count(&state, 0).await;
            support::close(state).await;
        }
    });
}

#[test]
fn submit_refuses_old_mixed_previews_even_when_only_one_target_is_selected() {
    isolated("submit_refuses_old_mixed_previews_even_when_only_one_target_is_selected", async {
        for (sizes, message) in [(&["4", "10"][..], MIXED), (&["4", "unknown"][..], UNKNOWN)] {
            let (_temp, state, project, targets) = setup(sizes).await;
            let old = plan(&project, targets.clone());
            let preview = save_preview(&state, &old).await;
            for chosen in [targets.clone(), vec![targets[0].clone()]] {
                assert_size_error(tasks::submit(&state, &project, &preview, input(&chosen)).await.unwrap_err(), message);
                assert_execution_count(&state, 0).await;
            }
            let (saved, _) = task_data::read_preflight_plan(state.local_store.pool(), &project, &preview).await.unwrap();
            assert_eq!(saved.targets, old.targets);
            let mut partially_passed = old.clone();
            partially_passed.targets.truncate(1);
            let partial_preview = save_preview(&state, &partially_passed).await;
            assert_size_error(tasks::submit(&state, &project, &partial_preview, input(&targets[..1])).await.unwrap_err(), message);
            assert_execution_count(&state, 0).await;
            support::close(state).await;
        }
    });
}

#[test]
fn submit_rechecks_latest_sizes_and_same_target_before_creating_execution() {
    isolated("submit_rechecks_latest_sizes_and_same_target_before_creating_execution", async {
        for after in [vec!["4", "10"], vec!["4", "unknown"], vec!["10", "10"]] {
            let (_temp, state, project, targets) = setup(&["4", "4"]).await;
            let preview = save_preview(&state, &plan(&project, targets.clone())).await;
            let repo = ScreenRepository::new(state.local_store.pool().clone());
            for (target, size) in targets.iter().zip(&after) {
                let mut fields = target.fields.clone(); fields.size = (*size).into();
                repo.save_local(&project, &fields, Some(&target.id), Some(target.revision)).await.unwrap();
            }
            let error = tasks::submit(&state, &project, &preview, input(&targets)).await.unwrap_err();
            if after == ["10", "10"] {
                assert!(matches!(error, AppError::Conflict(message) if message == "设备关键资料已变化或尚未提交"));
            } else { assert_size_error(error, if after.contains(&"unknown") { UNKNOWN } else { MIXED }); }
            assert_execution_count(&state, 0).await;
            support::close(state).await;
        }
    });
}

fn envelope(project: &str, id: &str) -> TaskEnvelope {
    TaskEnvelope { local_task_id: id.into(), local_project_id: project.into(), domain_type: "smart_screen".into(),
        operation_type: "app_config".into(), resource_keys: vec![], priority: 0, payload_ref: None, payload_sha256: None }
}

async fn old_queued(state: &FormalAppState, plan: &ScreenPlan) -> String {
    let preview = save_preview(state, plan).await;
    let (_, hash) = task_data::read_preflight_plan(state.local_store.pool(), &plan.project_id, &preview).await.unwrap();
    // 模拟此前版本已经排队的任务，刻意绕过本轮submit门禁；不注册真实设备执行器。
    previews::queue(state, &preview, maintenance::PREVIEW, &hash, plan).await.unwrap()
}

#[test]
fn old_queued_mixed_batch_finishes_without_adb_or_shared_writes() {
    isolated("old_queued_mixed_batch_finishes_without_adb_or_shared_writes", async {
        for platform in [false, true] {
            let (_temp, state, project, mut targets) = setup(&["4", "10"]).await;
            if platform { targets[0].source = "platform".into(); }
            let old = plan(&project, targets);
            let task = old_queued(&state, &old).await;
            assert!(state.task_repository.results_protected(&task).await.unwrap());
            maintenance::run(&state, envelope(&project, &task), CancellationToken::new()).await.unwrap();
            let record = state.task_repository.get(&task).await.unwrap();
            assert_eq!(record.state, TaskState::Failed);
            let results = task_data::read_results(state.local_store.pool(), &project, &task).await.unwrap();
            assert!(results.finished);
            assert_eq!(results.targets.len(), 2);
            for result in results.targets.values() {
                assert_eq!(result.device, ResultState::Failed);
                assert_eq!(result.business, ResultState::NotRequired);
                assert_eq!(result.shared, ResultState::NotRequired);
                assert_eq!(result.evidence["phase"], "config_batch_blocked");
                assert_eq!(result.evidence["config"]["save"], "not_required");
                assert!(result.message.contains(MIXED));
            }
            assert!(!state.task_repository.results_protected(&task).await.unwrap());
            assert!(ScreenRepository::new(state.local_store.pool().clone()).snapshot(&project).await.unwrap().observations.is_empty());
            support::close(state).await;
        }
    });
}

#[test]
fn queued_batch_rechecks_size_changes_but_preserves_any_existing_execution_evidence() {
    isolated("queued_batch_rechecks_size_changes_but_preserves_any_existing_execution_evidence", async {
        for after in [vec!["4", "10"], vec!["10", "10"]] {
            let (_temp, state, project, targets) = setup(&["4", "4"]).await;
            let old = plan(&project, targets.clone());
            // 本机活动任务触发器本身禁止改尺寸；先改资料，再模拟旧版本将过时快照排入队列。
            for (target, size) in targets.iter().zip(&after) {
                let mut changed = target.fields.clone(); changed.size = (*size).into();
                ScreenRepository::new(state.local_store.pool().clone()).save_local(&project, &changed,
                    Some(&target.id), Some(target.revision)).await.unwrap();
            }
            let task = old_queued(&state, &old).await;
            maintenance::run(&state, envelope(&project, &task), CancellationToken::new()).await.unwrap();
            assert_eq!(state.task_repository.get(&task).await.unwrap().state, TaskState::Failed);
            let results = task_data::read_results(state.local_store.pool(), &project, &task).await.unwrap();
            assert!(results.targets.values().all(|result| result.evidence["phase"] == "config_batch_blocked"));
            if after == ["10", "10"] {
                assert!(results.targets.values().all(|result| result.message.contains("设备关键资料已变化")));
            }
            assert!(!state.task_repository.results_protected(&task).await.unwrap());
            support::close(state).await;
        }

        for result_state in [ResultState::Unknown, ResultState::Succeeded] {
            let (_temp, state, project, targets) = setup(&["4", "10"]).await;
            let old = plan(&project, targets.clone());
            let task = old_queued(&state, &old).await;
            let before = ScreenResults { targets: BTreeMap::from([(targets[0].id.clone(), ScreenTargetResult {
                screen_id: targets[0].id.clone(), device: result_state, business: ResultState::NotRequired,
                shared: ResultState::NotRequired, evidence: json!({"phase":"config_save_requested","config":{"save":"unknown"}}),
                ..Default::default()
            })]), finished: false };
            task_data::save_results(state.local_store.pool(), &project, &task, &before).await.unwrap();
            let error = maintenance::run(&state, envelope(&project, &task), CancellationToken::new()).await.unwrap_err();
            assert!(matches!(error, AppError::Conflict(message) if message.contains("已有执行记录")));
            assert_eq!(serde_json::to_value(task_data::read_results(state.local_store.pool(), &project, &task).await.unwrap()).unwrap(), serde_json::to_value(before).unwrap());
            assert_eq!(state.task_repository.get(&task).await.unwrap().state, TaskState::Queued);
            assert!(state.task_repository.results_protected(&task).await.unwrap());
            support::close(state).await;
        }
    });
}

#[test]
fn same_size_keeps_legacy_capability_flow_and_other_actions_keep_their_rules() {
    isolated("same_size_keeps_legacy_capability_flow_and_other_actions_keep_their_rules", async {
        for size in ["4", "10"] {
            let (_temp, state, project, targets) = setup(&[size, size]).await;
            // 同尺寸到达原ADB能力发现阶段，尺寸门禁不按APK版本或字段定义拒绝。
            assert!(matches!(app_config::read(&state, &project, &input(&targets).target_ids).await.unwrap_err(), AppError::InvalidConfig(message) if message == "配置的 ADB 工具不存在"));
            assert!(matches!(maintenance::preflight_with_config(&state, &project, input(&targets), Some(patches(&targets))).await.unwrap_err(), AppError::InvalidConfig(message) if message == "配置的 ADB 工具不存在"));
            let old = plan(&project, targets.clone());
            let preview = save_preview(&state, &old).await;
            tasks::submit(&state, &project, &preview, input(&targets)).await.unwrap();
            assert_execution_count(&state, 1).await;
            support::close(state).await;
        }
        let (_temp, state, project, targets) = setup(&["4", "10"]).await;
        let mut install = input(&targets); install.action = "install".into(); install.application_id = Some("xiaoxin".into());
        assert!(matches!(maintenance::preflight(&state, &project, install).await.unwrap_err(), AppError::InvalidConfig(message) if message == "一批安装只能选择同一种明确尺寸，包括当前筛选隐藏的目标"));
        let mut ntp = plan(&project, targets.clone()); ntp.input.action = "ntp".into();
        ntp.detail = serde_json::to_value(maintenance::MaintenancePlan { apk: None, observations: Default::default(), request_ids: Default::default(), app_config: None,
            ntp: Some(targets.iter().map(|screen| (screen.id.clone(), inxaiot_desk_buddy_lib::domain::smart_screen::ntp::NtpPatch { server: "192.0.2.142".into() })).collect()) }).unwrap();
        let preview = save_preview(&state, &ntp).await;
        tasks::submit(&state, &project, &preview, ntp.input.clone()).await.unwrap();
        assert_execution_count(&state, 0).await;
        support::close(state).await;
    });
}
