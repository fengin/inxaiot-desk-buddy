//! 远程辅助操作的仓储、取消、空闲回收与凭据保留边界。
//! 设备输入使用内存模拟端口，不连接任何设备或平台，不进行真实登录。
#[path = "common/screen_test_support.rs"]
mod support;

use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::{AppError, AppResult},
    domain::{
        common::task::TaskState,
        smart_screen::{
            model::ScreenFields,
            operation::{ScreenOperationInput, ScreenPlan},
        },
    },
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::{screen_repository::ScreenRepository, task_repository::CreateTask},
        smart_screen::{
            remote_screen::{
                self, ControlAction, ControlFuture, ControlInput, ControlKey, ControlPort,
                ControlSendContext, DeviceFacts, RawCapture, RemoteScreenFrame,
                RemoteScreenSessions, ScreenCapturePort,
            },
            task_data, tasks,
        },
        stage75_adapter::Stage75Adapter,
        task_runtime::{reconcile_untracked_active_tasks, retry_pending_local_finalization},
    },
    interface::commands::task_activity::request_task_cancel,
};
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const OWNER: &str = "remote-control-fixture";
const IP: &str = "192.0.2.42";
const SERIAL: &str = "fixture-serial";
const PRIVATE_TEXT: &str = "RemoteInputSecret_%s'\"$&42";

fn png() -> Vec<u8> {
    fn chunk(output: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        output.extend_from_slice(&(data.len() as u32).to_be_bytes());
        output.extend_from_slice(kind);
        output.extend_from_slice(data);
        let mut crc = flate2::Crc::new();
        crc.update(kind);
        crc.update(data);
        output.extend_from_slice(&crc.sum().to_be_bytes());
    }
    let mut output = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(
        &mut output,
        b"IHDR",
        &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0],
    );
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&[0, 255, 128, 0, 255]).unwrap();
    chunk(&mut output, b"IDAT", &encoder.finish().unwrap());
    chunk(&mut output, b"IEND", &[]);
    output
}

struct Capture;
impl ScreenCapturePort for Capture {
    async fn capture(&self, ip: &str, _: CancellationToken) -> AppResult<Vec<u8>> {
        assert_eq!(ip, IP);
        Ok(png())
    }
    async fn capture_verified(&self, ip: &str, cancel: CancellationToken) -> AppResult<RawCapture> {
        Ok(RawCapture {
            png: self.capture(ip, cancel).await?,
            serial: Some(SERIAL.into()),
        })
    }
}

struct BlockedCapture {
    started: Notify,
    release: Notify,
}
impl BlockedCapture {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Notify::new(),
            release: Notify::new(),
        })
    }
}
impl ScreenCapturePort for BlockedCapture {
    async fn capture(&self, ip: &str, cancel: CancellationToken) -> AppResult<Vec<u8>> {
        assert_eq!(ip, IP);
        self.started.notify_one();
        tokio::select! {
            _ = cancel.cancelled() => Err(AppError::Cancelled),
            _ = self.release.notified() => Ok(png()),
        }
    }
    async fn capture_verified(&self, ip: &str, cancel: CancellationToken) -> AppResult<RawCapture> {
        Ok(RawCapture {
            png: self.capture(ip, cancel).await?,
            serial: Some(SERIAL.into()),
        })
    }
}

