use super::*;
use crate::application::ports::task_event::{TaskEventInput, TaskEventSink};
use crate::application::ports::task_log::{TaskLogQuery, TaskLogStore};
use crate::formal::local_store::LocalStore;
use crate::infrastructure::local_sqlite::task_repository::{CreateTask, TaskRepository};
use crate::infrastructure::logging::redactor::SensitiveValueRedactor;
use crate::infrastructure::logging::task_event_pipeline::TaskEventPipeline;
use crate::runtime::event_bus::TaskEventBus;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use tokio::sync::{Barrier, Notify};

const PROJECT: &str = "batch-regression-project";
const TASK: &str = "batch-regression-task";

struct Fixture {
    _directory: tempfile::TempDir,
    store: LocalStore,
    repository: TaskRepository,
    pipeline: TaskEventPipeline,
    targets: Vec<ScreenAsset>,
    log: std::path::PathBuf,
}

impl Fixture {
    async fn new(count: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = LocalStore::open(directory.path().join("local.db")).await.unwrap();
        sqlx::query("INSERT INTO local_project(id,name,platform_url,db_host,db_port,db_user,business_db,workbench_db,db_password_secret_ref,created_at,updated_at) VALUES(?,'隔离批次回归','','',3306,'','','','','1','1')")
            .bind(PROJECT).execute(store.pool()).await.unwrap();
        let targets = (0..count).map(|index| ScreenAsset {
            id: index.to_string(), source: "local".into(),
            fields: ScreenFields { name: format!("合成屏{index}"), ..Default::default() },
            ..Default::default()
        }).collect::<Vec<_>>();
        let repository = TaskRepository::new(store.pool().clone());
        let log = directory.path().join("events.jsonl");
        repository.create(CreateTask {
            id: TASK.into(), local_project_id: PROJECT.into(), remote_operation_record_id: None,
            domain_type: "smart_screen".into(), operation_type: "install".into(), name: "隔离批次回归".into(),
            priority: 0, batch_size: count as u32, concurrency: count.min(3) as u32,
            payload_ref: None, log_path: log.to_string_lossy().into_owned(),
            targets: targets.iter().map(|screen| ("smart_screen".into(), screen.id.clone())).collect(),
        }).await.unwrap();
        for (from, to) in [(TaskState::Draft, TaskState::Checking), (TaskState::Checking, TaskState::Ready),
            (TaskState::Ready, TaskState::Queued), (TaskState::Queued, TaskState::Running)] {
            repository.transition(TASK, from, to, None, None).await.unwrap();
        }
        repository.protect_results(TASK, "测试结果待核实").await.unwrap();
        let plan = ScreenPlan {
            project_id: PROJECT.into(), input: ScreenOperationInput {
                action: "install".into(), target_ids: targets.iter().map(|screen| screen.id.clone()).collect(),
                application_id: None, apk: None, app_version: "2.1.3".into(), abi: String::new(), reinstall: false,
                concurrency: count.min(3) as u32, expected_targets: BTreeMap::new(), retry_of_operation_id: None,
            }, targets: targets.clone(), business_project_id: None, data_source_id: None,
            operator: "synthetic".into(), instance_id: "synthetic".into(), created_at: now(), detail: Value::Null,
        };
        task_data::save_plan(store.pool(), TASK, &plan).await.unwrap();
        task_data::save_results(store.pool(), PROJECT, TASK, &ScreenResults {
            targets: targets.iter().map(|screen| (screen.id.clone(), result(screen, ResultState::Pending, None))).collect(),
            finished: false,
        }).await.unwrap();
        let pipeline = TaskEventPipeline::new(repository.clone(), TaskEventBus::new(512).unwrap(), SensitiveValueRedactor::default());
        Self { _directory: directory, store, repository, pipeline, targets, log }
    }

    async fn events(&self) -> Vec<crate::domain::common::task::TaskEvent> {
        self.pipeline.log_store().read_page(&self.log, &TaskLogQuery {
            levels: vec![], keyword: None, offset: 0, limit: 1000, newest_first: false,
        }).await.unwrap().items
    }
}

fn event(screen: &ScreenAsset) -> TaskEventInput {
    TaskEventInput { resource_type: Some("smart_screen".into()), resource_key: Some(screen.id.clone()),
        stage: "result".into(), status: "running".into(), progress_current: Some(80), progress_total: Some(100),
        level: TaskEventLevel::Info, message_code: "SYNTHETIC_RESULT".into(), message_params: BTreeMap::new(),
        message: Some("合成设备阶段结束".into()) }
}

fn result(screen: &ScreenAsset, device: ResultState, phase: Option<&str>) -> ScreenTargetResult {
    ScreenTargetResult { format_version: 1, screen_id: screen.id.clone(), device,
        business: ResultState::NotRequired, shared: ResultState::NotRequired,
        evidence: phase.map_or_else(|| json!({}), |phase| json!({"phase":phase})), ..Default::default() }
}

