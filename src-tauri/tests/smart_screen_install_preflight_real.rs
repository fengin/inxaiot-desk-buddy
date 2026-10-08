#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::smart_screen::{model::ScreenFields, operation::ScreenOperationInput},
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{apk, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "真实只读核对测试说明中的屏与明确指定的APK，不安装应用"]
async fn actual_architecture_signature_and_same_version_are_checked()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let state = support::state_at(dir.path(), true).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    for screen in support::live_screens()? {
        ids.push(
            repo.save_local(
                &project,
                &ScreenFields {
                    name: screen.name,
                    ip: screen.ip,
                    size: screen.size,
                    ..Default::default()
                },
                None,
                None,
            )
            .await?,
        );
    }
    let package = apk::inspect(&support::live_apk_path()?, CancellationToken::new()).await?;
    let input = |targets: Vec<String>| ScreenOperationInput {
        action: "install".into(),
        target_ids: targets,
        application_id: Some("xiaoxin".into()),
        apk: Some(serde_json::to_value(&package).unwrap()),
        app_version: package.app_version.clone(),
        abi: "universal".into(),
        reinstall: false,
        concurrency: 1,
        retry_of_operation_id: None,
        expected_targets: Default::default(),
    };
    if tasks::preflight(&state, &project, input(ids.clone()))
        .await
        .is_ok()
    {
        return Err("不同尺寸不应进入安装检查".into());
    }
    for id in &ids {
        let result = tasks::preflight(&state, &project, input(vec![id.clone()])).await?;
        let row = &result.items[0];
        let installed = row
            .observation
            .as_ref()
            .and_then(|item| item.app_version_code)
            .ok_or("没有读到已安装小新的数字版本")?;
        let expected = match package.app_version_code.cmp(&installed) {
            std::cmp::Ordering::Less => "blocked",
            std::cmp::Ordering::Equal => "skip",
            std::cmp::Ordering::Greater => "ready",
        };
        if row.state != expected {
            return Err(format!(
                "{} 当前{installed}，目标{}，预期{expected}但实际{}：{}",
                row.name, package.app_version_code, row.state, row.reason
            )
            .into());
        }
        eprintln!(
            "{}：当前数字版本{installed}，目标{}，检查状态{expected}",
            row.name, package.app_version_code
        );
    }
    if !tasks::views(&state, &project).await?.is_empty() {
        return Err("只做检查不应产生安装执行任务".into());
    }
    support::close(state).await;
    Ok(())
}