struct Input {
    sends: AtomicUsize,
    private_text_received: AtomicBool,
    block: bool,
    fail: bool,
    started: Notify,
    release: Notify,
    serial: Mutex<String>,
    foreground: Mutex<String>,
    orientation: AtomicUsize,
    awake: Mutex<Option<bool>>,
    wake_effect: AtomicBool,
    fail_probe_after_send: AtomicBool,
}
impl Input {
    fn new(block: bool, fail: bool) -> Arc<Self> {
        Arc::new(Self {
            sends: AtomicUsize::new(0),
            private_text_received: AtomicBool::new(false),
            block,
            fail,
            started: Notify::new(),
            release: Notify::new(),
            serial: Mutex::new(SERIAL.into()),
            foreground: Mutex::new("chat.xiaoxin.app".into()),
            orientation: AtomicUsize::new(0),
            awake: Mutex::new(Some(true)),
            wake_effect: AtomicBool::new(true),
            fail_probe_after_send: AtomicBool::new(false),
        })
    }
}
impl ControlPort for Input {
    fn probe<'a>(&'a self, ip: &'a str, _: CancellationToken) -> ControlFuture<'a, DeviceFacts> {
        Box::pin(async move {
            assert_eq!(ip, IP);
            if self.fail_probe_after_send.load(Ordering::SeqCst)
                && self.sends.load(Ordering::SeqCst) > 0
            {
                return Err(AppError::timeout("只读确认电源状态"));
            }
            Ok(DeviceFacts {
                android_sdk: 29,
                awake: *self.awake.lock().unwrap(),
                serial: Some(self.serial.lock().unwrap().clone()),
                mac: None,
                foreground_package: self.foreground.lock().unwrap().clone(),
                width: 1,
                height: 1,
                orientation: self.orientation.load(Ordering::SeqCst) as u32,
            })
        })
    }
    fn send<'a>(
        &'a self,
        ip: &'a str,
        action: &'a ControlAction,
        context: ControlSendContext,
        _: CancellationToken,
    ) -> ControlFuture<'a, ()> {
        Box::pin(async move {
            assert_eq!(ip, IP);
            assert_eq!(context.android_sdk, 29);
            assert!(context.deadline > tokio::time::Instant::now());
            self.sends.fetch_add(1, Ordering::SeqCst);
            if let ControlAction::Text { text } = action {
                self.private_text_received
                    .store(text == PRIVATE_TEXT, Ordering::SeqCst);
            }
            self.started.notify_one();
            // 模拟在途指令必须先收敛：取消不能让调用方在该Future返回前释放占用。
            if self.block {
                self.release.notified().await;
            }
            if self.fail {
                return Err(AppError::timeout("等待辅助输入结果"));
            }
            if self.wake_effect.load(Ordering::SeqCst)
                && matches!(
                    action,
                    ControlAction::Key {
                        key: ControlKey::WakeUp
                    }
                )
            {
                *self.awake.lock().unwrap() = Some(true);
                *self.foreground.lock().unwrap() = "chat.xiaoxin.app".into();
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn sleeping_screen_allows_only_one_wakeup_then_requires_a_new_full_control_frame() {
    let fixture = Fixture::new(Duration::from_secs(30)).await;
    let input = Input::new(false, false);
    *input.awake.lock().unwrap() = Some(false);
    *input.foreground.lock().unwrap() = String::new();
    let control = remote_screen::begin_control_with(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        input.clone(),
    )
    .await
    .unwrap();
    assert_eq!(control.control_mode, "wake_only");
    for action in [
        ControlAction::Tap { x: 0, y: 0 },
        ControlAction::Text {
            text: PRIVATE_TEXT.into(),
        },
        ControlAction::Key {
            key: ControlKey::Back,
        },
    ] {
        let frame_id = fixture.frame().await;
        assert!(
            remote_screen::act(
                &fixture.state,
                &fixture.sessions,
                OWNER,
                &fixture.project,
                &fixture.session,
                ControlInput { frame_id, action }
            )
            .await
            .is_err()
        );
        assert_eq!(input.sends.load(Ordering::SeqCst), 0);
    }
    let before = fixture.frame().await;
    let wake = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        ControlInput {
            frame_id: before.clone(),
            action: ControlAction::Key {
                key: ControlKey::WakeUp,
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(wake.status, "sent");
    assert_eq!(wake.control_mode, "full");
    assert!(wake.control_active);
    assert_eq!(input.sends.load(Ordering::SeqCst), 1);
    rejected_without_sending(&fixture, &input, before, 1).await;
    let frame = fixture.frame().await;
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(frame)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    assert_eq!(input.sends.load(Ordering::SeqCst), 2);
    fixture.end().await;
    fixture.settled(&control.task_id).await;
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn unconfirmed_wake_does_not_repeat_or_leave_an_active_session() {
    let fixture = Fixture::new(Duration::from_secs(30)).await;
    let input = Input::new(false, false);
    *input.awake.lock().unwrap() = Some(false);
    *input.foreground.lock().unwrap() = String::new();
    input.wake_effect.store(false, Ordering::SeqCst);
    input.fail_probe_after_send.store(true, Ordering::SeqCst);
    let task = fixture.enable(&input).await;
    let frame = fixture.frame().await;
    let receipt = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        ControlInput {
            frame_id: frame,
            action: ControlAction::Key {
                key: ControlKey::WakeUp,
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.status, "sent");
    assert_eq!(receipt.control_mode, "wake_only");
    assert!(!receipt.control_active);
    assert!(receipt.message.contains("重新开启"));
    fixture.settled(&task).await;
    assert_eq!(input.sends.load(Ordering::SeqCst), 1);
    assert!(
        !fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn awake_wrong_foreground_and_unknown_power_cannot_start_control() {
    for (awake, foreground) in [
        (None, "chat.xiaoxin.app"),
        (Some(true), "another.app"),
        (Some(true), ""),
    ] {
        let fixture = Fixture::new(Duration::from_secs(30)).await;
        let input = Input::new(false, false);
        *input.awake.lock().unwrap() = awake;
        *input.foreground.lock().unwrap() = foreground.into();
        assert!(
            remote_screen::begin_control_with(
                &fixture.state,
                &fixture.sessions,
                OWNER,
                &fixture.project,
                &fixture.session,
                input.clone()
            )
            .await
            .is_err()
        );
        fixture.end().await;
        assert_eq!(input.sends.load(Ordering::SeqCst), 0);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn a_full_session_that_falls_asleep_rejects_text_but_can_wake_once() {
    let fixture = Fixture::new(Duration::from_secs(30)).await;
    let input = Input::new(false, false);
    let task = fixture.enable(&input).await;
    *input.awake.lock().unwrap() = Some(false);
    *input.foreground.lock().unwrap() = String::new();
    let frame = fixture.frame().await;
    assert!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            text(frame)
        )
        .await
        .is_err()
    );
    assert_eq!(input.sends.load(Ordering::SeqCst), 0);
    let frame = fixture.frame().await;
    let receipt = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        ControlInput {
            frame_id: frame,
            action: ControlAction::Key {
                key: ControlKey::WakeUp,
            },
        },
    )
    .await
    .unwrap();
    assert_eq!(receipt.control_mode, "full");
    assert!(receipt.control_active);
    assert_eq!(input.sends.load(Ordering::SeqCst), 1);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

struct Fixture {
    _root: tempfile::TempDir,
    state: Arc<FormalAppState>,
    sessions: Arc<RemoteScreenSessions>,
    project: String,
    screen: String,
    session: String,
}
impl Fixture {
    async fn new(idle: Duration) -> Self {
        let root = tempfile::tempdir().unwrap();
        let state = support::state_at(root.path(), false).await;
        let project = Stage75Adapter::new(&state)
            .create_project(support::local_input())
            .await
            .unwrap()
            .project
            .id;
        let screen = ScreenRepository::new(state.local_store.pool().clone())
            .save_local(
                &project,
                &ScreenFields {
                    name: "辅助操作隔离目标".into(),
                    ip: IP.into(),
                    size: "10".into(),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap();
        let sessions = Arc::new(RemoteScreenSessions::with_control_idle_timeout(idle));
        let session = remote_screen::open(&state, &sessions, OWNER, &project, &screen)
            .await
            .unwrap()
            .session_id;
        Self {
            _root: root,
            state,
            sessions,
            project,
            screen,
            session,
        }
    }
    async fn enable(&self, input: &Arc<Input>) -> String {
        let io: Arc<dyn ControlPort> = input.clone();
        remote_screen::begin_control_with(
            &self.state,
            &self.sessions,
            OWNER,
            &self.project,
            &self.session,
            io,
        )
        .await
        .unwrap()
        .task_id
    }
    async fn frame(&self) -> String {
        remote_screen::capture_with(
            &self.state,
            &self.sessions,
            OWNER,
            &self.project,
            &self.session,
            &Capture,
        )
        .await
        .unwrap()
        .frame_id
    }
    async fn end(&self) {
        remote_screen::end_control(&self.sessions, OWNER, &self.project, &self.session)
            .await
            .unwrap();
    }
    async fn settled(&self, task: &str) {
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let record = self.state.task_repository.get(task).await.unwrap();
                if record.state.is_terminal() && !self.state.job_supervisor.contains(task).await {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("辅助操作未在限定时间内完成收尾");
    }
    async fn finish(self) {
        self.sessions.close_owner(OWNER);
        support::close(self.state).await;
    }
}

fn text(frame_id: String) -> ControlInput {
    ControlInput {
        frame_id,
        action: ControlAction::Text {
            text: PRIVATE_TEXT.into(),
        },
    }
}
fn tap(frame_id: String) -> ControlInput {
    ControlInput {
        frame_id,
        action: ControlAction::Tap { x: 0, y: 0 },
    }
}

fn spawn_blocked_capture(
    fixture: &Fixture,
    capture: &Arc<BlockedCapture>,
) -> tokio::task::JoinHandle<AppResult<RemoteScreenFrame>> {
    let (state, sessions, project, session, capture) = (
        fixture.state.clone(),
        fixture.sessions.clone(),
        fixture.project.clone(),
        fixture.session.clone(),
        capture.clone(),
    );
    tokio::spawn(async move {
        remote_screen::capture_with(&state, &sessions, OWNER, &project, &session, &*capture).await
    })
}

async fn rejected_without_sending(
    fixture: &Fixture,
    io: &Input,
    frame: String,
    expected_sends: usize,
) {
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(frame),
        ),
    )
    .await
    .expect("应立即拒绝失效或正在操作的画面，不应排队");
    if let Ok(receipt) = outcome {
        assert_ne!(receipt.status, "sent");
    }
    assert_eq!(io.sends.load(Ordering::SeqCst), expected_sends);
}
fn assert_no_private_bytes(state: &FormalAppState) {
    for entry in walkdir::WalkDir::new(&state.paths.data_dir).follow_links(false) {
        let entry = entry.unwrap();
        if !entry.file_type().is_file() {
            continue;
        }
        let bytes = std::fs::read(entry.path()).unwrap();
        assert!(
            !bytes
                .windows(PRIVATE_TEXT.len())
                .any(|part| part == PRIVATE_TEXT.as_bytes()),
            "辅助输入正文进入了持久文件"
        );
    }
}

async fn assert_submission_lock_available(fixture: &Fixture, task: &str) {
    // 其他并行测试也会短暂登记辅助任务；需要验证“会话期间可取得”，不是某瞬间无人用锁。
    let guard = tokio::time::timeout(Duration::from_secs(2), tasks::submission_lock().lock())
        .await
        .expect("辅助会话长期占据全局提交锁");
    assert!(
        fixture.state.job_supervisor.contains(task).await,
        "不能等辅助会话结束后才取得提交锁"
    );
    assert!(
        !fixture
            .state
            .task_repository
            .get(task)
            .await
            .unwrap()
            .state
            .is_terminal()
    );
    drop(guard);
}

async fn existing_install(fixture: &Fixture, alias: bool) -> String {
    let task_id = uuid::Uuid::now_v7().to_string();
    let mut asset = ScreenRepository::new(fixture.state.local_store.pool().clone())
        .confirmed_asset(&fixture.project, &fixture.screen)
        .await
        .unwrap();
    if alias {
        asset.id = uuid::Uuid::now_v7().to_string();
    }
    let plan = ScreenPlan {
        project_id: fixture.project.clone(),
        targets: vec![asset.clone()],
        business_project_id: None,
        data_source_id: None,
        operator: "fixture".into(),
        instance_id: "fixture-instance".into(),
        created_at: "1".into(),
        detail: json!({}),
        input: ScreenOperationInput {
            action: "install".into(),
            target_ids: vec![asset.id.clone()],
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: "universal".into(),
            reinstall: false,
            concurrency: 1,
            expected_targets: BTreeMap::new(),
            retry_of_operation_id: None,
        },
    };
    task_data::save_plan(fixture.state.local_store.pool(), &task_id, &plan)
        .await
        .unwrap();
    fixture
        .state
        .task_repository
        .create(CreateTask {
            id: task_id.clone(),
            local_project_id: fixture.project.clone(),
            remote_operation_record_id: None,
            domain_type: "smart_screen".into(),
            operation_type: "install".into(),
            name: "已有安装任务".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: fixture
                .state
                .paths
                .project_task_log_path(&fixture.project, &task_id)
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            targets: vec![("smart_screen".into(), asset.id)],
        })
        .await
        .unwrap();
    fixture
        .state
        .job_supervisor
        .register(&task_id)
        .await
        .unwrap();
    let mut current = TaskState::Draft;
    for next in [
        TaskState::Checking,
        TaskState::Ready,
        TaskState::Queued,
        TaskState::Running,
    ] {
        fixture
            .state
            .task_repository
            .transition(&task_id, current, next, None, None)
            .await
            .unwrap();
        current = next;
    }
    task_id
}

#[tokio::test]
async fn assistance_is_visible_to_existing_tasks_and_supervised_without_holding_submission_lock() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    assert!(fixture.state.task_queue.is_tracked(&task).await);
    assert!(fixture.state.job_supervisor.contains(&task).await);
    assert_submission_lock_available(&fixture, &task).await;
    let repository = ScreenRepository::new(fixture.state.local_store.pool().clone());
    assert!(
        repository
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_err()
    );
    let at_ip:i64=sqlx::query_scalar("SELECT count(*) FROM local_task t JOIN local_screen_task_data d ON d.local_task_id=t.id,json_each(d.plan_json,'$.targets') target WHERE t.state='running' AND json_extract(target.value,'$.ip')=?")
        .bind(IP).fetch_one(fixture.state.local_store.pool()).await.unwrap();
    assert_eq!(at_ip, 1, "旧设备IP互斥查询看不到辅助占用");
    reconcile_untracked_active_tasks(
        &fixture.state.task_repository,
        &fixture.state.paths,
        &fixture.state.task_queue,
    )
    .await;
    assert_eq!(
        fixture
            .state
            .task_repository
            .get(&task)
            .await
            .unwrap()
            .state,
        TaskState::Running
    );
    fixture.end().await;
    fixture.settled(&task).await;
    assert!(
        repository
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_ok()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn existing_install_by_screen_or_same_ip_blocks_assistance_before_any_input() {
    for alias in [false, true] {
        let fixture = Fixture::new(Duration::from_secs(10)).await;
        let io = Input::new(false, false);
        let existing = existing_install(&fixture, alias).await;
        let port: Arc<dyn ControlPort> = io.clone();
        assert!(
            remote_screen::begin_control_with(
                &fixture.state,
                &fixture.sessions,
                OWNER,
                &fixture.project,
                &fixture.session,
                port
            )
            .await
            .is_err()
        );
        assert_eq!(io.sends.load(Ordering::SeqCst), 0);
        fixture.state.job_supervisor.finish(&existing).await;
        fixture.finish().await;
    }
}

#[tokio::test]
async fn text_is_sent_once_and_never_persisted_in_plan_database_or_logs() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    let receipt = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame.clone()),
    )
    .await
    .unwrap();
    assert_eq!(receipt.status, "sent");
    assert!(io.private_text_received.load(Ordering::SeqCst));
    let replay = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame),
    )
    .await;
    assert!(replay.is_err(), "发送后未经新截图仍接受旧帧");
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    assert_no_private_bytes(&fixture.state);
    fixture.end().await;
    fixture.settled(&task).await;
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn end_waits_for_inflight_input_before_releasing_existing_task_protection() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(true, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    let state = fixture.state.clone();
    let sessions = fixture.sessions.clone();
    let project = fixture.project.clone();
    let session = fixture.session.clone();
    let action = tokio::spawn(async move {
        remote_screen::act(&state, &sessions, OWNER, &project, &session, text(frame)).await
    });
    io.started.notified().await;
    let sessions = fixture.sessions.clone();
    let project = fixture.project.clone();
    let session = fixture.session.clone();
    let end = tokio::spawn(async move {
        remote_screen::end_control(&sessions, OWNER, &project, &session).await
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!end.is_finished(), "在途输入未收敛就结束了辅助操作");
    assert!(
        ScreenRepository::new(fixture.state.local_store.pool().clone())
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_err()
    );
    assert_submission_lock_available(&fixture, &task).await;
    io.release.notify_one();
    let _ = action.await.unwrap();
    end.await.unwrap().unwrap();
    fixture.settled(&task).await;
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn closing_view_keeps_inflight_protection_until_the_short_command_settles() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(true, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    let state = fixture.state.clone();
    let sessions = fixture.sessions.clone();
    let project = fixture.project.clone();
    let session = fixture.session.clone();
    let action = tokio::spawn(async move {
        remote_screen::act(&state, &sessions, OWNER, &project, &session, text(frame)).await
    });
    io.started.notified().await;
    remote_screen::close(&fixture.sessions, OWNER, &fixture.project, &fixture.session).unwrap();
    assert!(
        remote_screen::capture_with(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            &Capture
        )
        .await
        .is_err()
    );
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(fixture.state.job_supervisor.contains(&task).await);
    assert!(
        ScreenRepository::new(fixture.state.local_store.pool().clone())
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_err()
    );
    io.release.notify_one();
    let _ = action.await.unwrap();
    fixture.settled(&task).await;
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn normal_task_cancel_stops_assistance_and_removes_supervisor_registration() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    request_task_cancel(&fixture.state, &task).await.unwrap();
    fixture.settled(&task).await;
    assert_eq!(
        fixture
            .state
            .task_repository
            .get(&task)
            .await
            .unwrap()
            .state,
        TaskState::Cancelled
    );
    assert!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(frame)
        )
        .await
        .is_err()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn automatic_screenshots_do_not_extend_assistance_idle_timeout() {
    let fixture = Fixture::new(Duration::from_millis(150)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let mut captured = 0;
    for _ in 0..6 {
        // 过期恰好发生在采集期间时，服务会拒绝跨控制状态的帧；这是正确保护。
        match remote_screen::capture_with(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            &Capture,
        )
        .await
        {
            Ok(_) => captured += 1,
            Err(AppError::Conflict(_)) => {}
            Err(_) => panic!("空闲回收期间截图出现非预期错误"),
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    assert!(captured >= 2);
    fixture.settled(&task).await;
    let frame = fixture.frame().await;
    assert!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(frame)
        )
        .await
        .is_err()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn unknown_input_result_is_not_replayed_or_reported_as_login_success() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, true);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    if let Ok(receipt) = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame.clone()),
    )
    .await
    {
        assert_eq!(receipt.status, "unconfirmed");
        assert!(!receipt.control_active);
        assert!(!receipt.message.contains(PRIVATE_TEXT));
    }
    assert!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            text(frame)
        )
        .await
        .is_err()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    fixture.settled(&task).await;
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn wrong_owner_project_or_changed_device_identity_cannot_receive_input() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    for (owner, project) in [
        ("other-window", fixture.project.as_str()),
        (OWNER, "other-project"),
    ] {
        assert!(
            remote_screen::act(
                &fixture.state,
                &fixture.sessions,
                owner,
                project,
                &fixture.session,
                tap(frame.clone())
            )
            .await
            .is_err()
        );
    }
    *io.serial.lock().unwrap() = "different-device".into();
    let outcome = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        ControlInput {
            frame_id: frame,
            action: ControlAction::Key {
                key: ControlKey::Back,
            },
        },
    )
    .await;
    if let Ok(receipt) = outcome {
        assert_ne!(receipt.status, "sent");
    }
    assert_eq!(io.sends.load(Ordering::SeqCst), 0);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

#[tokio::test]
async fn square_screen_rotation_change_stops_control_even_when_dimensions_match() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    // 180度后宽高仍然为1×1，不能只凭尺寸相同接受旧画面位置。
    io.orientation.store(2, Ordering::SeqCst);
    let result = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        tap(frame),
    )
    .await;
    if let Ok(receipt) = result {
        assert_ne!(receipt.status, "sent");
    }
    fixture.settled(&task).await;
    io.orientation.store(0, Ordering::SeqCst);
    let next_frame = fixture.frame().await;
    assert!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(next_frame)
        )
        .await
        .is_err()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 0);
    fixture.finish().await;
}

#[tokio::test]
async fn local_summary_failure_remains_recoverable_without_replaying_input() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    let receipt = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame),
    )
    .await
    .unwrap();
    assert_eq!(receipt.status, "sent");
    sqlx::query("CREATE TRIGGER fixture_remote_summary_failure BEFORE UPDATE OF result_json ON local_screen_task_data BEGIN SELECT RAISE(FAIL, 'isolated summary fixture'); END")
        .execute(fixture.state.local_store.pool()).await.unwrap();
    let _ =
        remote_screen::end_control(&fixture.sessions, OWNER, &fixture.project, &fixture.session)
            .await;
    tokio::time::timeout(Duration::from_secs(8), async {
        while fixture.state.job_supervisor.contains(&task).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("记录失败后辅助执行器没有结束");
    let record = fixture.state.task_repository.get(&task).await.unwrap();
    assert!(
        record.state == TaskState::FinalizingFailed || record.state.is_terminal(),
        "独立辅助执行器结束后不能留下不可核实的Running状态"
    );
    assert!(
        fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    assert!(
        ScreenRepository::new(fixture.state.local_store.pool().clone())
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_err()
    );
    sqlx::query("DROP TRIGGER fixture_remote_summary_failure")
        .execute(fixture.state.local_store.pool())
        .await
        .unwrap();
    retry_pending_local_finalization(&fixture.state, &task, false)
        .await
        .unwrap();
    assert!(
        !fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1, "补存不应重新发送输入");
    assert!(
        ScreenRepository::new(fixture.state.local_store.pool().clone())
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_ok()
    );
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn displayed_frame_can_drive_input_while_background_capture_is_pending() {
    let fixture = Fixture::new(Duration::from_secs(20)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let displayed = fixture.frame().await;
    let blocked = BlockedCapture::new();
    let capture = spawn_blocked_capture(&fixture, &blocked);
    tokio::time::timeout(Duration::from_secs(2), blocked.started.notified())
        .await
        .unwrap();
    let receipt = tokio::time::timeout(
        Duration::from_secs(2),
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(displayed),
        ),
    )
    .await
    .expect("后台截图不应阻止已显示有效画面的输入")
    .unwrap();
    assert_eq!(receipt.status, "sent");
    assert!(!capture.is_finished(), "未释放截图前输入应已完成");
    blocked.release.notify_one();
    let late = tokio::time::timeout(Duration::from_secs(2), capture)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!late.input_ready, "跨过输入动作的截图只能查看");
    rejected_without_sending(&fixture, &io, late.frame_id, 1).await;
    let fresh = fixture.frame().await;
    let receipt = remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        tap(fresh),
    )
    .await
    .unwrap();
    assert_eq!(receipt.status, "sent");
    assert_eq!(io.sends.load(Ordering::SeqCst), 2);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

#[tokio::test]
async fn capture_can_finish_during_input_but_cannot_authorize_the_next_input() {
    let fixture = Fixture::new(Duration::from_secs(20)).await;
    let io = Input::new(true, false);
    let task = fixture.enable(&io).await;
    let displayed = fixture.frame().await;
    let acting = {
        let (state, sessions, project, session, frame) = (
            fixture.state.clone(),
            fixture.sessions.clone(),
            fixture.project.clone(),
            fixture.session.clone(),
            displayed.clone(),
        );
        tokio::spawn(async move {
            remote_screen::act(&state, &sessions, OWNER, &project, &session, tap(frame)).await
        })
    };
    tokio::time::timeout(Duration::from_secs(2), io.started.notified())
        .await
        .unwrap();
    let during = tokio::time::timeout(
        Duration::from_secs(2),
        remote_screen::capture_with(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            &Capture,
        ),
    )
    .await
    .expect("输入在途时截图应可独立完成")
    .unwrap();
    assert!(!during.input_ready);
    rejected_without_sending(&fixture, &io, displayed, 1).await;
    rejected_without_sending(&fixture, &io, during.frame_id.clone(), 1).await;
    assert!(!acting.is_finished());
    io.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), acting)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .status,
        "sent"
    );
    rejected_without_sending(&fixture, &io, during.frame_id, 1).await;
    let fresh = fixture.frame().await;
    io.release.notify_one();
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(fresh)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 2);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