#[tokio::test]
async fn maintenance_batch_polls_lock_owner_while_another_target_finalizes() {
    // 使用生产调度函数和真实任务事件管线；锁内等待模拟数据库/文件 I/O。
    let fixture = Fixture::new(2).await;
    let held = Arc::new(Notify::new());
    let ready = Arc::new(Notify::new());
    let observer = {
        let ready = ready.clone();
        tokio::spawn(async move { tokio::time::sleep(Duration::from_millis(20)).await; ready.notify_one(); })
    };
    let batch = tokio::time::timeout(Duration::from_secs(3), drain_maintenance_targets(&fixture.targets, 2, |screen| {
        let fixture = &fixture;
        let held = &held;
        let ready = &ready;
        async move {
        if screen.id == "0" {
            held.notified().await;
        } else {
            let guard = fixture.pipeline.hold_task_lock_for_test(TASK).await;
            held.notify_one();
            ready.notified().await;
            drop(guard);
        }
        task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, ResultState::Succeeded, Some("verified"))).await?;
        fixture.pipeline.emit(TASK, event(&screen)).await?;
        Ok(())
    }})).await.expect("结果收尾不能暂停持有事件锁的另一单屏 future");
    observer.await.unwrap();
    assert!(batch.failures.is_empty() && batch.not_started.is_empty());
    let events = fixture.events().await;
    assert_eq!(events.len(), 2);
    assert!(events[0].sequence < events[1].sequence);
}

#[tokio::test]
async fn maintenance_batch_twenty_and_eighty_targets_keep_three_in_flight_with_real_logs() {
    for count in [20, 80] {
        let fixture = Fixture::new(count).await;
        let active = AtomicUsize::new(0);
        let maximum = AtomicUsize::new(0);
        let completed = AtomicUsize::new(0);
        let batch = tokio::time::timeout(Duration::from_secs(20), drain_maintenance_targets(&fixture.targets, 3, |screen| {
            let fixture = &fixture;
            let active = &active;
            let maximum = &maximum;
            let completed = &completed;
            async move {
            let running = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(running, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(2)).await;
            task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, ResultState::Succeeded, Some("verified"))).await?;
            fixture.pipeline.emit(TASK, event(&screen)).await?;
            completed.fetch_add(1, Ordering::SeqCst);
            active.fetch_sub(1, Ordering::SeqCst);
            Ok(())
        }})).await.expect("大批次日志收尾应持续推进");
        assert!(batch.failures.is_empty() && batch.not_started.is_empty());
        assert_eq!(maximum.load(Ordering::SeqCst), 3);
        assert_eq!(completed.load(Ordering::SeqCst), count);
        let events = fixture.events().await;
        assert_eq!(events.len(), count);
        assert!(events.windows(2).all(|pair| pair[0].sequence < pair[1].sequence));
        assert_eq!(events.iter().map(|event| event.resource_key.as_deref().unwrap()).collect::<BTreeSet<_>>().len(), count);
    }
}

#[tokio::test]
async fn maintenance_batch_storage_failure_stops_dispatch_but_drains_installed_target() {
    let fixture = Fixture::new(5).await;
    let first = &fixture.targets[0];
    task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(first, ResultState::Unknown, Some("install_requested"))).await.unwrap();
    sqlx::query("CREATE TRIGGER fail_first_success BEFORE UPDATE ON local_screen_task_data WHEN json_extract(NEW.result_json,'$.targets.\"0\".device')='succeeded' BEGIN SELECT RAISE(ABORT,'INJECTED_RESULT_SAVE'); END")
        .execute(fixture.store.pool()).await.unwrap();
    let second_started = Notify::new();
    let first_save_failed = Notify::new();
    let second_finished = AtomicBool::new(false);
    let commands = AtomicUsize::new(0);
    let batch = tokio::time::timeout(Duration::from_secs(3), drain_maintenance_targets(&fixture.targets, 2, |screen| {
        let fixture = &fixture;
        let commands = &commands;
        let second_started = &second_started;
        let first_save_failed = &first_save_failed;
        let second_finished = &second_finished;
        async move {
        commands.fetch_add(1, Ordering::SeqCst);
        if screen.id == "0" {
            second_started.notified().await;
        } else {
            assert_eq!(screen.id, "1", "存储失败后不能补位执行新屏");
            second_started.notify_one();
            // 已发出的安装继续等待完成；另一个屏的保存失败不得 drop 此 future。
            first_save_failed.notified().await;
        }
        if let Err(error) = task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, ResultState::Succeeded, Some("verified"))).await {
            first_save_failed.notify_one();
            return Err(error);
        }
        fixture.pipeline.emit(TASK, event(&screen)).await?;
        second_finished.store(true, Ordering::SeqCst);
        Ok(())
    }})).await.unwrap();
    assert_eq!(commands.load(Ordering::SeqCst), 2);
    assert!(second_finished.load(Ordering::SeqCst));
    assert_eq!(batch.failures.len(), 1);
    assert_eq!(batch.failures[0].0, "0");
    assert!(matches!(batch.failures[0].1, AppError::Database { .. }));
    assert_eq!(batch.not_started.len(), 3);
    defer_maintenance_batch(&fixture.repository, TASK, batch.not_started.len()).await.unwrap();
    assert_eq!(fixture.repository.get(TASK).await.unwrap().state, TaskState::FinalizingFailed);
    assert!(fixture.repository.results_protected(TASK).await.unwrap());
    let mut saved = task_data::read_results(fixture.store.pool(), PROJECT, TASK).await.unwrap();
    assert!(!saved.finished);
    assert_eq!(saved.targets["0"].device, ResultState::Unknown);
    assert_eq!(saved.targets["0"].evidence["phase"], "install_requested");
    assert_eq!(saved.targets["1"].device, ResultState::Succeeded);
    for id in ["2", "3", "4"] { assert_eq!(saved.targets[id].device, ResultState::Pending); }
    // 重开原任务使用同一个生产恢复判定：未发出动作的屏取消，已发安装保留核实。
    for result in saved.targets.values_mut() { cancel_unsubmitted_result(result); }
    assert_eq!(saved.targets["0"].device, ResultState::Unknown);
    assert_eq!(saved.targets["1"].device, ResultState::Succeeded);
    for id in ["2", "3", "4"] { assert_eq!(saved.targets[id].device, ResultState::Cancelled); }
}

