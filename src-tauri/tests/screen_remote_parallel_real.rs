//! 截图与辅助输入并行的独立实机门禁。仅允许42/82，每屏最多两次WakeUp。
//! 人为暂存回包模拟网络在途，不据此声称屏幕像素采集与硬件输入必然重叠。
//! 截图、H5正文与原生配置正文不落盘；不点击业务控件、不输入文字、不修改配置。
#[path = "common/screen_test_support.rs"]
mod support;

use base64::{engine::general_purpose::STANDARD, Engine};
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::{AppError, AppResult},
    domain::smart_screen::model::ScreenFields,
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools},
            remote_screen::{
                self, ControlAction, ControlFuture, ControlInput, ControlKey, ControlPort,
                ControlSendContext, DeviceFacts, NativeControl, NativeScreenCapture, RawCapture,
                RemoteScreenFrame, RemoteScreenSessions, ScreenCapturePort,
            },
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{sync::Notify, time::Instant};
use tokio_util::sync::CancellationToken;

type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const OWNER: &str = "remote-parallel-real";
const TARGETS: [Target; 2] = [
    Target {
        ip: "192.168.3.42",
        size: "10",
        serial: "9c000c3c3643071205d",
        sdk: 29,
    },
    Target {
        ip: "192.168.3.82",
        size: "4",
        serial: "A47A5CK00524",
        sdk: 27,
    },
];
#[derive(Clone, Copy)]
struct Target {
    ip: &'static str,
    size: &'static str,
    serial: &'static str,
    sdk: u32,
}
fn require(ok: bool, message: &'static str) -> TestResult<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn fixed_error(message: &str) -> AppError {
    AppError::Conflict(message.into())
}
fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn persist(path: &Path, value: &Value) -> TestResult<()> {
    std::fs::write(path.join("result.json"), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn evidence_dir() -> TestResult<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("无工作台目录")?
        .canonicalize()?;
    let parent = repo.join(".review-tools/remote-screen-20261026");
    for p in [repo.join(".review-tools"), parent.clone()] {
        if p.exists() {
            require(!p.is_symlink(), "隔离证据目录不能为符号链接")?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                require(
                    std::fs::symlink_metadata(&p)?.file_attributes() & 0x400 == 0,
                    "隔离证据目录不能为重解析点",
                )?;
            }
            require(
                p.canonicalize()?.starts_with(&repo),
                "证据目录必须位于工作台仓库内",
            )?;
        }
    }
    let path = parent.join(format!("parallel-real-{}", uuid::Uuid::now_v7()));
    require(!path.exists(), "随机隔离目录不能复用")?;
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

async fn provider(
    device: &AdbDevice,
    target: Target,
    domain: &str,
    method: &str,
) -> TestResult<Value> {
    require(
        matches!(
            (domain, method),
            ("config", "readConfig" | "readKiosk" | "getCapabilities") | ("home", "readHome")
        ),
        "本测试只允许读取原生配置",
    )?;
    let uri = format!("content://chat.xiaoxin.app.workbench.{domain}");
    let raw = device
        .shell(
            target.ip,
            &["content", "call", "--uri", &uri, "--method", method],
            CancellationToken::new(),
        )
        .await?;
    let regex = regex::Regex::new(r"result=([A-Za-z0-9+/=]+)")?;
    let encoded = regex.captures(&raw).ok_or("只读管理响应无结果")?;
    let result: Value = serde_json::from_slice(&STANDARD.decode(&encoded[1])?)?;
    require(result["status"] == "ok", "原生配置读取未确认成功")?;
    Ok(result["data"].clone())
}

async fn baseline(device: &AdbDevice, target: Target) -> TestResult<Value> {
    let serial = device
        .shell(
            target.ip,
            &["getprop", "ro.serialno"],
            CancellationToken::new(),
        )
        .await?;
    let sdk = device
        .shell(
            target.ip,
            &["getprop", "ro.build.version.sdk"],
            CancellationToken::new(),
        )
        .await?;
    require(serial.trim() == target.serial, "实机硬件序列号不符合强门禁")?;
    require(
        sdk.trim().parse::<u32>()? == target.sdk,
        "实机API版本不符合强门禁",
    )?;
    let capabilities = provider(device, target, "config", "getCapabilities").await?;
    let config = provider(device, target, "config", "readConfig").await?;
    let h5 = config["environments"]["test"]["h5Url"]
        .as_str()
        .ok_or("未读到测试H5地址")?;
    require(
        reqwest::Url::parse(h5)?.host_str() == Some("192.168.3.44"),
        "测试H5地址必须保留44",
    )?;
    let kiosk = provider(device, target, "config", "readKiosk").await?;
    require(
        kiosk["storageConfirmed"] == true && kiosk["deviceOwner"] == true,
        "测试屏OWNER与锁定存储未确认",
    )?;
    let policy = [
        "configured",
        "enabled",
        "deviceOwner",
        "activityReady",
        "lockTaskMode",
        "active",
        "storageConfirmed",
    ]
    .into_iter()
    .map(|key| (key.to_string(), kiosk[key].clone()))
    .collect::<serde_json::Map<String, Value>>();
    let home = provider(device, target, "home", "readHome").await?;
    let timeout = device
        .shell(
            target.ip,
            &["settings", "get", "system", "screen_off_timeout"],
            CancellationToken::new(),
        )
        .await?;
    Ok(
        json!({"serial":target.serial,"androidSdk":target.sdk,"configSha256":hash(&config),
        "h5Host":"192.168.3.44","appVersionName":capabilities["appVersionName"],
        "appVersionCode":capabilities["appVersionCode"],"kiosk":policy,"home":home,"screenTimeout":timeout}),
    )
}

#[derive(Default)]
struct Hold {
    entered: AtomicBool,
    released: AtomicBool,
    started: Notify,
    release: Notify,
}
impl Hold {
    async fn wait_entered(&self) -> TestResult<()> {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let notice = self.started.notified();
                if self.entered.load(Ordering::SeqCst) {
                    break;
                }
                notice.await;
            }
        })
        .await
        .map_err(|_| "真实命令未进入测试回包暂存点")?;
        Ok(())
    }
    async fn pause(&self, deadline: Instant, cancel: CancellationToken) -> AppResult<()> {
        self.entered.store(true, Ordering::SeqCst);
        self.started.notify_waiters();
        loop {
            let notice = self.release.notified();
            if self.released.load(Ordering::SeqCst) {
                return Ok(());
            }
            tokio::select! {
                _=cancel.cancelled()=>return Err(AppError::Cancelled),
                _=tokio::time::sleep_until(deadline)=>return Err(fixed_error("测试回包暂存超时")),
                _=notice=>{},
            }
        }
    }
    fn release(&self) {
        self.released.store(true, Ordering::SeqCst);
        self.release.notify_waiters();
    }
}

