//! 单屏远程查看与辅助输入。图片和输入正文仅在内存，辅助操作另存占用与安全结果摘要。
pub mod assistance;
mod input;
mod png;
mod transport;
pub use assistance::{act, begin_control, begin_control_with, end_control};
pub use input::{
    ActionReceipt, ControlAction, ControlFuture, ControlInput, ControlKey, ControlPort,
    ControlSendContext, ControlView, DeviceFacts, NativeControl,
};
pub use transport::{NativeScreenCapture, RawCapture, ScreenCapturePort};

use super::device::AndroidTools;
use crate::{
    application::{
        ports::project_access::ProjectAccessPort, project_access::ProjectAccessRequirement,
        smart_screen::operations::same_target,
    },
    core::error::{AppError, AppResult},
    domain::smart_screen::model::ScreenAsset,
    formal::app_state::FormalAppState,
    infrastructure::{
        local_sqlite::screen_repository::ScreenRepository, stage75_adapter::Stage75Adapter,
    },
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

const SESSION_IDLE: Duration = Duration::from_secs(10 * 60);
const MAX_WINDOWS: usize = 8;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_FRAME_STAMPS: usize = 32;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteScreenSessionView {
    pub session_id: String,
    pub project_id: String,
    pub screen_id: String,
    pub name: String,
    pub ip: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteScreenFrame {
    pub session_id: String,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub captured_at: String,
    pub png_base64: String,
    pub elapsed_ms: u64,
    pub control_active: bool,
    pub input_ready: bool,
    pub input_generation: u64,
}

#[derive(Clone)]
struct Binding {
    screen: ScreenAsset,
    connection_revision: String,
    scope: Option<(String, String)>,
}
struct Session {
    view: RemoteScreenSessionView,
    binding: Binding,
    cancel: CancellationToken,
    capture: tokio::sync::Mutex<()>,
    touched: Mutex<Instant>,
    frames: Mutex<FrameHistory>,
    control: Mutex<Option<Arc<assistance::ControlHandle>>>,
}
#[derive(Clone)]
struct FrameStamp {
    id: String,
    width: u32,
    height: u32,
    captured: Instant,
    control_task: Option<String>,
    serial: Option<String>,
    generation: u64,
}
#[derive(Default)]
struct FrameHistory {
    generation: u64,
    input_in_flight: bool,
    stamps: VecDeque<FrameStamp>,
}
impl FrameHistory {
    fn accepts_capture(&self, generation: u64, started_during_input: bool) -> bool {
        !started_during_input && !self.input_in_flight && generation == self.generation
    }
    fn advance(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.stamps.clear();
    }
    fn retain(&mut self, stamp: FrameStamp) {
        self.stamps
            .retain(|frame| frame.captured.elapsed() <= input::FRAME_MAX_AGE);
        self.stamps.push_back(stamp);
        while self.stamps.len() > MAX_FRAME_STAMPS {
            self.stamps.pop_front();
        }
    }
}
/// 随单个已接收动作移动，提前拒绝、超时或取消均清理动作代次；不占截图锁。
struct FrameAction {
    stamp: FrameStamp,
    session: Arc<Session>,
}
impl Drop for FrameAction {
    fn drop(&mut self) {
        if let Ok(mut frames) = self.session.frames.lock() {
            frames.advance();
            frames.input_in_flight = false;
        }
    }
}
struct WindowSlot {
    opening_id: String,
    session: Option<Arc<Session>>,
}

pub struct RemoteScreenSessions {
    windows: Mutex<HashMap<String, WindowSlot>>,
    control_idle_timeout: Duration,
}
impl Default for RemoteScreenSessions {
    fn default() -> Self {
        Self::with_control_idle_timeout(Duration::from_secs(5 * 60))
    }
}
impl RemoteScreenSessions {
    pub fn with_control_idle_timeout(timeout: Duration) -> Self {
        Self {
            windows: Mutex::new(HashMap::new()),
            control_idle_timeout: timeout.max(Duration::from_millis(1)),
        }
    }
    fn lock(&self) -> AppResult<std::sync::MutexGuard<'_, HashMap<String, WindowSlot>>> {
        self.windows
            .lock()
            .map_err(|_| AppError::Conflict("远程屏幕会话暂不可用，请重新打开".into()))
    }
    fn begin_open(&self, owner: &str) -> AppResult<String> {
        if owner.is_empty() || owner.len() > 128 {
            return Err(AppError::InvalidConfig("远程屏幕窗口标识无效".into()));
        }
        let mut windows = self.lock()?;
        windows.retain(|_, slot| {
            let expired = slot
                .session
                .as_ref()
                .is_some_and(|session| session.expired());
            if expired && let Some(session) = &slot.session {
                session.cancel.cancel();
            }
            !expired
        });
        if !windows.contains_key(owner) && windows.len() >= MAX_WINDOWS {
            return Err(AppError::Conflict(
                "打开的远程屏幕过多，请先关闭其他窗口".into(),
            ));
        }
        let opening_id = uuid::Uuid::now_v7().to_string();
        if let Some(previous) = windows.insert(
            owner.into(),
            WindowSlot {
                opening_id: opening_id.clone(),
                session: None,
            },
        ) && let Some(session) = previous.session
        {
            session.cancel.cancel();
        }
        Ok(opening_id)
    }
    fn finish_open(
        &self,
        owner: &str,
        opening_id: &str,
        project: &str,
        binding: Binding,
    ) -> AppResult<RemoteScreenSessionView> {
        let mut windows = self.lock()?;
        let slot = windows
            .get_mut(owner)
            .filter(|slot| slot.opening_id == opening_id)
            .ok_or(AppError::Cancelled)?;
        let view = RemoteScreenSessionView {
            session_id: uuid::Uuid::now_v7().to_string(),
            project_id: project.into(),
            screen_id: binding.screen.id.clone(),
            name: binding.screen.fields.name.clone(),
            ip: binding.screen.fields.ip.clone(),
        };
        slot.session = Some(Arc::new(Session {
            view: view.clone(),
            binding,
            cancel: CancellationToken::new(),
            capture: tokio::sync::Mutex::new(()),
            touched: Mutex::new(Instant::now()),
            frames: Mutex::new(FrameHistory::default()),
            control: Mutex::new(None),
        }));
        Ok(view)
    }
    fn abandon_open(&self, owner: &str, opening_id: &str) {
        if let Ok(mut windows) = self.lock()
            && windows
                .get(owner)
                .is_some_and(|slot| slot.opening_id == opening_id)
        {
            windows.remove(owner);
        }
    }
    fn session(&self, owner: &str, project: &str, session_id: &str) -> AppResult<Arc<Session>> {
        let mut windows = self.lock()?;
        let session = windows
            .get(owner)
            .and_then(|slot| slot.session.clone())
            .filter(|session| {
                session.view.session_id == session_id && session.view.project_id == project
            })
            .ok_or_else(|| AppError::NotFound("远程屏幕会话已关闭，请重新打开".into()))?;
        if session.expired() || session.cancel.is_cancelled() {
            session.cancel.cancel();
            windows.remove(owner);
            return Err(AppError::NotFound("远程屏幕会话已过期，请重新打开".into()));
        }
        session.touch()?;
        Ok(session)
    }
    pub fn close_owner(&self, owner: &str) {
        if let Ok(mut windows) = self.lock()
            && let Some(slot) = windows.remove(owner)
            && let Some(session) = slot.session
        {
            session.cancel.cancel();
        }
    }

    pub fn close_all(&self) {
        if let Ok(mut windows) = self.lock() {
            for slot in windows.values() {
                if let Some(session) = &slot.session {
                    session.cancel.cancel();
                }
            }
            windows.clear();
        }
    }
}
impl Session {
    fn frame_history(&self) -> AppResult<std::sync::MutexGuard<'_, FrameHistory>> {
        self.frames
            .lock()
            .map_err(|_| AppError::Conflict("屏幕画面状态已失效，请重新打开".into()))
    }
    fn input_generation(&self) -> u64 {
        self.frames
            .lock()
            .map(|frames| frames.generation)
            .unwrap_or(u64::MAX)
    }
    fn begin_frame_action(
        self: &Arc<Self>,
        frame_id: &str,
        task: &str,
        action: &ControlAction,
    ) -> AppResult<FrameAction> {
        let mut frames = self.frame_history()?;
        if frames.input_in_flight {
            return Err(AppError::Conflict("正在发送操作，请等待操作完成".into()));
        }
        let stamp = frames
            .stamps
            .iter()
            .find(|frame| {
                frame.id == frame_id
                    && frame.control_task.as_deref() == Some(task)
                    && frame.generation == frames.generation
                    && frame.captured.elapsed() <= input::FRAME_MAX_AGE
            })
            .cloned()
            .ok_or_else(|| AppError::Conflict("画面已变化或过期，请刷新后再操作".into()))?;
        if let ControlAction::Tap { x, y } = action
            && (*x >= stamp.width || *y >= stamp.height)
        {
            return Err(AppError::InvalidConfig("点击位置超出当前画面".into()));
        }
        frames.advance();
        frames.input_in_flight = true;
        Ok(FrameAction {
            stamp,
            session: self.clone(),
        })
    }
    fn expired(&self) -> bool {
        self.touched
            .lock()
            .map(|time| time.elapsed() > SESSION_IDLE)
            .unwrap_or(true)
    }
    fn touch(&self) -> AppResult<()> {
        *self
            .touched
            .lock()
            .map_err(|_| AppError::Conflict("远程屏幕会话已失效，请重新打开".into()))? =
            Instant::now();
        Ok(())
    }
}

