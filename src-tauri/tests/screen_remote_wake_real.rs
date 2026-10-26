//! 休眠辅助唤醒实机门禁：42最多一次SLEEP设置，每屏正式Wake最多一次。
//! 74保持自然休眠现场；拒发用例的点按/测试文字/返回不得进入设备发送端口。
#[path = "common/screen_test_support.rs"]
mod support;
use base64::{engine::general_purpose::STANDARD, Engine};
use inxaiot_desk_buddy_lib::{
    application::ports::project_management::ProjectManagementPort,
    core::error::AppError,
    domain::smart_screen::model::ScreenFields,
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository,
        smart_screen::{
            device::{AdbDevice, AndroidTools},
            remote_screen::{
                self, ControlAction, ControlFuture, ControlInput, ControlKey, ControlPort,
                ControlSendContext, DeviceFacts, NativeControl, RemoteScreenFrame,
                RemoteScreenSessions,
            },
        },
        stage75_adapter::Stage75Adapter,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;
type TestResult<T> = Result<T, Box<dyn std::error::Error>>;
const OWNER: &str = "remote-wake-real";
#[derive(Clone, Copy)]
struct Target {
    ip: &'static str,
    size: &'static str,
    serial: &'static str,
    sdk: u32,
    may_sleep: bool,
}
// 先测不会短时自动休眠的42；74最后验证，保持其60秒原休眠设置。
const TARGETS: [Target; 2] = [
    Target {
        ip: "192.168.3.42",
        size: "10",
        serial: "9c000c3c3643071205d",
        sdk: 29,
        may_sleep: true,
    },
    Target {
        ip: "192.168.3.74",
        size: "4",
        serial: "A47A4AK02040",
        sdk: 27,
        may_sleep: false,
    },
];
fn require(ok: bool, message: &'static str) -> TestResult<()> {
    if ok {
        Ok(())
    } else {
        Err(message.into())
    }
}
fn conflict(message: &str) -> AppError {
    AppError::Conflict(message.into())
}
fn hash(value: &Value) -> String {
    hex::encode(Sha256::digest(serde_json::to_vec(value).unwrap()))
}
fn persist(path: &Path, value: &Value) -> TestResult<()> {
    std::fs::write(path.join("result.json"), serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn directory() -> TestResult<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or("无仓库目录")?
        .canonicalize()?;
    let parent = repo.join(".review-tools/remote-screen-20261026");
    for p in [repo.join(".review-tools"), parent.clone()] {
        if p.exists() {
            require(
                !p.is_symlink() && p.canonicalize()?.starts_with(&repo),
                "隔离目录越出仓库或为符号链接",
            )?;
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                require(
                    std::fs::symlink_metadata(&p)?.file_attributes() & 0x400 == 0,
                    "隔离目录不能为重解析点",
                )?;
            }
        }
    }
    let result = parent.join(format!("wake-real-{}", uuid::Uuid::now_v7()));
    require(!result.exists(), "不能复用旧实例数据")?;
    std::fs::create_dir_all(&result)?;
    Ok(result)
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
            ("config", "readConfig" | "readKiosk") | ("home", "readHome")
        ),
        "只允许指定只读管理方法",
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
    let data = regex.captures(&raw).ok_or("未读到管理结果")?;
    let value: Value = serde_json::from_slice(&STANDARD.decode(&data[1])?)?;
    require(value["status"] == "ok", "原生管理读取未成功")?;
    Ok(value["data"].clone())
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
    require(
        serial.trim() == target.serial && sdk.trim().parse::<u32>()? == target.sdk,
        "IP、序列号或API强门禁不匹配",
    )?;
    let config = provider(device, target, "config", "readConfig").await?;
    let h5 = config["environments"]["test"]["h5Url"]
        .as_str()
        .ok_or("缺少测试H5地址")?;
    require(
        reqwest::Url::parse(h5)?.host_str() == Some("192.168.3.44"),
        "H5地址必须保持44",
    )?;
    let kiosk = provider(device, target, "config", "readKiosk").await?;
    require(
        kiosk["storageConfirmed"] == true && kiosk["deviceOwner"] == true,
        "OWNER或锁定持久值未确认",
    )?;
    let home = provider(device, target, "home", "readHome").await?;
    let timeout = device
        .shell(
            target.ip,
            &["settings", "get", "system", "screen_off_timeout"],
            CancellationToken::new(),
        )
        .await?;
    // activityReady/active/lockTaskMode是运行态，可能随休眠和唤醒变化，不冒充持久差异。
    Ok(
        json!({"serial":target.serial,"androidSdk":target.sdk,"configSha256":hash(&config),"h5Host":"192.168.3.44",
        "kiosk":{"configured":kiosk["configured"],"enabled":kiosk["enabled"],"deviceOwner":kiosk["deviceOwner"],"storageConfirmed":kiosk["storageConfirmed"]},
        "home":home,"screenTimeout":timeout}),
    )
}