struct HeldCapture {
    inner: NativeScreenCapture,
    target: Target,
    hold: Arc<Hold>,
}
impl ScreenCapturePort for HeldCapture {
    async fn capture(&self, ip: &str, cancel: CancellationToken) -> AppResult<Vec<u8>> {
        if ip != self.target.ip {
            return Err(fixed_error("截图目标越出门禁"));
        }
        let bytes = self.inner.capture(ip, cancel.clone()).await?;
        self.hold
            .pause(Instant::now() + Duration::from_secs(20), cancel)
            .await?;
        Ok(bytes)
    }
    async fn capture_verified(&self, ip: &str, cancel: CancellationToken) -> AppResult<RawCapture> {
        if ip != self.target.ip {
            return Err(fixed_error("截图目标越出门禁"));
        }
        let raw = self.inner.capture_verified(ip, cancel.clone()).await?;
        if raw.serial.as_deref() != Some(self.target.serial) {
            return Err(fixed_error("截图序列号不符"));
        }
        self.hold
            .pause(Instant::now() + Duration::from_secs(20), cancel)
            .await?;
        Ok(raw)
    }
}

struct WakeOnlyControl {
    inner: NativeControl,
    target: Target,
    entries: AtomicUsize,
    attempts: AtomicUsize,
    completed: AtomicUsize,
    hold_next: AtomicBool,
    hold: Arc<Hold>,
}
impl ControlPort for WakeOnlyControl {
    fn probe<'a>(
        &'a self,
        ip: &'a str,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, DeviceFacts> {
        Box::pin(async move {
            if ip != self.target.ip {
                return Err(fixed_error("输入检查目标越出门禁"));
            }
            let facts = self.inner.probe(ip, cancel).await?;
            if facts.serial.as_deref() != Some(self.target.serial)
                || facts.android_sdk != self.target.sdk
            {
                return Err(fixed_error("输入检查序列号或系统版本不符"));
            }
            Ok(facts)
        })
    }
    fn send<'a>(
        &'a self,
        ip: &'a str,
        action: &'a ControlAction,
        context: ControlSendContext,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, ()> {
        Box::pin(async move {
            self.entries.fetch_add(1, Ordering::SeqCst);
            if ip != self.target.ip
                || context.android_sdk != self.target.sdk
                || !matches!(
                    action,
                    ControlAction::Key {
                        key: ControlKey::WakeUp
                    }
                )
            {
                return Err(fixed_error(
                    "本测试只允许固定实机WakeUp，不允许文字、点按或其他按键",
                ));
            }
            self.attempts
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    (count < 2).then_some(count + 1)
                })
                .map_err(|_| fixed_error("实机WakeUp每屏最多两次"))?;
            self.inner.send(ip, action, context, cancel.clone()).await?;
            self.completed.fetch_add(1, Ordering::SeqCst);
            if self.hold_next.swap(false, Ordering::SeqCst) {
                self.hold
                    .pause(context.deadline - Duration::from_millis(500), cancel)
                    .await?;
            }
            Ok(())
        })
    }
}