async fn binding(state: &FormalAppState, project: &str, screen_id: &str) -> AppResult<Binding> {
    Stage75Adapter::new(state)
        .require_project_access(project, ProjectAccessRequirement::Configured)
        .await?;
    let repository = ScreenRepository::new(state.local_store.pool().clone());
    let revision = repository.project_connection_revision(project).await?;
    let screen = repository.confirmed_asset(project, screen_id).await?;
    screen
        .fields
        .ip
        .parse::<std::net::Ipv4Addr>()
        .map_err(|_| AppError::InvalidConfig("屏的已确认 IP 无效，请先核对资料".into()))?;
    let scope = repository.scope(project).await?;
    if revision != repository.project_connection_revision(project).await? {
        return Err(AppError::Conflict("项目连接已改变，请重新打开屏幕".into()));
    }
    Ok(Binding {
        screen,
        connection_revision: revision,
        scope,
    })
}

fn same_binding(expected: &Binding, current: &Binding) -> bool {
    expected.connection_revision == current.connection_revision
        && expected.scope == current.scope
        && same_target(&expected.screen, &current.screen)
}

pub async fn open(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    screen_id: &str,
) -> AppResult<RemoteScreenSessionView> {
    let opening_id = sessions.begin_open(owner)?;
    let result = async {
        let target = binding(state, project, screen_id).await?;
        sessions.finish_open(owner, &opening_id, project, target)
    }
    .await;
    if result.is_err() {
        sessions.abandon_open(owner, &opening_id);
    }
    result
}

