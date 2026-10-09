#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{model::ScreenFields, operation::ScreenOperationInput},
    },
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            apk,
            device::{AdbDevice, AndroidTools},
            maintenance, task_data,
        },
        stage75_adapter::Stage75Adapter,
    },
};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;

async fn configuration(
    device: &AdbDevice,
    ip: &str,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let text = device
        .shell(
            ip,
            &[
                "su",
                "0",
                "cat",
                "/data/data/chat.xiaoxin.app/shared_prefs/FlutterSharedPreferences.xml",
            ],
            CancellationToken::new(),
        )
        .await?;
    // 只比较原有稳定配置的摘要，不输出或保存应用凭据。
    let mut values = BTreeMap::new();
    for key in [
        "device_id",
        "client_id",
        "custom_device_name",
        "auth_code",
        "config_password",
        "audio_config",
        "extend_params",
    ] {
        let pattern = format!(
            r#"(?s)<(?:string|boolean|int|long|float) name="flutter\.{}"[^>]*(?:/>|>.*?</(?:string|boolean|int|long|float)>)"#,
            key
        );
        if let Some(item) = regex::Regex::new(&pattern)?.find(&text) {
            values.insert(
                key.into(),
                hex::encode(Sha256::digest(item.as_str().as_bytes())),
            );
        }
    }
    if values.is_empty() {
        return Err("未读取到可比较的原有配置，停止安装验收".into());
    }
    Ok(values)
}
#[tokio::test]
#[ignore = "按测试说明对同尺寸屏批量安装明确指定的APK，保留小新数据并核对配置；不卸载、不降级"]
async fn same_size_batch_upgrade_preserves_configuration_and_verifies_running()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = support::evidence_dir("batch-install")?;
    let state = support::state_at(&directory, true).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let device = AdbDevice::new(AndroidTools::discover()?);
    let package = apk::inspect(&support::live_apk_path()?, CancellationToken::new()).await?;
    let screens = support::live_screens()?;
    for size in ["4", "10"] {
        if std::env::var("INX_SCREEN_INSTALL_SIZE")
            .ok()
            .is_some_and(|selected| selected != size)
        {
            continue;
        }
        let batch = screens
            .iter()
            .filter(|screen| screen.size == size)
            .collect::<Vec<_>>();
        if batch.is_empty() {
            continue;
        }
        if size == "4" && batch.len() < 2 {
            return Err("4寸同尺寸批量验收至少需要两台不同的4寸屏".into());
        }
        let mut originals = BTreeMap::new();
        let mut addresses = BTreeMap::new();
        for screen in batch {
            device.connect(&screen.ip, CancellationToken::new()).await?;
            let original = configuration(&device, &screen.ip).await?;
            let id = repo
                .save_local(
                    &project,
                    &ScreenFields {
                        name: screen.name.clone(),
                        ip: screen.ip.clone(),
                        size: size.into(),
                        ..Default::default()
                    },
                    None,
                    None,
                )
                .await?;
            originals.insert(id.clone(), original);
            addresses.insert(id, screen.ip.clone());
        }
        let input = ScreenOperationInput {
            action: "install".into(),
            target_ids: addresses.keys().cloned().collect(),
            application_id: Some("xiaoxin".into()),
            apk: Some(serde_json::to_value(&package)?),
            app_version: package.app_version.clone(),
            abi: "universal".into(),
            reinstall: true,
            concurrency: 2,
            retry_of_operation_id: None,
            expected_targets: Default::default(),
        };
        eprintln!("{size}寸：开始真实设备与安装条件检查");
        let preview = maintenance::preflight(&state, &project, input.clone()).await?;
        if let Some(blocked) = preview.items.iter().find(|item| item.state != "ready") {
            return Err(format!("安装检查未通过：{} {}", blocked.name, blocked.reason).into());
        }
        let task = maintenance::submit(&state, &project, &preview.id, input).await?;
        eprintln!("{size}寸：执行保留数据覆盖安装");
        let mut samples: BTreeMap<String, Vec<(String, u32, String)>> = BTreeMap::new();
        tokio::time::timeout(Duration::from_secs(1200), async {
            loop {
                let value = state.task_repository.get(&task).await.unwrap();
                let view = inxaiot_desk_buddy_lib::infrastructure::smart_screen::tasks::views(
                    &state, &project,
                )
                .await
                .unwrap();
                if let Some(view) = view.iter().find(|t| t.id == task) {
                    for row in &view.targets {
                        let sample = (row.state.clone(), row.progress, row.message.clone());
                        let target_samples = samples.entry(row.screen_id.clone()).or_default();
                        if target_samples.last() != Some(&sample) {
                            target_samples.push(sample);
                        }
                    }
                }
                if value.state.is_terminal() || value.state == TaskState::FinalizingFailed {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
        .await?;
        let result = task_data::read_results(state.local_store.pool(), &project, &task).await?;
        if state.task_repository.get(&task).await?.state != TaskState::Succeeded {
            for row in result.targets.values() {
                eprintln!("{size}寸：{}", row.message);
            }
            eprintln!("安装结果保留在 {}", directory.display());
            return Err("安装验收未通过，保留待处理记录".into());
        }
        for (id, address) in &addresses {
            if originals[id] != configuration(&device, address).await? {
                return Err(format!("{address} 安装前后稳定配置不一致").into());
            }
            assert_eq!(
                result.targets[id]
                    .observation
                    .as_ref()
                    .unwrap()
                    .app_version_code,
                Some(package.app_version_code)
            );
            assert_eq!(
                result.targets[id].observation.as_ref().unwrap().app_running,
                Some(true)
            );
            let target_samples = samples.get(id).ok_or("未采集到目标进度")?;
            assert!(
                target_samples
                    .iter()
                    .any(|(status, percent, message)| status == "running"
                        && *percent > 20
                        && *percent < 40
                        && message.contains("MB")),
                "应采集到真实传包过程，而不只是阶段起止值：{target_samples:?}"
            );
            assert!(
                target_samples
                    .iter()
                    .any(|(status, percent, message)| status == "running"
                        && *percent == 40
                        && message.contains("安装小新"))
            );
            assert!(
                target_samples.windows(2).all(|w| w[1].1 >= w[0].1),
                "逐屏进度不能倒退：{target_samples:?}"
            );
        }
        assert!(!state.task_repository.results_protected(&task).await?);
        let logs = inxaiot_desk_buddy_lib::interface::commands::task_activity::query_task_logs(
            &state,
            &task,
            &[],
            None,
            0,
            500,
            false,
        )
        .await
        .map_err(|_| "读取安装日志失败")?;
        for keyword in [
            "开始安装",
            "正在传输安装包",
            "正在安装小新",
            "正在启动小新",
            "安装后的版本",
            "操作结果",
        ] {
            assert!(
                logs.items.iter().any(|e| e.message.contains(keyword)),
                "安装日志缺少阶段：{keyword}"
            );
        }
        std::fs::write(
            directory.join(format!("screen-install-progress-{size}.json")),
            serde_json::to_vec_pretty(
                &serde_json::json!({"passed":true,"version":package.app_version,"versionCode":package.app_version_code,"targetCount":addresses.len(),"samples":samples,"logs":logs.items.iter().map(|entry|&entry.message).collect::<Vec<_>>()}),
            )?,
        )?;
        eprintln!(
            "{size}寸：{} 台同批安装 {}-{}、启动检查、原有配置保持通过",
            addresses.len(),
            package.app_version,
            package.app_version_code
        );
    }
    support::close(state).await;
    Ok(())
}