fn wake(frame: String) -> ControlInput {
    ControlInput {
        frame_id: frame,
        action: ControlAction::Key {
            key: ControlKey::WakeUp,
        },
    }
}
fn frame_meta(frame: &RemoteScreenFrame) -> TestResult<Value> {
    let bytes = STANDARD.decode(&frame.png_base64)?;
    require(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "截图未得到PNG")?;
    Ok(
        json!({"frameId":frame.frame_id,"width":frame.width,"height":frame.height,"bytes":bytes.len(),
        "pngSha256":hex::encode(Sha256::digest(&bytes)),"backendElapsedMs":frame.elapsed_ms,
        "inputReady":frame.input_ready,"inputGeneration":frame.input_generation}),
    )
}

async fn reject_old(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    project: &str,
    session: &str,
    frame: String,
    io: &WakeOnlyControl,
    expected: usize,
) -> TestResult<()> {
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        remote_screen::act(state, sessions, OWNER, project, session, wake(frame)),
    )
    .await
    .map_err(|_| "旧帧拒发不应排队等待设备")?;
    require(result.is_err(), "旧或跨操作帧未在设备发送前明确拒绝")?;
    require(
        io.entries.load(Ordering::SeqCst) == expected,
        "旧帧触达了真实输入发送端口",
    )?;
    require(
        io.attempts.load(Ordering::SeqCst) == expected,
        "旧帧增加了真实输入次数",
    )
}

fn no_images(path: &Path) -> TestResult<usize> {
    let mut files = 0;
    for entry in walkdir::WalkDir::new(path).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        files += 1;
        let bytes = std::fs::read(entry.path())?;
        require(
            !bytes.windows(8).any(|part| part == b"\x89PNG\r\n\x1a\n")
                && !bytes.windows(11).any(|part| part == b"iVBORw0KGgo"),
            "工作台实例持久文件含截图正文",
        )?;
        require(
            !matches!(
                entry.path().extension().and_then(|s| s.to_str()),
                Some("png" | "jpg" | "jpeg" | "webp")
            ),
            "实例出现图片文件",
        )?;
    }
    Ok(files)
}

