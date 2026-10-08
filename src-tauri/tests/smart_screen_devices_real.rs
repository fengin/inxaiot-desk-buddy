#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::{model::ScreenFields, operation::ScreenOperationInput},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};

#[tokio::test]
#[ignore = "只读检查测试说明中的 4 寸和 10 寸屏，不修改设备或平台业务库"]
async fn configured_screens_run_through_real_task_queue_and_preserve_observations()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = support::evidence_dir("device-inspection-and-mac")?;
    let state = support::state_at(&dir, true).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let screens = support::live_screens()?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    for screen in &screens {
        ids.push(
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
            .await?,
        );
    }
    let input = ScreenOperationInput {
        action: "inspect".into(),
        target_ids: ids.clone(),
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 2,
        retry_of_operation_id: None,
        expected_targets: Default::default(),
    };
    let preview = tasks::preflight(&state, &project, input.clone()).await?;
    assert!(preview.items.iter().all(|i| i.state == "ready"));
    let id = tasks::submit(&state, &project, &preview.id, input.clone()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            if state
                .task_repository
                .get(&id)
                .await
                .unwrap()
                .state
                .is_terminal()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await?;
    let results = task_data::read_results(state.local_store.pool(), &project, &id).await?;
    for target in results.targets.values() {
        eprintln!("设备结果：{}；{}", target.screen_id, target.message);
        let observation = target.observation.as_ref().ok_or("没有设备实测")?;
        eprintln!(
            "Android={:?}; ABI={:?}; 应用版本={:?}; 错误={:?}",
            observation.android,
            observation.abis,
            observation.observed_app_version,
            observation.errors
        );
        assert!(observation.adb_available);
        assert!(observation.observed_mac.is_some());
        assert!(!observation.abis.is_empty());
        assert!(observation.app_version_code.is_some());
        assert!(observation.observed_app_version.is_some());
        assert!(observation.device_time.is_some());
        assert!(observation.computer_time.is_some());
        assert!(observation.timezone.is_some());
        assert!(observation.automatic_time.is_some());
        assert!(observation.automatic_timezone.is_some());
        assert!(observation.errors.is_empty());
    }
    let snapshot = repo.snapshot(&project).await?;
    assert_eq!(snapshot.observations.len(), screens.len());
    assert!(snapshot.screens.iter().all(|s| s.app_version.is_none()));
    let task = state.task_repository.get(&id).await?;
    assert!(task.remote_operation_record_id.is_none());
    let mac_input = ScreenOperationInput {
        action: "mac".into(),
        ..input
    };
    let mac_preview = tasks::preflight(&state, &project, mac_input.clone()).await?;
    assert!(mac_preview.items.iter().all(|item| item.state == "ready"));
    let mac_task = tasks::submit(&state, &project, &mac_preview.id, mac_input).await?;
    tokio::time::timeout(std::time::Duration::from_secs(120), async {
        loop {
            let status = state.task_repository.get(&mac_task).await.unwrap().state;
            if status.is_terminal()
                || status
                    == inxaiot_desk_buddy_lib::domain::common::task::TaskState::FinalizingFailed
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await?;
    let mac_results =
        task_data::read_results(state.local_store.pool(), &project, &mac_task).await?;
    let after_mac = repo.snapshot(&project).await?;
    assert_eq!(after_mac.screens.len(), screens.len());
    assert_eq!(after_mac.observations.len(), screens.len());
    for screen_id in &ids {
        let result = &mac_results.targets[screen_id];
        assert_eq!(
            result.device,
            inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState::Succeeded
        );
        let mac = result
            .observation
            .as_ref()
            .ok_or("MAC任务没有保留观察结果")?;
        let original = results.targets[screen_id]
            .observation
            .as_ref()
            .ok_or("原设备检查缺失")?;
        assert!(mac.observed_mac.is_some());
        assert_eq!(mac.observed_mac, original.observed_mac);
        assert_eq!(mac.operation_type, "mac");
        assert!(
            after_mac.observations[screen_id]
                .iter()
                .any(|row| row.id == mac.id)
        );
        let preserved = after_mac.observations[screen_id]
            .iter()
            .find(|row| row.id == original.id)
            .ok_or("MAC采集抹掉了原设备检查记录")?;
        assert_eq!(
            serde_json::to_value(preserved)?,
            serde_json::to_value(original)?
        );
    }
    assert_eq!(tasks::views(&state, &project).await?.len(), 2);
    std::fs::write(
        dir.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":true,"screenCount":screens.len(),"inspectTask":id,"macTask":mac_task,"macMatchesInspection":true,"inspectionHistoryPreserved":true,"businessWrites":false}),
        )?,
    )?;
    eprintln!("三屏独立MAC采集通过，与检查设备的MAC一致，原版本/系统/时间等历史信息保留");
    support::close(state).await;
    Ok(())
}
