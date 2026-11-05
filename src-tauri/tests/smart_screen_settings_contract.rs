#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::{
        ports::project_management::ProjectManagementPort, smart_screen::operations::validate_input,
    },
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::{ScreenAsset, ScreenFields},
            operation::{ScreenOperationInput, ScreenPlan, WRITE_ACTIONS, action_label},
            settings::ScreenSettingsPatch,
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::{
            screen_repository::{ScreenRepository, now},
            task_repository::CreateTask,
        },
        smart_screen::{device::AdbDevice, maintenance, task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::json;
use std::collections::BTreeMap;

fn input(action: &str, screen: &str) -> ScreenOperationInput {
    ScreenOperationInput {
        action: action.into(),
        target_ids: vec![screen.into()],
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: String::new(),
        reinstall: false,
        concurrency: 1,
        expected_targets: BTreeMap::new(),
        retry_of_operation_id: None,
    }
}

#[test]
fn default_settings_select_three_items_and_leave_ntp_unselected() {
    let patch = ScreenSettingsPatch::default();
    assert!(patch.adb && patch.time && patch.sleep_off);
    assert!(patch.ntp.is_none());
    assert_eq!(
        serde_json::to_value(&patch).unwrap(),
        json!({
            "adb":true,"time":true,"sleepOff":true,"ntp":null
        })
    );
    assert!(patch.selected("sleepOff"));
    assert!(!patch.selected("ntp"));
    assert!(!patch.selected("unknown"));
}

#[test]
fn ntp_null_and_explicit_empty_server_have_different_selected_meanings() {
    let omitted: ScreenSettingsPatch = serde_json::from_value(json!({
        "adb":false,"time":false,"sleepOff":false,"ntp":null
    }))
    .unwrap();
    let default_server: ScreenSettingsPatch = serde_json::from_value(json!({
        "adb":false,"time":false,"sleepOff":false,"ntp":{"server":""}
    }))
    .unwrap();
    assert!(!omitted.selected("ntp"));
    assert!(default_server.selected("ntp"));
    assert_eq!(default_server.ntp.as_ref().unwrap().server(), "");
    assert!(omitted.validate().is_ok());
    assert!(default_server.validate().is_ok());
    assert_ne!(omitted, default_server);
}

#[test]
fn settings_wire_contract_rejects_unknown_fields_missing_switches_and_command_text() {
    for value in [
        json!({"adb":true,"time":true,"sleepOff":true,"ntp":null,"reboot":true}),
        json!({"adb":true,"time":true,"ntp":null}),
        json!({"adb":"true","time":true,"sleepOff":true,"ntp":null}),
    ] {
        assert!(serde_json::from_value::<ScreenSettingsPatch>(value).is_err());
    }
    let patch: ScreenSettingsPatch = serde_json::from_value(json!({
        "adb":false,"time":false,"sleepOff":false,"ntp":{"server":"ntp;reboot"}
    }))
    .unwrap();
    assert!(patch.validate().is_err());
}

#[test]
fn unified_action_is_registered_as_a_valid_write_operation() {
    assert!(WRITE_ACTIONS.contains(&"screen_settings"));
    assert_eq!(action_label("screen_settings"), "设置智能屏");
    assert!(validate_input(&input("screen_settings", "screen-1")).is_ok());
}

async fn frozen_task(
    state: &FormalAppState,
    project: &str,
    screen: &ScreenAsset,
    action: &str,
    detail: serde_json::Value,
) -> (String, ScreenPlan) {
    let id = uuid::Uuid::now_v7().to_string();
    let plan = ScreenPlan {
        project_id: project.into(),
        input: input(action, &screen.id),
        targets: vec![screen.clone()],
        business_project_id: None,
        data_source_id: None,
        operator: "隔离契约测试".into(),
        instance_id: "settings-contract".into(),
        created_at: now(),
        detail,
    };
    task_data::save_plan(state.local_store.pool(), &id, &plan)
        .await
        .unwrap();
    state
        .task_repository
        .create(CreateTask {
            id: id.clone(),
            local_project_id: project.into(),
            remote_operation_record_id: None,
            domain_type: tasks::DOMAIN.into(),
            operation_type: action.into(),
            name: action_label(action).into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: state
                .paths
                .project_task_log_path(project, &id)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            targets: vec![(tasks::DOMAIN.into(), screen.id.clone())],
        })
        .await
        .unwrap();
    (id, plan)
}

async fn project_and_screen(state: &FormalAppState) -> (String, ScreenAsset) {
    let project = Stage75Adapter::new(state)
        .create_project(support::local_input())
        .await
        .unwrap()
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let id = repo
        .save_local(
            &project,
            &ScreenFields {
                name: "隔离屏".into(),
                ip: "192.0.2.4".into(),
                size: "4".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let screen = repo.asset(&project, &id).await.unwrap();
    (project, screen)
}

#[tokio::test]
async fn known_legacy_retry_import_preserves_terminal_target_project_and_protection_gates() {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let (project, screen) = project_and_screen(&state).await;
    for action in ["time", "adb", "sleep_off", "ntp", "reboot"] {
        let (id, _) = frozen_task(&state, &project, &screen, action, json!(null)).await;
        let mut retry = input("screen_settings", &screen.id);
        retry.retry_of_operation_id = Some(id.clone());
        assert!(
            tasks::validate_retry(&state, &project, &retry)
                .await
                .is_err()
        );
        state
            .task_repository
            .transition(&id, TaskState::Draft, TaskState::Checking, None, None)
            .await
            .unwrap();
        state
            .task_repository
            .transition(&id, TaskState::Checking, TaskState::Failed, None, None)
            .await
            .unwrap();
        assert_eq!(
            tasks::validate_retry(&state, &project, &retry)
                .await
                .is_ok(),
            ["time", "adb", "sleep_off"].contains(&action)
        );
        retry.action = action.into();
        assert!(
            tasks::validate_retry(&state, &project, &retry)
                .await
                .is_ok()
        );
        assert!(
            tasks::validate_retry(&state, "other-project", &retry)
                .await
                .is_err()
        );
        retry.target_ids = vec!["not-in-original-task".into()];
        assert!(
            tasks::validate_retry(&state, &project, &retry)
                .await
                .is_err()
        );
        retry.target_ids = vec![screen.id.clone()];
        state
            .task_repository
            .protect_results(&id, "隔离待核实结果")
            .await
            .unwrap();
        assert!(
            tasks::validate_retry(&state, &project, &retry)
                .await
                .is_err()
        );
    }
    support::close(state).await;
}

#[tokio::test]
async fn task_view_reads_only_frozen_settings_and_rejects_a_damaged_patch() {
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let (project, screen) = project_and_screen(&state).await;
    let expected = BTreeMap::from([(
        screen.id.clone(),
        ScreenSettingsPatch {
            adb: false,
            time: true,
            sleep_off: false,
            ntp: None,
        },
    )]);
    let (id, _) = frozen_task(
        &state,
        &project,
        &screen,
        "screen_settings",
        json!({"settings":expected}),
    )
    .await;
    let views = tasks::views(&state, &project).await.unwrap();
    let view = views.iter().find(|view| view.id == id).unwrap();
    assert_eq!(view.settings.as_ref(), Some(&expected));
    let mut damaged = json!({"settings":expected});
    damaged["settings"][&screen.id]["time"] = json!("invalid");
    frozen_task(&state, &project, &screen, "screen_settings", damaged).await;
    assert!(tasks::views(&state, &project).await.is_err());
    support::close(state).await;
}

#[tokio::test]
async fn imported_retries_reject_added_items_and_completed_or_unknown_children_before_device_access()
 {
    use inxaiot_desk_buddy_lib::{
        core::error::AppResult, domain::smart_screen::model::ScreenTargetResult,
    };
    let temp = tempfile::tempdir().unwrap();
    let state = support::state_at(temp.path(), false).await;
    let (project, screen) = project_and_screen(&state).await;
    for action in ["time", "adb", "sleep_off", "screen_settings"] {
        let detail = if action == "screen_settings" {
            json!({"settings":{(screen.id.clone()):{"adb":false,"time":true,"sleepOff":false,"ntp":null}}})
        } else {
            json!(null)
        };
        let (id, _) = frozen_task(&state, &project, &screen, action, detail).await;
        state
            .task_repository
            .transition(&id, TaskState::Draft, TaskState::Checking, None, None)
            .await
            .unwrap();
        state
            .task_repository
            .transition(&id, TaskState::Checking, TaskState::Failed, None, None)
            .await
            .unwrap();
        let mut request = input("screen_settings", &screen.id);
        request.retry_of_operation_id = Some(id.clone());
        let patches = BTreeMap::from([(screen.id.clone(), ScreenSettingsPatch::default())]);
        let error = maintenance::preflight_settings_with_device_factory(
            &state,
            &project,
            request.clone(),
            None,
            None,
            Some(patches),
            || -> AppResult<AdbDevice> { panic!("非法重试必须在设备访问前拒绝") },
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains(if action == "screen_settings" {
            "原任务未选择"
        } else {
            "独立操作重试只能"
        }));
        if action == "screen_settings" {
            for child_state in ["succeeded", "skipped", "unknown", "pending"] {
                task_data::save_target(state.local_store.pool(),&project,&id,&ScreenTargetResult {
                    screen_id:screen.id.clone(),evidence:json!({"screenSettings":{"items":{"time":{"state":child_state}}}}),..Default::default()
                }).await.unwrap();
                let patches = BTreeMap::from([(
                    screen.id.clone(),
                    ScreenSettingsPatch {
                        adb: false,
                        time: true,
                        sleep_off: false,
                        ntp: None,
                    },
                )]);
                let error = maintenance::preflight_settings_with_device_factory(
                    &state,
                    &project,
                    request.clone(),
                    None,
                    None,
                    Some(patches),
                    || -> AppResult<AdbDevice> {
                        panic!("已完成或待核实子项不能重复访问设备")
                    },
                )
                .await
                .unwrap_err();
                assert!(error.to_string().contains(
                    if ["succeeded", "skipped"].contains(&child_state) {
                        "子项已经完成"
                    } else {
                        "尚未确认"
                    }
                ));
            }
        }
    }
    support::close(state).await;
}