#[tokio::test]
async fn maintenance_batch_user_cancel_keeps_submitted_work_and_stops_new_device_commands() {
    let fixture = Fixture::new(12).await;
    let cancellation = CancellationToken::new();
    let started = AtomicUsize::new(0);
    let finished = AtomicUsize::new(0);
    let barrier = Barrier::new(3);
    let batch = tokio::time::timeout(Duration::from_secs(5), drain_maintenance_targets(&fixture.targets, 3, |screen| {
        let fixture = &fixture;
        let cancellation = &cancellation;
        let started = &started;
        let finished = &finished;
        let barrier = &barrier;
        async move {
        if cancellation.is_cancelled() {
            task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, ResultState::Cancelled, None)).await?;
            return Ok(());
        }
        started.fetch_add(1, Ordering::SeqCst);
        barrier.wait().await;
        if screen.id == "0" { cancellation.cancel(); }
        // 对应已发出的 pm install：不让取消另一个目标直接终止本机核实。
        tokio::time::sleep(Duration::from_millis(20)).await;
        task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, ResultState::Succeeded, Some("verified"))).await?;
        fixture.pipeline.emit(TASK, event(&screen)).await?;
        finished.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }})).await.unwrap();
    assert!(batch.failures.is_empty());
    assert_eq!(started.load(Ordering::SeqCst), 3);
    assert_eq!(finished.load(Ordering::SeqCst), 3);
    let saved = task_data::read_results(fixture.store.pool(), PROJECT, TASK).await.unwrap();
    assert_eq!(saved.targets.values().filter(|target| target.device == ResultState::Succeeded).count(), 3);
    assert_eq!(saved.targets.values().filter(|target| target.device == ResultState::Cancelled).count(), 9);
}

#[tokio::test]
async fn maintenance_batch_normal_device_failure_does_not_stop_other_screens() {
    let fixture = Fixture::new(8).await;
    let batch = drain_maintenance_targets(&fixture.targets, 3, |screen| {
        let fixture = &fixture;
        async move {
        let device = if screen.id == "0" { ResultState::Failed } else { ResultState::Succeeded };
        task_data::save_target(fixture.store.pool(), PROJECT, TASK, &result(&screen, device, Some("verified"))).await?;
        fixture.pipeline.emit(TASK, event(&screen)).await?;
        Ok(())
    }}).await;
    assert!(batch.failures.is_empty() && batch.not_started.is_empty());
    let saved = task_data::read_results(fixture.store.pool(), PROJECT, TASK).await.unwrap();
    assert_eq!(saved.targets["0"].device, ResultState::Failed);
    assert_eq!(saved.targets.values().filter(|target| target.device == ResultState::Succeeded).count(), 7);
}

#[test]
fn recovered_install_confirmation_replaces_stale_error_without_claiming_shared_success() {
    for shared in [ResultState::Pending, ResultState::Failed] {
        let mut result = ScreenTargetResult { device: ResultState::Succeeded, business: ResultState::Unknown,
            shared, message: "设备安装已完成，平台记录待处理：数据库操作失败".into(),
            evidence: json!({"phase":"verified","beforeInstallStamp":"original"}), ..Default::default() };
        let receipt = json!({"after":{"appVersion":"2.1.3"},"verified":true});
        confirm_recovered_install_result(&mut result, "2.1.3", receipt.clone());
        assert_eq!(result.business, ResultState::Succeeded);
        assert_eq!(result.device, ResultState::Succeeded);
        assert_eq!(result.shared, shared);
        assert!(!result.message.contains("失败") && !result.message.contains("待处理"));
        assert!(result.message.contains("2.1.3") && result.message.contains("未重复安装"));
        assert!(!result.message.contains("共享记录已完成"));
        assert_eq!(result.evidence["businessReceipt"], receipt);
        assert_eq!(result.evidence["phase"], "verified");
        assert_eq!(result.evidence["beforeInstallStamp"], "original");
    }
}
