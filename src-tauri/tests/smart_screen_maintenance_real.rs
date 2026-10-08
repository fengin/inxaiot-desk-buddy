#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    domain::{
        common::task::TaskState,
        smart_screen::{model::ScreenFields, operation::ScreenOperationInput},
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{diagnostics, task_data, tasks},
        stage75_adapter::Stage75Adapter,
    },
};
use std::time::Duration;
fn input(action: &str, ids: Vec<String>) -> ScreenOperationInput {
    ScreenOperationInput {
        action: action.into(),
        target_ids: ids,
        application_id: None,
        apk: None,
        app_version: String::new(),
        abi: String::new(),
        reinstall: false,
        concurrency: 2,
        retry_of_operation_id: None,
        expected_targets: Default::default(),
    }
}
async fn execute(
    state: &FormalAppState,
    project: &str,
    request: ScreenOperationInput,
) -> Result<String, Box<dyn std::error::Error>> {
    let preview = tasks::preflight(state, project, request.clone()).await?;
    if let Some(blocked) = preview.items.iter().find(|i| i.state != "ready") {
        return Err(format!("{}：{}", blocked.name, blocked.reason).into());
    }
    let task = tasks::submit(state, project, &preview.id, request.clone()).await?;
    tokio::time::timeout(Duration::from_secs(300), async {
        loop {
            let record = state.task_repository.get(&task).await.unwrap();
            if record.state.is_terminal() || record.state == TaskState::FinalizingFailed {
                break;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    })
    .await?;
    let results = task_data::read_results(state.local_store.pool(), project, &task).await?;
    for result in results.targets.values() {
        eprintln!("{}：{}", request.action, result.message);
    }
    if state.task_repository.get(&task).await?.state != TaskState::Succeeded {
        return Err(format!("{} 未通过，task={task}", request.action).into());
    }
    Ok(task)
}
#[tokio::test]
#[ignore = "测试说明全部实机校时、应用重启、系统重启，10寸连续两次端口设置和重启；保留应用数据"]
async fn time_restart_reboot_persistent_adb_and_diagnostics()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = support::evidence_dir("device-maintenance")?;
    let state = support::state_at(&directory, true).await;
    let project = Stage75Adapter::new(&state)
        .create_project(support::local_input())
        .await?
        .project
        .id;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let mut ids = Vec::new();
    let mut four_ids = Vec::new();
    let mut ten_ids = Vec::new();
    for screen in support::live_screens()? {
        ids.push(
            repo.save_local(
                &project,
                &ScreenFields {
                    name: screen.name,
                    ip: screen.ip,
                    size: screen.size.clone(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await?,
        );
        if screen.size == "4" {
            four_ids.push(ids.last().unwrap().clone());
        }
        if screen.size == "10" {
            ten_ids.push(ids.last().unwrap().clone());
        }
    }
    let result: Result<(), Box<dyn std::error::Error>> = async {
        let unsupported =
            tasks::preflight(&state, &project, input("adb", four_ids.clone())).await?;
        assert_eq!(unsupported.items[0].state, "blocked");
        assert!(unsupported.items[0].reason.contains("10寸"));
        execute(&state, &project, input("time", ids.clone())).await?;
        execute(&state, &project, input("restart", ids.clone())).await?;
        execute(&state, &project, input("reboot", ids.clone())).await?;
        for round in 1..=2 {
            eprintln!("10寸端口持久化：第{round}次连续重启验证");
            execute(&state, &project, input("adb", ten_ids.clone())).await?;
        }
        let diagnostic = execute(&state, &project, input("diagnostics", ids.clone())).await?;
        let content = diagnostics::read(&state, &project, &diagnostic).await?;
        let document: serde_json::Value = serde_json::from_str(&content)?;
        assert_eq!(document["screens"].as_array().unwrap().len(), ids.len());
        assert!(!content.contains("FlutterSharedPreferences"));
        let export = diagnostics::export(&state, &project, &diagnostic, &directory).await?;
        assert_eq!(std::fs::read_to_string(export)?, content);
        assert!(
            diagnostics::read(&state, "another-project", &diagnostic)
                .await
                .is_err()
        );
        eprintln!(
            "两种尺寸校时、应用重启、系统重启，10寸连续两次端口重启，诊断保存/导出/项目隔离均通过"
        );
        Ok(())
    }
    .await;
    support::close(state).await;
    std::fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"passed":result.is_ok(),"screenCount":ids.len(),"error":result.as_ref().err().map(|error|error.to_string())}),
        )?,
    )?;
    if result.is_err() {
        eprintln!("维护验收记录保留于 {}", directory.display());
    }
    result
}