struct WakeOnly {
    native: NativeControl,
    target: Target,
    entries: AtomicUsize,
    attempts: AtomicUsize,
}
impl ControlPort for WakeOnly {
    fn probe<'a>(
        &'a self,
        ip: &'a str,
        cancel: CancellationToken,
    ) -> ControlFuture<'a, DeviceFacts> {
        Box::pin(async move {
            if ip != self.target.ip {
                return Err(conflict("输入检查目标越界"));
            }
            let facts = self.native.probe(ip, cancel).await?;
            if facts.serial.as_deref() != Some(self.target.serial)
                || facts.android_sdk != self.target.sdk
            {
                return Err(conflict("身份或系统版本不符"));
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
                return Err(conflict("本测试禁止真实点按、文字或非WakeUp按键"));
            }
            self.attempts
                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                .map_err(|_| conflict("每屏正式WakeUp最多一次"))?;
            self.native.send(ip, action, context, cancel).await
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

// 只计算RGB非黑像素数量；不保存像素、图片或网页文字。支持实机screencap的8位RGB/RGBA PNG。
fn frame_meta(frame: &RemoteScreenFrame) -> TestResult<Value> {
    let bytes = STANDARD.decode(&frame.png_base64)?;
    require(bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "截图非PNG")?;
    let (mut position, mut idat, mut bpp) = (8usize, Vec::new(), 0usize);
    while position + 12 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[position..position + 4].try_into()?) as usize;
        let end = position.checked_add(12 + len).ok_or("PNG长度溢出")?;
        require(end <= bytes.len(), "PNG块不完整")?;
        let kind = &bytes[position + 4..position + 8];
        let data = &bytes[position + 8..position + 8 + len];
        if kind == b"IHDR" {
            require(
                len == 13 && data[8] == 8 && data[12] == 0,
                "测试像素检查仅支持非交错8位PNG",
            )?;
            bpp = match data[9] {
                2 => 3,
                6 => 4,
                _ => return Err("测试像素检查不支持此PNG颜色类型".into()),
            };
        }
        if kind == b"IDAT" {
            idat.extend_from_slice(data);
        }
        position = end;
    }
    require(
        bpp > 0 && frame.width <= 4096 && frame.height <= 4096,
        "PNG像素范围无效",
    )?;
    let stride = frame.width as usize * bpp;
    let expected = (stride + 1) * frame.height as usize;
    let mut scan = Vec::new();
    flate2::read::ZlibDecoder::new(idat.as_slice())
        .take(expected as u64 + 1)
        .read_to_end(&mut scan)?;
    require(scan.len() == expected, "PNG像素长度不符")?;
    let mut pixels = vec![0u8; stride * frame.height as usize];
    for row in 0..frame.height as usize {
        let filter = scan[row * (stride + 1)];
        require(filter <= 4, "PNG过滤类型无效")?;
        for col in 0..stride {
            let i = row * stride + col;
            let a = if col >= bpp { pixels[i - bpp] } else { 0 };
            let b = if row > 0 { pixels[i - stride] } else { 0 };
            let c = if row > 0 && col >= bpp {
                pixels[i - stride - bpp]
            } else {
                0
            };
            let predictor = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => {
                    let p = a as i32 + b as i32 - c as i32;
                    let (pa, pb, pc) = (
                        (p - a as i32).abs(),
                        (p - b as i32).abs(),
                        (p - c as i32).abs(),
                    );
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    }
                }
                _ => unreachable!(),
            };
            pixels[i] = scan[row * (stride + 1) + 1 + col].wrapping_add(predictor);
        }
    }
    let nonblack = pixels
        .chunks_exact(bpp)
        .filter(|p| p[..3].iter().any(|v| *v != 0))
        .count();
    Ok(
        json!({"frameId":frame.frame_id,"width":frame.width,"height":frame.height,"pngBytes":bytes.len(),"pngSha256":hex::encode(Sha256::digest(&bytes)),
        "nonBlackPixels":nonblack,"inputReady":frame.input_ready,"inputGeneration":frame.input_generation}),
    )
}
async fn exercise(
    state: &Arc<FormalAppState>,
    sessions: &RemoteScreenSessions,
    project: &str,
    target: Target,
    tools: &AndroidTools,
    report: &mut Value,
) -> TestResult<()> {
    let device = AdbDevice::new(tools.clone());
    device.connect(target.ip, CancellationToken::new()).await?;
    let before = baseline(&device, target).await?;
    report["baseline"] = before.clone();
    let io = Arc::new(WakeOnly {
        native: NativeControl {
            tools: tools.clone(),
        },
        target,
        entries: AtomicUsize::new(0),
        attempts: AtomicUsize::new(0),
    });
    let (mut sleep_setup, mut fallback, mut session_id, mut task_id) = (false, false, None, None);
    let outcome=async{
        let initial=io.probe(target.ip,CancellationToken::new()).await?;
        if target.may_sleep&&initial.awake==Some(true){
            sleep_setup=true; // 先登记尝试；即使ADB响应丢失也进入保底恢复判断。
            device.shell(target.ip,&["input","keyevent","223"],CancellationToken::new()).await?;
        }
        let mut asleep=false;
        for _ in 0..6{
            let facts=io.probe(target.ip,CancellationToken::new()).await?;
            if facts.awake==Some(false){asleep=true;break;}
            if !target.may_sleep{break;}
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        require(asleep,"测试屏不是明确Asleep；74禁止主动构造睡眠")?;
        let screen=ScreenRepository::new(state.local_store.pool().clone()).save_local(project,&ScreenFields{name:format!("{}寸休眠唤醒隔离验收",target.size),ip:target.ip.into(),size:target.size.into(),..Default::default()},None,None).await?;
        let session=remote_screen::open(state,sessions,OWNER,project,&screen).await?;session_id=Some(session.session_id.clone());
        let port:Arc<dyn ControlPort>=io.clone();
        let control=remote_screen::begin_control_with(state,sessions,OWNER,project,&session.session_id,port).await?;
        task_id=Some(control.task_id.clone());report["taskId"]=json!(control.task_id);
        require(control.control_mode=="wake_only","明确休眠未进入仅唤醒模式")?;
        let black=remote_screen::capture(state,sessions,OWNER,project,&session.session_id).await?;
        let black_meta=frame_meta(&black)?;require(black.input_ready&&black_meta["nonBlackPixels"]==0,"休眠现场应有可用于唤醒的纯黑有效帧")?;
        report["sleepFrame"]=black_meta;
        for action in [ControlAction::Tap{x:0,y:0},ControlAction::Text{text:"wake-mode-fixture".into()},ControlAction::Key{key:ControlKey::Back}]{
            let frame=remote_screen::capture(state,sessions,OWNER,project,&session.session_id).await?;
            require(frame.input_ready,"拒发用例必须使用当前有效帧，不能仅因旧帧被拒绝")?;
            let result=remote_screen::act(state,sessions,OWNER,project,&session.session_id,ControlInput{frame_id:frame.frame_id,action}).await;
            require(result.is_err()&&io.entries.load(Ordering::SeqCst)==0,"休眠普通输入未在真实发送端口前拒绝")?;
        }
        report["tapTextBackRejectedWithoutDeviceSend"]=json!(true);
        let frame=remote_screen::capture(state,sessions,OWNER,project,&session.session_id).await?;
        let old_id=frame.frame_id.clone();
        let receipt=remote_screen::act(state,sessions,OWNER,project,&session.session_id,wake(frame.frame_id)).await?;
        report["wakeReceipt"]=json!({"status":receipt.status,"controlMode":receipt.control_mode,"controlActive":receipt.control_active});
        require(receipt.status=="sent"&&receipt.control_mode=="full"&&receipt.control_active,"Wake后未只读确认完整控制能力")?;
        require(remote_screen::act(state,sessions,OWNER,project,&session.session_id,wake(old_id)).await.is_err(),"唤醒前旧图仍可授权")?;
        require(io.entries.load(Ordering::SeqCst)==1&&io.attempts.load(Ordering::SeqCst)==1,"旧图导致重复Wake")?;
        let mut nonblack=false;
        for _ in 0..4{
            let fresh=remote_screen::capture(state,sessions,OWNER,project,&session.session_id).await?;
            let meta=frame_meta(&fresh)?;
            if fresh.input_ready&&meta["nonBlackPixels"].as_u64().unwrap_or(0)>0{report["awakeFrame"]=meta;nonblack=true;break;}
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        require(nonblack,"唤醒后没有取得非纯黑的有效新帧")?;
        report["oldFrameRejectedAndNewFrameReady"]=json!(true);
        Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    if let Some(id) = session_id.as_deref() {
        let ended = remote_screen::end_control(sessions, OWNER, project, id).await;
        let _ = remote_screen::close(sessions, OWNER, project, id);
        report["controlEnded"] = json!(ended.is_ok());
    }
    if sleep_setup && io.attempts.load(Ordering::SeqCst) == 0 {
        fallback = true;
        let restored = device
            .shell(
                target.ip,
                &["input", "keyevent", "224"],
                CancellationToken::new(),
            )
            .await;
        report["fallbackWakeCommandConfirmed"] = json!(restored.is_ok());
    }
    report["sleepSetupAttempts"] = json!(usize::from(sleep_setup));
    report["fallbackWakeAttempts"] = json!(usize::from(fallback));
    report["formalWakeAttempts"] = json!(io.attempts.load(Ordering::SeqCst));
    report["sendPortEntries"] = json!(io.entries.load(Ordering::SeqCst));
    let after = baseline(&device, target).await?;
    report["after"] = after.clone();
    report["configurationUnchanged"] = json!(before == after);
    require(
        before == after,
        "原生配置、H5、主屏、锁定持久值或休眠设置发生变化",
    )?;
    if let Some(task) = task_id {
        require(
            state.task_repository.get(&task).await?.state.is_terminal()
                && !state.job_supervisor.contains(&task).await,
            "唤醒任务未完成收尾",
        )?;
    }
    let guards: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM local_task_result_guard")
        .fetch_one(state.local_store.pool())
        .await?;
    report["remainingGuards"] = json!(guards);
    require(guards == 0, "唤醒任务结果保护未释放")?;
    outcome?;
    require(!fallback, "使用保底恢复不算主流程通过")?;
    require(
        io.probe(target.ip, CancellationToken::new()).await?.awake == Some(true),
        "测试末未保持唤醒",
    )?;
    report["passed"] = json!(true);
    Ok(())
}

#[tokio::test]
#[ignore = "仅74自然休眠与42最多一次SLEEP；正式Wake每屏一次；需要独立实机门禁"]
async fn sleeping_screen_can_open_wake_only_and_recover_full_control_without_changing_configuration(
) -> TestResult<()> {
    require(
        std::env::var("INX_REMOTE_SCREEN_WAKE_REAL").as_deref() == Ok("YES"),
        "缺少休眠唤醒实机独立门禁",
    )?;
    let configured = support::live_screens()?;
    for target in TARGETS {
        require(
            configured
                .iter()
                .any(|s| s.ip == target.ip && s.size == target.size),
            "固定目标不在当前测试说明中",
        )?;
    }
    let evidence = directory()?;
    let state = support::state_at(&evidence.join("app-data"), false).await;
    let sessions = RemoteScreenSessions::default();
    let mut report = json!({"passed":false,"targets":[],"productionDataUsed":false,"sourceDatabaseCopied":false,"screenshotsWritten":false,
        "deviceActions":"42可一次SLEEP设置；每屏正式Wake最多一次；42尚未发主Wake而失败时最多一次保底Wake，且主流程记失败"});
    persist(&evidence, &report)?;
    println!(
        "休眠唤醒实机证据：{}",
        evidence.join("result.json").display()
    );
    let outcome=async{
        let project=Stage75Adapter::new(&state).create_project(support::local_input()).await?.project.id;
        let tools=AndroidTools::discover()?;
        for target in TARGETS{
            let mut row=json!({"ip":target.ip,"size":target.size,"serial":target.serial,"androidSdk":target.sdk,"passed":false});
            let result=exercise(&state,&sessions,&project,target,&tools,&mut row).await;
            report["targets"].as_array_mut().unwrap().push(row);persist(&evidence,&report)?;result?;
        }
        let native=NativeControl{tools};
        for target in TARGETS{require(native.probe(target.ip,CancellationToken::new()).await?.awake==Some(true),"最终双屏并非均处于唤醒状态")?;}
        for entry in walkdir::WalkDir::new(&state.paths.data_dir).follow_links(false){let entry=entry?;if entry.file_type().is_file(){let bytes=std::fs::read(entry.path())?;
            require(!bytes.windows(8).any(|p|p==b"\x89PNG\r\n\x1a\n")&&!bytes.windows(11).any(|p|p==b"iVBORw0KGgo"),"截图正文进入实例文件")?;}}
        report["bothAwakeAtEnd"]=json!(true);Ok::<(),Box<dyn std::error::Error>>(())
    }.await;
    sessions.close_owner(OWNER);
    support::close(state).await;
    report["passed"] = json!(outcome.is_ok());
    persist(&evidence, &report)?;
    outcome
}