pub fn close(
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
) -> AppResult<()> {
    let mut windows = sessions.lock()?;
    if let Some(session) = windows.get(owner).and_then(|slot| slot.session.as_ref())
        && session.view.session_id == session_id
        && session.view.project_id == project
    {
        session.cancel.cancel();
        windows.remove(owner);
    }
    Ok(())
}

pub async fn capture(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
) -> AppResult<RemoteScreenFrame> {
    capture_with(
        state,
        sessions,
        owner,
        project,
        session_id,
        &NativeScreenCapture {
            tools: AndroidTools::discover()?,
        },
    )
    .await
}

pub async fn capture_with<T: ScreenCapturePort>(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
    transport: &T,
) -> AppResult<RemoteScreenFrame> {
    let session = sessions.session(owner, project, session_id)?;
    let _capture = session
        .capture
        .try_lock()
        .map_err(|_| AppError::Conflict("正在获取屏幕，请等待当前刷新完成".into()))?;
    let started = Instant::now();
    let (capture_generation, started_during_input) = {
        let frames = session.frame_history()?;
        (frames.generation, frames.input_in_flight)
    };
    let operation = async {
        if !same_binding(
            &session.binding,
            &binding(state, project, &session.view.screen_id).await?,
        ) {
            session.cancel.cancel();
            return Err(AppError::Conflict(
                "屏或项目资料已改变，请重新打开屏幕".into(),
            ));
        }
        let control = assistance::active_control(&session)?;
        let capture_task = control.as_ref().map(|value| value.task_id.clone());
        let raw = if control.is_some() {
            transport
                .capture_verified(&session.view.ip, session.cancel.clone())
                .await?
        } else {
            RawCapture {
                png: transport
                    .capture(&session.view.ip, session.cancel.clone())
                    .await?,
                serial: None,
            }
        };
        if let Some(control) = &control {
            control.verify_frame_serial(raw.serial.as_deref())?;
        }
        let bytes = raw.png;
        let (width, height, png_base64) = tokio::task::spawn_blocking(move || {
            let (width, height) = png::dimensions(&bytes)?;
            Ok::<_, AppError>((width, height, STANDARD.encode(bytes)))
        })
        .await
        .map_err(|_| AppError::Conflict("屏幕图片处理失败，请重新刷新".into()))??;
        if !same_binding(
            &session.binding,
            &binding(state, project, &session.view.screen_id).await?,
        ) {
            session.cancel.cancel();
            return Err(AppError::Conflict(
                "屏或项目资料在采集期间发生变化，请重新打开".into(),
            ));
        }
        let active = sessions.session(owner, project, session_id)?;
        if !Arc::ptr_eq(&session, &active) || session.cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        let current_task = assistance::active_control(&session)?
            .as_ref()
            .map(|value| value.task_id.clone());
        let captured_at = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .map_err(|_| AppError::Conflict("无法记录截图时间".into()))?;
        let frame_id = uuid::Uuid::now_v7().to_string();
        let input_ready = {
            let mut frames = session.frame_history()?;
            let ready = current_task.is_some()
                && current_task == capture_task
                && frames.accepts_capture(capture_generation, started_during_input)
                && started.elapsed() <= input::FRAME_MAX_AGE;
            if ready {
                frames.retain(FrameStamp {
                    id: frame_id.clone(),
                    width,
                    height,
                    captured: started,
                    control_task: capture_task,
                    serial: raw.serial,
                    generation: capture_generation,
                });
            }
            ready
        };
        Ok(RemoteScreenFrame {
            session_id: session_id.into(),
            frame_id,
            width,
            height,
            captured_at,
            png_base64,
            elapsed_ms: started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
            control_active: current_task.is_some(),
            input_ready,
            input_generation: capture_generation,
        })
    };
    tokio::select! {
        biased;
        _ = session.cancel.cancelled() => Err(AppError::Cancelled),
        result = tokio::time::timeout(CAPTURE_TIMEOUT, operation) => result.unwrap_or_else(|_| Err(AppError::timeout("获取当前屏幕"))),
    }
}

#[cfg(test)]
mod tests;