#[tokio::test]
async fn recent_displayed_frame_survives_refresh_but_all_old_generation_frames_expire_on_input() {
    let fixture = Fixture::new(Duration::from_secs(20)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let first = fixture.frame().await;
    let second = fixture.frame().await;
    let third = fixture.frame().await;
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(first)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    rejected_without_sending(&fixture, &io, second, 1).await;
    rejected_without_sending(&fixture, &io, third, 1).await;
    let fresh = fixture.frame().await;
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(fresh)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 2);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

#[tokio::test]
async fn authorization_metadata_is_bounded_and_oldest_frame_cannot_send_after_eviction() {
    let fixture = Fixture::new(Duration::from_secs(20)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let oldest = fixture.frame().await;
    let mut newest = oldest.clone();
    // 当前授权元数据上限32；不持久化图像，也不为验证边界等待30秒。
    for _ in 0..34 {
        newest = fixture.frame().await;
    }
    rejected_without_sending(&fixture, &io, oldest, 0).await;
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(newest)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    assert_no_private_bytes(&fixture.state);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

#[tokio::test]
async fn capture_from_stopped_control_cannot_authorize_reopened_control() {
    let fixture = Fixture::new(Duration::from_secs(20)).await;
    let io = Input::new(false, false);
    let previous_task = fixture.enable(&io).await;
    let previous_frame = fixture.frame().await;
    let blocked = BlockedCapture::new();
    let capturing = spawn_blocked_capture(&fixture, &blocked);
    tokio::time::timeout(Duration::from_secs(2), blocked.started.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), fixture.end())
        .await
        .expect("停止辅助不应等待只读截图");
    fixture.settled(&previous_task).await;
    let task = tokio::time::timeout(Duration::from_secs(2), fixture.enable(&io))
        .await
        .expect("重新开启辅助不应占用截图锁");
    assert_ne!(task, previous_task);
    blocked.release.notify_one();
    let late = tokio::time::timeout(Duration::from_secs(2), capturing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(!late.input_ready);
    rejected_without_sending(&fixture, &io, late.frame_id, 0).await;
    rejected_without_sending(&fixture, &io, previous_frame, 0).await;
    let fresh = fixture.frame().await;
    assert_eq!(
        remote_screen::act(
            &fixture.state,
            &fixture.sessions,
            OWNER,
            &fixture.project,
            &fixture.session,
            tap(fresh)
        )
        .await
        .unwrap()
        .status,
        "sent"
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    fixture.end().await;
    fixture.settled(&task).await;
    fixture.finish().await;
}

async fn fail_finalization_writes(fixture: &Fixture, block_state: bool) {
    sqlx::query("CREATE TRIGGER fixture_remote_summary_failure BEFORE UPDATE OF result_json ON local_screen_task_data BEGIN SELECT RAISE(FAIL, 'isolated summary fixture'); END")
        .execute(fixture.state.local_store.pool()).await.unwrap();
    sqlx::query("CREATE TRIGGER fixture_remote_guard_failure BEFORE INSERT ON local_task_result_guard BEGIN SELECT RAISE(FAIL, 'isolated guard fixture'); END")
        .execute(fixture.state.local_store.pool()).await.unwrap();
    if block_state {
        sqlx::query("CREATE TRIGGER fixture_remote_state_failure BEFORE UPDATE OF state ON local_task WHEN OLD.operation_type='remote_assistance' BEGIN SELECT RAISE(FAIL, 'isolated state fixture'); END")
            .execute(fixture.state.local_store.pool()).await.unwrap();
    }
}
async fn remove_finalization_faults(fixture: &Fixture, block_state: bool) {
    for name in [
        "fixture_remote_summary_failure",
        "fixture_remote_guard_failure",
    ] {
        sqlx::query(&format!("DROP TRIGGER {name}"))
            .execute(fixture.state.local_store.pool())
            .await
            .unwrap();
    }
    if block_state {
        sqlx::query("DROP TRIGGER fixture_remote_state_failure")
            .execute(fixture.state.local_store.pool())
            .await
            .unwrap();
    }
}
async fn worker_finished(fixture: &Fixture, task: &str) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while fixture.state.job_supervisor.contains(task).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("独立辅助执行器未结束");
}

#[tokio::test]
async fn guard_write_failure_does_not_skip_independent_finalizing_transition() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame),
    )
    .await
    .unwrap();
    fail_finalization_writes(&fixture, false).await;
    assert!(
        remote_screen::end_control(&fixture.sessions, OWNER, &fixture.project, &fixture.session)
            .await
            .is_err()
    );
    worker_finished(&fixture, &task).await;
    assert_eq!(
        fixture
            .state
            .task_repository
            .get(&task)
            .await
            .unwrap()
            .state,
        TaskState::FinalizingFailed
    );
    assert!(
        fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    remove_finalization_faults(&fixture, false).await;
    tasks::verify(&fixture.state, &fixture.project, &task)
        .await
        .unwrap();
    assert!(
        !fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1);
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn orphaned_running_after_all_writes_fail_is_visible_and_recoverable_when_storage_returns() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let io = Input::new(false, false);
    let task = fixture.enable(&io).await;
    let frame = fixture.frame().await;
    remote_screen::act(
        &fixture.state,
        &fixture.sessions,
        OWNER,
        &fixture.project,
        &fixture.session,
        text(frame),
    )
    .await
    .unwrap();
    fail_finalization_writes(&fixture, true).await;
    let _ =
        remote_screen::end_control(&fixture.sessions, OWNER, &fixture.project, &fixture.session)
            .await;
    worker_finished(&fixture, &task).await;
    assert_eq!(
        fixture
            .state
            .task_repository
            .get(&task)
            .await
            .unwrap()
            .state,
        TaskState::Running
    );
    let view = tasks::views(&fixture.state, &fixture.project)
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == task)
        .unwrap();
    assert_eq!(view.state, "needs_review");
    assert_eq!(view.targets[0].state, "needs_review");
    let activity =
        inxaiot_desk_buddy_lib::interface::commands::task_activity::query_activity_tasks(
            &fixture.state,
            &fixture.project,
            100,
        )
        .await
        .unwrap();
    let row = activity.iter().find(|x| x.id == task).unwrap();
    assert_eq!(row.state, "finalizing_failed");
    assert!(!row.cancellable && !row.clearable);
    remove_finalization_faults(&fixture, true).await;
    // 使用screen_verify的真实业务路径，而不是直接调用恢复handler。
    tasks::verify(&fixture.state, &fixture.project, &task)
        .await
        .unwrap();
    assert_eq!(
        fixture
            .state
            .task_repository
            .get(&task)
            .await
            .unwrap()
            .state,
        TaskState::Failed
    );
    assert!(
        !fixture
            .state
            .task_repository
            .results_protected(&task)
            .await
            .unwrap()
    );
    assert!(
        ScreenRepository::new(fixture.state.local_store.pool().clone())
            .require_idle(&fixture.project, &fixture.screen)
            .await
            .is_ok()
    );
    assert_eq!(io.sends.load(Ordering::SeqCst), 1, "恢复不能重发输入");
    assert_no_private_bytes(&fixture.state);
    fixture.finish().await;
}