async fn exercise(
    state: &Arc<FormalAppState>,
    sessions: &Arc<RemoteScreenSessions>,
    project: &str,
    target: Target,
    tools: &AndroidTools,
    report: &mut Value,
) -> TestResult<()> {
    let device = AdbDevice::new(tools.clone());
    device.connect(target.ip, CancellationToken::new()).await?;
    let before = baseline(&device, target).await?;
    report["baseline"] = before.clone();
    let screen = ScreenRepository::new(state.local_store.pool().clone())
        .save_local(
            project,
            &ScreenFields {
                name: format!("{}寸截图并行隔离验收", target.size),
                ip: target.ip.into(),
                size: target.size.into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
    let session = remote_screen::open(state, sessions, OWNER, project, &screen).await?;
    require(session.ip == target.ip, "会话连接地址不符")?;
    let capture_hold = Arc::new(Hold::default());
    let input_hold = Arc::new(Hold::default());
    let io = Arc::new(WakeOnlyControl {
        inner: NativeControl {
            tools: tools.clone(),
        },
        target,
        entries: AtomicUsize::new(0),
        attempts: AtomicUsize::new(0),
        completed: AtomicUsize::new(0),
        hold_next: AtomicBool::new(false),
        hold: input_hold.clone(),
    });
    let mut capture_job = None;
    let mut input_job = None;
    let mut task_id = None;
    let outcome = async {
        let port: Arc<dyn ControlPort> = io.clone();
        let control = remote_screen::begin_control_with(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            port,
        )
        .await?;
        task_id = Some(control.task_id.clone());
        report["taskId"] = json!(control.task_id);
        let native = NativeScreenCapture {
            tools: tools.clone(),
        };
        let displayed = remote_screen::capture_with(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            &native,
        )
        .await?;
        require(displayed.input_ready, "开启后的真实画面尚不可操作")?;
        report["displayedFrame"] = frame_meta(&displayed)?;
        let held = Arc::new(HeldCapture {
            inner: NativeScreenCapture {
                tools: tools.clone(),
            },
            target,
            hold: capture_hold.clone(),
        });
        {
            let (state, sessions, project, id, held) = (
                state.clone(),
                sessions.clone(),
                project.to_string(),
                session.session_id.clone(),
                held.clone(),
            );
            capture_job = Some(tokio::spawn(async move {
                remote_screen::capture_with(&state, &sessions, OWNER, &project, &id, &*held).await
            }));
        }
        capture_hold.wait_entered().await?;
        let started = Instant::now();
        let sent = remote_screen::act(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            wake(displayed.frame_id),
        )
        .await?;
        require(
            sent.status == "sent" && io.completed.load(Ordering::SeqCst) == 1,
            "截图回包在途时真实Wake未确认",
        )?;
        require(
            !capture_job.as_ref().unwrap().is_finished(),
            "测试截图回包未维持在途状态",
        )?;
        report["wakeWhileCaptureHeldMs"] = json!(started.elapsed().as_millis());
        report["wakeCompletedBeforeCaptureReleased"] = json!(true);
        capture_hold.release();
        let late = capture_job.take().unwrap().await??;
        require(!late.input_ready, "跨过输入动作的迟到截图被错误授权")?;
        report["lateCapture"] = frame_meta(&late)?;
        reject_old(
            state,
            sessions,
            project,
            &session.session_id,
            late.frame_id,
            &io,
            1,
        )
        .await?;

        let fresh = remote_screen::capture_with(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            &native,
        )
        .await?;
        require(fresh.input_ready, "动作结束后的新帧未恢复授权")?;
        report["betweenActionsFrame"] = frame_meta(&fresh)?;
        io.hold_next.store(true, Ordering::SeqCst);
        {
            let (state, sessions, project, id, frame) = (
                state.clone(),
                sessions.clone(),
                project.to_string(),
                session.session_id.clone(),
                fresh.frame_id,
            );
            input_job = Some(tokio::spawn(async move {
                remote_screen::act(&state, &sessions, OWNER, &project, &id, wake(frame)).await
            }));
        }
        input_hold.wait_entered().await?;
        require(
            io.completed.load(Ordering::SeqCst) == 2,
            "第二次Wake尚未真实完成",
        )?;
        let started = Instant::now();
        let during = remote_screen::capture_with(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            &native,
        )
        .await?;
        require(!during.input_ready, "输入回执在途期间截图被错误授权")?;
        require(
            !input_job.as_ref().unwrap().is_finished(),
            "测试输入回执未维持在途状态",
        )?;
        report["captureWhileInputReceiptHeldMs"] = json!(started.elapsed().as_millis());
        report["inputInFlightFrame"] = frame_meta(&during)?;
        reject_old(
            state,
            sessions,
            project,
            &session.session_id,
            during.frame_id.clone(),
            &io,
            2,
        )
        .await?;
        input_hold.release();
        let sent = input_job.take().unwrap().await??;
        require(sent.status == "sent", "第二次Wake回执未确认")?;
        reject_old(
            state,
            sessions,
            project,
            &session.session_id,
            during.frame_id,
            &io,
            2,
        )
        .await?;
        let final_frame = remote_screen::capture_with(
            state,
            sessions,
            OWNER,
            project,
            &session.session_id,
            &native,
        )
        .await?;
        require(final_frame.input_ready, "最终新帧没有恢复操作条件")?;
        report["finalReadyFrame"] = frame_meta(&final_frame)?;
        report["oldFramesRejectedBeforeSend"] = json!(true);
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    // 任何断言失败也先释放测试暂存，再让既有会话完成收尾；不重新执行动作。
    capture_hold.release();
    input_hold.release();
    if let Some(job) = capture_job {
        let _ = job.await;
    }
    if let Some(job) = input_job {
        let _ = job.await;
    }
    let ended = remote_screen::end_control(sessions, OWNER, project, &session.session_id).await;
    let closed = remote_screen::close(sessions, OWNER, project, &session.session_id);
    report["realSendEntries"] = json!(io.entries.load(Ordering::SeqCst));
    report["wakeAttempts"] = json!(io.attempts.load(Ordering::SeqCst));
    report["wakeCompleted"] = json!(io.completed.load(Ordering::SeqCst));
    let after = baseline(&device, target).await?;
    report["after"] = after.clone();
    report["configurationUnchanged"] = json!(before == after);
    require(
        before == after,
        "并行验收前后原生配置或策略有变化，请保留现场",
    )?;
    ended?;
    closed?;
    if let Some(task) = task_id {
        let record = state.task_repository.get(&task).await?;
        require(
            record.state.is_terminal() && !state.job_supervisor.contains(&task).await,
            "辅助任务未完成收尾",
        )?;
        require(
            !state.task_repository.results_protected(&task).await?,
            "辅助结果保护尚未释放",
        )?;
        report["taskTerminalAndUntracked"] = json!(true);
    }
    let guards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task_result_guard")
        .fetch_one(state.local_store.pool())
        .await?;
    require(guards == 0, "隔离实例仍有结果保护")?;
    report["remainingGuards"] = json!(guards);
    report["checkedInstanceFiles"] = json!(no_images(&state.paths.data_dir)?);
    report["screenshotsWritten"] = json!(false);
    outcome?;
    require(
        io.entries.load(Ordering::SeqCst) == 2
            && io.attempts.load(Ordering::SeqCst) == 2
            && io.completed.load(Ordering::SeqCst) == 2,
        "真实Wake次数不符合每屏两次门禁",
    )?;
    report["passed"] = json!(true);
    Ok(())
}

#[tokio::test]
#[ignore = "固定42/82真实截图及每屏最多2次WakeUp；须独立门禁，不点击业务或输入文字"]
async fn real_capture_and_wake_up_can_overlap_in_flight_without_reauthorizing_old_frames(
) -> TestResult<()> {
    require(
        std::env::var("INX_REMOTE_SCREEN_PARALLEL_REAL").as_deref() == Ok("YES"),
        "缺少并行实机验证独立门禁",
    )?;
    let configured = support::live_screens()?;
    for target in TARGETS {
        require(
            configured
                .iter()
                .any(|screen| screen.ip == target.ip && screen.size == target.size),
            "固定目标不在当前测试说明中",
        )?;
    }
    let directory = evidence_dir()?;
    let state = support::state_at(&directory.join("app-data"), false).await;
    let sessions = Arc::new(RemoteScreenSessions::default());
    let mut report = json!({"passed":false,"gate":"INX_REMOTE_SCREEN_PARALLEL_REAL=YES","productionDataUsed":false,
        "sourceDatabaseCopied":false,"onlyDeviceWrite":"WakeUp","maximumWakeAttemptsPerScreen":2,
        "targets":[],"bothSizesCovered":false,"screenshotsWritten":false,
        "holdMeaning":"人为暂存已完成真实调用的回包，模拟网络在途；不证明硬件像素采集与写命令必然重叠"});
    persist(&directory, &report)?;
    println!(
        "截图输入并行实机元数据：{}",
        directory.join("result.json").display()
    );
    let outcome=async {
        let project=Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let tools=AndroidTools::discover()?;
        for target in TARGETS {
            let mut detail=json!({"ip":target.ip,"size":target.size,"serial":target.serial,"androidSdk":target.sdk,"passed":false});
            let result=exercise(&state,&sessions,&project,target,&tools,&mut detail).await;
            report["targets"].as_array_mut().unwrap().push(detail); persist(&directory,&report)?;
            result?;
            println!("{}寸：两次WakeUp、截图/输入回包在途、旧帧拒发和配置保持通过",target.size);
        }
        Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    sessions.close_owner(OWNER);
    support::close(state).await;
    report["passed"] = json!(outcome.is_ok());
    report["bothSizesCovered"] =
        json!(outcome.is_ok() && report["targets"].as_array().unwrap().len() == 2);
    persist(&directory, &report)?;
    outcome
}