#[tokio::test]
#[ignore = "受控接续本轮10寸ADB维护：先只核实原重启，成功后最多启动第二轮，再核对三屏诊断"]
async fn resume_persistent_adb_and_diagnostics_from_preserved_case()
-> Result<(), Box<dyn std::error::Error>> {
    use inxaiot_desk_buddy_lib::domain::smart_screen::model::ResultState;
    use sqlx::Row;
    const PROJECT: &str = "01a11a52-9ba0-70f3-a8c1-6e1a2600cff5";
    const FIRST_TASK: &str = "01a11a55-f8a1-72f1-bb93-8e329800d4f4";
    const TEN: &str = "01a11a52-9bb1-7f71-9902-f02711468760";
    const EXPECTED: &[(&str, &str, &str)] = &[
        ("01a11a52-9baf-7fc3-af77-e5b1500a015b", "192.168.3.63", "4"),
        ("01a11a52-9bb1-7f71-9902-f012befce516", "192.168.3.103", "4"),
        (TEN, "192.168.3.70", "10"),
    ];
    let requested = std::path::PathBuf::from(
        std::env::var_os("INX_SCREEN_MAINTENANCE_CASE")
            .ok_or("必须明确设置 INX_SCREEN_MAINTENANCE_CASE，不能自动选择待处理任务")?,
    )
    .canonicalize()?;
    let expected =
        std::path::Path::new(r"C:\Users\fengin\AppData\Local\Temp\.tmpD14jp6").canonicalize()?;
    if requested != expected || !requested.join("local.db").is_file() {
        return Err("此接续用例只允许原维护验收目录.tmpD14jp6，未访问其他任务".into());
    }
    let evidence = support::evidence_dir("maintenance-resume")?;
    let state = support::state_at(&requested, true).await;
    let mut report = serde_json::json!({"passed":false,"firstTask":FIRST_TASK,"screenId":TEN,"firstRebootReplayed":false,
        "wirelessIntervention":"第一轮后用户现场手动重连WiFi，仍为原IP；ADB端口没有现场修改。生产使用有线，本用例不改变网络或自动唤醒需求。"});
    let outcome: Result<(), Box<dyn std::error::Error>> = async {
        let screens =
            sqlx::query("SELECT local_project_id,id,fields_json FROM local_screen WHERE removed=0")
                .fetch_all(state.local_store.pool())
                .await?;
        if screens.len() != EXPECTED.len() {
            return Err("原本机目录不是本轮三屏，未接续".into());
        }
        for row in screens {
            let id: String = row.try_get("id")?;
            let fields: ScreenFields =
                serde_json::from_str(&row.try_get::<String, _>("fields_json")?)?;
            if row.try_get::<String, _>("local_project_id")? != PROJECT
                || !EXPECTED.iter().any(|(known_id, ip, size)| {
                    *known_id == id && *ip == fields.ip && *size == fields.size
                })
            {
                return Err("原屏ID、项目、IP或尺寸不匹配，未接续".into());
            }
        }
        let first = state.task_repository.get(FIRST_TASK).await?;
        let (plan, _) = task_data::read_plan(state.local_store.pool(), PROJECT, FIRST_TASK).await?;
        let original =
            task_data::read_results(state.local_store.pool(), PROJECT, FIRST_TASK).await?;
        let original_target = original.targets.get(TEN).ok_or("原任务不含指定10寸屏")?;
        if first.local_project_id != PROJECT
            || first.operation_type != "adb"
            || plan.input.action != "adb"
            || plan.targets.len() != 1
            || plan.targets[0].id != TEN
            || plan.targets[0].fields.ip != "192.168.3.70"
            || original_target.evidence["before"]["observedMac"] != "26:da:35:7d:85:b5"
            || original_target.evidence["capability"]["previousPort"] != "5555"
        {
            return Err("原ADB任务的目标、MAC或端口依据不匹配，未接续".into());
        }
        if first.state == TaskState::FinalizingFailed
            || state.task_repository.results_protected(FIRST_TASK).await?
        {
            // 正式核实路径只读原设备结果，不再次setprop或reboot。
            tasks::verify(&state, PROJECT, FIRST_TASK).await?;
        }
        let confirmed =
            task_data::read_results(state.local_store.pool(), PROJECT, FIRST_TASK).await?;
        let target = &confirmed.targets[TEN];
        if state.task_repository.get(FIRST_TASK).await?.state != TaskState::Succeeded
            || target.device != ResultState::Succeeded
            || state.task_repository.results_protected(FIRST_TASK).await?
        {
            return Err("第一轮原重启尚未核实成功，停止；未发送第二轮重启".into());
        }
        let first_boot = target.evidence["afterBootId"]
            .as_str()
            .ok_or("第一轮缺少核实后的bootId")?
            .to_string();
        if first_boot.is_empty() || target.evidence["capability"]["bootId"] == first_boot {
            return Err("第一轮没有确认启动编号变化，停止；未重复重启".into());
        }
        report["firstAfterBootId"] = serde_json::json!(first_boot);
        report["firstVerified"] = serde_json::json!(true);
        std::fs::write(
            evidence.join("progress.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;

        // 重跑接续脚本时，若第二轮已经建立任务，只核实它，绝不再发第三次重启。
        let previous_second = state
            .task_repository
            .list_recent(PROJECT, 500)
            .await?
            .into_iter()
            .filter(|task| task.operation_type == "adb" && task.id != FIRST_TASK)
            .collect::<Vec<_>>();
        if previous_second.len() > 1 {
            return Err("目录中有多条后续ADB任务，请人工核对；不自动追加重启".into());
        }
        let second = if let Some(task) = previous_second.first() {
            let (plan, _) =
                task_data::read_plan(state.local_store.pool(), PROJECT, &task.id).await?;
            if plan.targets.len() != 1 || plan.targets[0].id != TEN {
                return Err("已有第二轮任务目标不匹配，停止".into());
            }
            if task.state == TaskState::FinalizingFailed
                || state.task_repository.results_protected(&task.id).await?
            {
                tasks::verify(&state, PROJECT, &task.id).await?;
            }
            task.id.clone()
        } else {
            let request = input("adb", vec![TEN.into()]);
            let preview = tasks::preflight(&state, PROJECT, request.clone()).await?;
            if preview.items.len() != 1 || preview.items[0].state != "ready" {
                return Err("第二轮ADB维护检查未通过，未重启".into());
            }
            let task = tasks::submit(&state, PROJECT, &preview.id, request).await?;
            report["secondTask"] = serde_json::json!(task);
            std::fs::write(
                evidence.join("progress.json"),
                serde_json::to_vec_pretty(&report)?,
            )?;
            // 仅等待既有任务终态；设备内部仍使用原180秒恢复上限，不加长或循环重启。
            tokio::time::timeout(Duration::from_secs(300), async {
                loop {
                    let status = state.task_repository.get(&task).await.unwrap().state;
                    if status.is_terminal() || status == TaskState::FinalizingFailed {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            })
            .await?;
            task
        };
        report["secondTask"] = serde_json::json!(second);
        let second_result =
            task_data::read_results(state.local_store.pool(), PROJECT, &second).await?;
        let target = second_result.targets.get(TEN).ok_or("第二轮目标结果缺失")?;
        if state.task_repository.get(&second).await?.state != TaskState::Succeeded
            || target.device != ResultState::Succeeded
        {
            return Err("第二轮ADB维护结果待核实；可能需要现场恢复WiFi，停止且不再发送重启".into());
        }
        let second_boot = target.evidence["afterBootId"]
            .as_str()
            .ok_or("第二轮缺少bootId")?;
        if second_boot.is_empty() || second_boot == first_boot {
            return Err("第二轮启动编号未变化，未确认第二次系统启动".into());
        }
        report["secondAfterBootId"] = serde_json::json!(second_boot);
        let ids = EXPECTED
            .iter()
            .map(|(id, _, _)| id.to_string())
            .collect::<Vec<_>>();
        let diagnostic = execute(&state, PROJECT, input("diagnostics", ids)).await?;
        let content = diagnostics::read(&state, PROJECT, &diagnostic).await?;
        let document: serde_json::Value = serde_json::from_str(&content)?;
        if document["screens"].as_array().map(Vec::len) != Some(3)
            || content.contains("FlutterSharedPreferences")
        {
            return Err("三屏诊断内容不完整或包含私有配置文件".into());
        }
        let exported = diagnostics::export(&state, PROJECT, &diagnostic, &evidence).await?;
        if std::fs::read_to_string(&exported)? != content
            || diagnostics::read(&state, "another-project", &diagnostic)
                .await
                .is_ok()
        {
            return Err("诊断导出或项目隔离未通过".into());
        }
        report["diagnosticTask"] = serde_json::json!(diagnostic);
        report["threeScreenDiagnosticsAndExport"] = serde_json::json!(true);
        Ok(())
    }
    .await;
    support::close(state).await;
    report["passed"] = serde_json::json!(outcome.is_ok());
    report["error"] = serde_json::json!(outcome.as_ref().err().map(|error| error.to_string()));
    std::fs::write(
        evidence.join("result.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!("维护接续证据：{}", evidence.display());
    outcome
}