#[tokio::test]
async fn queued_guarded_assistance_with_registered_supervisor_is_not_an_orphan() {
    let fixture = Fixture::new(Duration::from_secs(10)).await;
    let id = uuid::Uuid::now_v7().to_string();
    fixture.state.job_supervisor.register(&id).await.unwrap();
    let asset = ScreenRepository::new(fixture.state.local_store.pool().clone())
        .confirmed_asset(&fixture.project, &fixture.screen)
        .await
        .unwrap();
    let plan = ScreenPlan {
        project_id: fixture.project.clone(),
        targets: vec![asset],
        business_project_id: None,
        data_source_id: None,
        operator: "fixture".into(),
        instance_id: "fixture".into(),
        created_at: "1".into(),
        detail: json!({}),
        input: ScreenOperationInput {
            action: "remote_assistance".into(),
            target_ids: vec![fixture.screen.clone()],
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: String::new(),
            reinstall: false,
            concurrency: 1,
            expected_targets: BTreeMap::new(),
            retry_of_operation_id: None,
        },
    };
    task_data::save_plan(fixture.state.local_store.pool(), &id, &plan)
        .await
        .unwrap();
    fixture
        .state
        .task_repository
        .create(CreateTask {
            id: id.clone(),
            local_project_id: fixture.project.clone(),
            remote_operation_record_id: None,
            domain_type: "smart_screen".into(),
            operation_type: "remote_assistance".into(),
            name: "准备中的辅助操作".into(),
            priority: 0,
            batch_size: 1,
            concurrency: 1,
            payload_ref: None,
            log_path: fixture
                .state
                .paths
                .project_task_log_path(&fixture.project, &id)
                .unwrap()
                .to_string_lossy()
                .into(),
            targets: vec![("smart_screen".into(), fixture.screen.clone())],
        })
        .await
        .unwrap();
    let mut current = TaskState::Draft;
    for next in [TaskState::Checking, TaskState::Ready, TaskState::Queued] {
        fixture
            .state
            .task_repository
            .transition(&id, current, next, None, None)
            .await
            .unwrap();
        current = next;
    }
    fixture
        .state
        .task_repository
        .protect_results(&id, "fixture preparing")
        .await
        .unwrap();
    let task = fixture.state.task_repository.get(&id).await.unwrap();
    assert!(
        !remote_screen::assistance::orphan_needs_review(&fixture.state, &task)
            .await
            .unwrap()
    );
    let view = tasks::views(&fixture.state, &fixture.project)
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.id == id)
        .unwrap();
    assert_eq!(view.state, "running");
    assert!(
        tasks::verify(&fixture.state, &fixture.project, &id)
            .await
            .is_err()
    );
    assert_eq!(
        fixture.state.task_repository.get(&id).await.unwrap().state,
        TaskState::Queued
    );
    fixture.state.job_supervisor.finish(&id).await;
    tasks::verify(&fixture.state, &fixture.project, &id)
        .await
        .unwrap();
    assert!(
        !fixture
            .state
            .task_repository
            .results_protected(&id)
            .await
            .unwrap()
    );
    fixture.finish().await;
}
