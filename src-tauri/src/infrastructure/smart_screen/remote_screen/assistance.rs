//! 短生命周期的辅助操作任务。输入只经内存通道传递，持久数据仅含会话归属、计数和结果。
use super::super::{
    leases::HeldScreenLeases,
    platform_write,
    shared_results::{ScreenSharedResults, SharedScreenOperation},
    task_data, tasks,
    write_context::{self, ScreenWriteContext},
};
use super::{
    FrameAction, FrameStamp, RemoteScreenSessions, Session,
    input::{self, ActionReceipt, ControlInput, ControlPort, ControlView, DeviceFacts},
};
use crate::{
    application::{
        ports::project_access::ProjectAccessPort, project_access::ProjectAccessRequirement,
    },
    core::error::{AppError, AppResult},
    domain::{
        common::task::{TargetState, TaskEventLevel, TaskRecord, TaskState},
        smart_screen::{
            model::{ResultState, ScreenAsset, ScreenFields, ScreenTargetResult},
            operation::{ScreenOperationInput, ScreenPlan, ScreenResults},
            rules::{normalize_mac, valid_mac},
        },
    },
    formal::{
        app_state::FormalAppState,
        resource_lease_repository::{LeaseGrant, ResourceLeaseRepository},
    },
    infrastructure::{
        client_instance::application_instance_id,
        local_sqlite::{
            screen_repository::{ScreenRepository, now},
            task_repository::TargetUpdate,
        },
        project_context::map_formal_error,
        stage75_adapter::Stage75Adapter,
    },
};
use futures_util::FutureExt;
use serde_json::json;
use sqlx::Row;
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Notify, mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub const OPERATION: &str = "remote_assistance";
const PREPARE_TIMEOUT: Duration = Duration::from_secs(60);

struct ActionEnvelope {
    input: ControlInput,
    frame: FrameAction,
    reply: oneshot::Sender<AppResult<ActionReceipt>>,
}
pub(super) struct ControlHandle {
    pub task_id: String,
    stop: CancellationToken,
    active: AtomicBool,
    in_flight: AtomicBool,
    wake_only: AtomicBool,
    finished: AtomicBool,
    finalization_failed: AtomicBool,
    done: Notify,
    sender: mpsc::Sender<ActionEnvelope>,
    identity: Mutex<Option<DeviceFacts>>,
}
impl ControlHandle {
    fn control_mode(&self) -> &'static str {
        if self.wake_only.load(Ordering::SeqCst) {
            "wake_only"
        } else {
            "full"
        }
    }
    fn stop(&self) {
        self.active.store(false, Ordering::SeqCst);
        self.stop.cancel();
    }
    fn finish(&self) {
        self.stop();
        self.in_flight.store(false, Ordering::SeqCst);
        self.finished.store(true, Ordering::SeqCst);
        self.done.notify_waiters();
    }
    pub(super) fn verify_frame_serial(&self, serial: Option<&str>) -> AppResult<()> {
        let facts = self
            .identity
            .lock()
            .map_err(|_| AppError::Conflict("辅助操作身份已失效".into()))?;
        let expected = facts
            .as_ref()
            .ok_or_else(|| AppError::Conflict("辅助操作尚未就绪".into()))?;
        if expected.serial.as_deref() != serial {
            return Err(AppError::Conflict(
                "屏幕设备身份已改变，请结束辅助操作后重新连接".into(),
            ));
        }
        Ok(())
    }
}
pub(super) fn active_control(session: &Session) -> AppResult<Option<Arc<ControlHandle>>> {
    Ok(session
        .control
        .lock()
        .map_err(|_| AppError::Conflict("辅助操作会话已失效".into()))?
        .as_ref()
        .filter(|h| {
            h.active.load(Ordering::SeqCst)
                && !h.stop.is_cancelled()
                && !h.finished.load(Ordering::SeqCst)
        })
        .cloned())
}

/// 只克隆既有共享句柄，不创建连接、数据库或第二套应用状态。
fn shared_state(state: &FormalAppState) -> Arc<FormalAppState> {
    Arc::new(FormalAppState {
        local_store: state.local_store.clone(),
        secret_store: state.secret_store.clone(),
        runtime_registry: state.runtime_registry.clone(),
        job_supervisor: state.job_supervisor.clone(),
        task_handler_registry: state.task_handler_registry.clone(),
        task_recovery_registry: state.task_recovery_registry.clone(),
        task_queue: state.task_queue.clone(),
        task_event_bus: state.task_event_bus.clone(),
        task_repository: state.task_repository.clone(),
        task_event_pipeline: state.task_event_pipeline.clone(),
        paths: state.paths.clone(),
    })
}

async fn critical_draft(
    state: &FormalAppState,
    project: &str,
    screen: &ScreenAsset,
) -> AppResult<bool> {
    if screen.source != "platform" {
        return Ok(false);
    }
    let row=sqlx::query("SELECT d.base_json,d.values_json FROM local_screen_draft d JOIN local_screen_context c ON c.local_project_id=d.local_project_id AND c.business_project_id=d.business_project_id WHERE d.local_project_id=? AND d.platform_screen_id=?")
        .bind(project).bind(&screen.id).fetch_optional(state.local_store.pool()).await.map_err(|e|AppError::database("核对辅助操作资料",&e))?;
    let Some(row) = row else { return Ok(false) };
    let decode = |text: String| {
        serde_json::from_str::<ScreenFields>(&text)
            .map_err(|_| AppError::Conflict("屏资料草稿异常，请先核对".into()))
    };
    let base = decode(
        row.try_get("base_json")
            .map_err(|e| AppError::database("读取辅助操作草稿", &e))?,
    )?;
    let value = decode(
        row.try_get("values_json")
            .map_err(|e| AppError::database("读取辅助操作草稿", &e))?,
    )?;
    Ok((value.ip != base.ip && value.ip != screen.fields.ip)
        || (normalize_mac(&value.mac) != normalize_mac(&base.mac)
            && normalize_mac(&value.mac) != normalize_mac(&screen.fields.mac))
        || (value.size != base.size && value.size != screen.fields.size))
}
async fn device_idle(state: &FormalAppState, ip: &str) -> AppResult<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_task t JOIN local_screen_task_data d ON d.local_task_id=t.id,json_each(d.plan_json,'$.targets') target WHERE t.domain_type='smart_screen' AND json_extract(target.value,'$.ip')=? AND (t.state IN ('draft','checking','ready','queued','running','cancelling','finalizing_failed') OR EXISTS(SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id))")
        .bind(ip).fetch_one(state.local_store.pool()).await.map_err(|e|AppError::database("核对同地址辅助操作占用",&e))?;
    if count != 0 {
        return Err(AppError::Conflict(
            "该地址已有操作或待核实结果，请先处理".into(),
        ));
    }
    Ok(())
}
async fn validate_binding(state: &FormalAppState, session: &Session) -> AppResult<()> {
    let current = super::binding(state, &session.view.project_id, &session.view.screen_id).await?;
    if !super::same_binding(&session.binding, &current)
        || critical_draft(state, &session.view.project_id, &current.screen).await?
    {
        return Err(AppError::Conflict(
            "屏或项目资料已改变或关键修改尚未提交，请重新核对".into(),
        ));
    }
    Ok(())
}
fn validate_device(
    expected: &ScreenAsset,
    baseline: Option<&DeviceFacts>,
    facts: &DeviceFacts,
) -> AppResult<()> {
    input::text_input_behavior(facts.android_sdk)?;
    // 身份与几何信息独立于前台；休眠时仍须能验证目标并仅执行系统唤醒。
    if valid_mac(&expected.fields.mac)
        && facts.mac.as_deref().map(normalize_mac).as_deref()
            != Some(normalize_mac(&expected.fields.mac).as_str())
    {
        return Err(AppError::Conflict(
            "设备MAC与已确认资料不一致，请先核对身份".into(),
        ));
    }
    if facts.serial.is_none() && !valid_mac(&expected.fields.mac) {
        return Err(AppError::Conflict(
            "未取得可核实的设备身份，暂不能辅助操作".into(),
        ));
    }
    if let Some(previous) = baseline
        && (previous.serial != facts.serial
            || previous.mac != facts.mac
            || previous.android_sdk != facts.android_sdk)
    {
        return Err(AppError::Conflict(
            "设备身份在会话中发生变化，已停止输入".into(),
        ));
    }
    if let Some(previous) = baseline
        && (previous.width != facts.width
            || previous.height != facts.height
            || previous.orientation != facts.orientation)
    {
        return Err(AppError::Conflict(
            "屏幕尺寸或方向已改变，请刷新后重新开启辅助操作".into(),
        ));
    }
    Ok(())
}

fn plan(session: &Session) -> ScreenPlan {
    ScreenPlan {
        project_id: session.view.project_id.clone(),
        input: ScreenOperationInput {
            action: OPERATION.into(),
            target_ids: vec![session.view.screen_id.clone()],
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: String::new(),
            reinstall: false,
            concurrency: 1,
            expected_targets: BTreeMap::new(),
            retry_of_operation_id: None,
        },
        targets: vec![session.binding.screen.clone()],
        business_project_id: session.binding.scope.as_ref().map(|x| x.0.clone()),
        data_source_id: session.binding.scope.as_ref().map(|x| x.1.clone()),
        operator: "本机辅助操作".into(),
        instance_id: application_instance_id().into(),
        created_at: now(),
        detail: json!({"formatVersion":1,"kind":OPERATION}),
    }
}
async fn persist_placeholder(state: &FormalAppState, id: &str, plan: &ScreenPlan) -> AppResult<()> {
    let (value, hash) = task_data::plan_json(plan)?;
    let timestamp = now();
    let log = state
        .paths
        .project_task_log_path(&plan.project_id, id)
        .map_err(map_formal_error)?
        .to_string_lossy()
        .into_owned();
    // Queue状态、单屏目标、无输入值的计划和恢复保护一次提交，避免留下无法恢复的半份Draft。
    let mut tx = state
        .local_store
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| AppError::database("开始辅助操作登记", &e))?;
    sqlx::query("INSERT INTO local_task(id,local_project_id,domain_type,operation_type,name,state,priority,batch_size,concurrency,sequence,log_path,created_at,updated_at) VALUES(?,?,'smart_screen',?,'远程辅助操作','queued',0,1,1,0,?,?,?)")
        .bind(id).bind(&plan.project_id).bind(OPERATION).bind(log).bind(&timestamp).bind(&timestamp).execute(&mut *tx).await.map_err(|e|AppError::database("登记辅助操作",&e))?;
    sqlx::query("INSERT INTO local_task_target(local_task_id,resource_type,resource_key,state,stage,progress,progress_current,progress_total,updated_at) VALUES(?,'smart_screen',?,'pending','准备辅助操作',0,0,1,?)")
        .bind(id).bind(&plan.targets[0].id).bind(&timestamp).execute(&mut *tx).await.map_err(|e|AppError::database("登记辅助操作目标",&e))?;
    sqlx::query("INSERT INTO local_screen_task_data(local_task_id,local_project_id,plan_json,plan_sha256,result_json,updated_at) VALUES(?,?,?,?,?,?)")
        .bind(id).bind(&plan.project_id).bind(value).bind(hash).bind(json!({"targets":{},"finished":false}).to_string()).bind(&timestamp).execute(&mut *tx).await.map_err(|e|AppError::database("登记辅助操作依据",&e))?;
    sqlx::query("INSERT INTO local_task_result_guard(local_task_id,reason,created_at) VALUES(?,'辅助操作尚未收尾',?)").bind(id).bind(&timestamp).execute(&mut *tx).await.map_err(|e|AppError::database("保护辅助操作结果",&e))?;
    tx.commit()
        .await
        .map_err(|e| AppError::database("提交辅助操作登记", &e))?;
    Ok(())
}

pub async fn begin_control(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
) -> AppResult<ControlView> {
    begin_control_with(
        state,
        sessions,
        owner,
        project,
        session_id,
        input::native_control()?,
    )
    .await
}
pub async fn begin_control_with(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
    io: Arc<dyn ControlPort>,
) -> AppResult<ControlView> {
    let session = sessions.session(owner, project, session_id)?;
    if session.binding.screen.source == "platform" {
        Stage75Adapter::new(state)
            .require_project_access(project, ProjectAccessRequirement::ActiveSession)
            .await?;
    }
    let lock = tokio::select! {
        _ = session.cancel.cancelled() => return Err(AppError::Cancelled),
        lock = tasks::submission_lock().lock() => lock,
    };
    if session.cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    validate_binding(state, &session).await?;
    ScreenRepository::new(state.local_store.pool().clone())
        .require_idle(project, &session.view.screen_id)
        .await?;
    device_idle(state, &session.view.ip).await?;
    if session
        .control
        .lock()
        .map_err(|_| AppError::Conflict("辅助操作会话不可用".into()))?
        .as_ref()
        .is_some_and(|h| !h.finished.load(Ordering::SeqCst))
    {
        return Err(AppError::Conflict(
            "辅助操作仍在进行或结束中，请稍后重试".into(),
        ));
    }
    let mut execution = plan(&session);
    if session.binding.screen.source == "platform" {
        execution.operator =
            crate::infrastructure::project_context::project_operator(state, project).await?;
    }
    let stored_plan = execution.clone();
    let task_id = uuid::Uuid::now_v7().to_string();
    let (sender, receiver) = mpsc::channel(1);
    let handle = Arc::new(ControlHandle {
        task_id: task_id.clone(),
        stop: CancellationToken::new(),
        active: AtomicBool::new(false),
        in_flight: AtomicBool::new(false),
        wake_only: AtomicBool::new(true),
        finished: AtomicBool::new(false),
        finalization_failed: AtomicBool::new(false),
        done: Notify::new(),
        sender,
        identity: Mutex::new(None),
    });
    *session
        .control
        .lock()
        .map_err(|_| AppError::Conflict("辅助操作会话不可用".into()))? = Some(handle.clone());
    let (ready_tx, ready_rx) = oneshot::channel();
    let (start_tx, start_rx) = oneshot::channel::<bool>();
    let worker_state = shared_state(state);
    let worker_session = session.clone();
    let worker_handle = handle.clone();
    let idle = sessions.control_idle_timeout;
    let spawned = state
        .job_supervisor
        .spawn(&task_id, move |cancel| async move {
            let result = if start_rx.await == Ok(true) {
                std::panic::AssertUnwindSafe(run_control(
                    worker_state.clone(),
                    worker_session,
                    worker_handle.clone(),
                    execution,
                    io,
                    receiver,
                    ready_tx,
                    cancel,
                    idle,
                ))
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(AppError::Conflict("辅助操作异常结束，请核实原任务".into()))
                })
            } else {
                Ok(())
            };
            if result.is_err() {
                worker_handle
                    .finalization_failed
                    .store(true, Ordering::SeqCst);
                let _ = mark_pending(&worker_state, &worker_handle.task_id).await;
            }
            worker_state
                .job_supervisor
                .finish(&worker_handle.task_id)
                .await;
            worker_handle.finish();
            result
        })
        .await;
    if let Err(error) = spawned {
        handle.finish();
        return Err(error);
    }
    let persisted = persist_placeholder(state, &task_id, &stored_plan).await;
    if let Err(error) = persisted {
        let _ = start_tx.send(false);
        handle.stop();
        drop(lock);
        return Err(error);
    }
    drop(lock);
    let _ = start_tx.send(true);
    match tokio::time::timeout(PREPARE_TIMEOUT, ready_rx).await {
        Ok(Ok(result)) => result,
        _ => {
            handle.stop();
            Err(AppError::timeout("建立辅助操作"))
        }
    }
}

pub async fn act(
    state: &FormalAppState,
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
    input: ControlInput,
) -> AppResult<ActionReceipt> {
    input::validate_action(&input.action)?;
    if input.frame_id.is_empty() || input.frame_id.len() > 128 {
        return Err(AppError::InvalidConfig("请先刷新屏幕".into()));
    }
    let session = sessions.session(owner, project, session_id)?;
    validate_binding(state, &session).await?;
    let handle = active_control(&session)?
        .ok_or_else(|| AppError::Conflict("辅助操作已结束，请重新启用".into()))?;
    if handle
        .in_flight
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::Conflict(
            "正在发送操作，请等待上一次操作完成".into(),
        ));
    }
    let frame = match session.begin_frame_action(&input.frame_id, &handle.task_id, &input.action) {
        Ok(frame) => frame,
        Err(error) => {
            handle.in_flight.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let (reply, receive) = oneshot::channel();
    if handle
        .sender
        .try_send(ActionEnvelope {
            input,
            frame,
            reply,
        })
        .is_err()
    {
        handle.in_flight.store(false, Ordering::SeqCst);
        return Err(AppError::Conflict("辅助操作已停止或正在收尾".into()));
    }
    match tokio::time::timeout(Duration::from_secs(45), receive).await {
        Ok(Ok(result)) => result,
        _ => {
            handle.stop();
            Ok(ActionReceipt {
                status: "unconfirmed".into(),
                message: "操作结果暂未确认，已停止后续输入，请刷新屏幕核对".into(),
                control_active: false,
                input_generation: session.input_generation(),
                control_mode: handle.control_mode(),
            })
        }
    }
}
pub async fn end_control(
    sessions: &RemoteScreenSessions,
    owner: &str,
    project: &str,
    session_id: &str,
) -> AppResult<()> {
    let session = match sessions.session(owner, project, session_id) {
        Ok(s) => s,
        Err(AppError::NotFound(_)) => return Ok(()),
        Err(e) => return Err(e),
    };
    let handle = session
        .control
        .lock()
        .map_err(|_| AppError::Conflict("辅助操作会话不可用".into()))?
        .clone();
    let Some(handle) = handle else { return Ok(()) };
    handle.stop();
    tokio::time::timeout(Duration::from_secs(35), async {
        loop {
            let done = handle.done.notified();
            if handle.finished.load(Ordering::SeqCst) {
                break;
            }
            done.await;
        }
    })
    .await
    .map_err(|_| AppError::timeout("结束辅助操作，正在等待已发送操作收尾"))?;
    if handle.finalization_failed.load(Ordering::SeqCst) {
        return Err(AppError::Conflict(
            "输入已停止，但结果保存或占用释放尚未确认，请核实原任务".into(),
        ));
    }
    Ok(())
}

async fn mark_pending(state: &FormalAppState, id: &str) -> AppResult<()> {
    let guard = state
        .task_repository
        .protect_results(id, "辅助操作已停止，结果保存或占用释放待核实")
        .await;
    // guard在建会话事务中已存在；再次INSERT失败不能阻止独立收敛任务状态。
    let transition = async {
        let mut current = state.task_repository.get(id).await?.state;
        if current == TaskState::Queued {
            state
                .task_repository
                .transition(id, current, TaskState::Running, None, None)
                .await?;
            current = TaskState::Running;
        }
        if matches!(
            current,
            TaskState::Running | TaskState::Cancelling | TaskState::Interrupted
        ) {
            state
                .task_repository
                .transition(
                    id,
                    current,
                    TaskState::FinalizingFailed,
                    Some("REMOTE_ASSISTANCE_FINALIZATION_PENDING"),
                    Some("辅助操作已停止，结果待核实；不会重新发送输入"),
                )
                .await?;
        }
        Ok::<(), AppError>(())
    }
    .await;
    let current = state.task_repository.get(id).await?;
    if state.task_repository.results_protected(id).await?
        && (current.state == TaskState::FinalizingFailed || current.state.is_terminal())
    {
        return Ok(());
    }
    transition?;
    guard
}

/// 只识别本模块持久guard仍在、且已经没有任何受管执行器的活动记录。
/// 重新读取当前行，避免旧Running快照把刚正常结束的会话重新判成待核实。
pub async fn orphan_needs_review(state: &FormalAppState, task: &TaskRecord) -> AppResult<bool> {
    if task.domain_type != "smart_screen"
        || task.operation_type != OPERATION
        || !matches!(
            task.state,
            TaskState::Queued | TaskState::Running | TaskState::Cancelling
        )
        || state.task_queue.is_tracked(&task.id).await
    {
        return Ok(false);
    }
    let current = match state.task_repository.get(&task.id).await {
        Ok(value) => value,
        Err(AppError::NotFound(_)) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(current.domain_type == "smart_screen"
        && current.operation_type == OPERATION
        && current.local_project_id == task.local_project_id
        && matches!(
            current.state,
            TaskState::Queued | TaskState::Running | TaskState::Cancelling
        )
        && state.task_repository.results_protected(&task.id).await?)
}

pub async fn converge_orphan(state: &FormalAppState, task: &TaskRecord) -> AppResult<()> {
    if orphan_needs_review(state, task).await? {
        mark_pending(state, &task.id).await?;
    }
    Ok(())
}

struct SharedControl {
    context: ScreenWriteContext,
    held: Option<HeldScreenLeases>,
}
async fn prepare_shared(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    slot: &mut Option<SharedControl>,
) -> AppResult<()> {
    if plan.targets[0].source != "platform" {
        return Ok(());
    }
    let context = write_context::open(state, &plan.project_id).await?;
    context.validate_plan(plan)?;
    // 在可能创建共享记录/租约之前保留上下文。超时未取得held不能当作未取得占用。
    *slot = Some(SharedControl {
        context,
        held: None,
    });
    let shared = slot
        .as_mut()
        .expect("context stored before lease preparation");
    let context = &shared.context;
    let target = format!("{}:{}", context.business, plan.targets[0].id);
    ScreenSharedResults::new(context.shared.clone())
        .start(&SharedScreenOperation {
            id,
            business_project_id: &context.business,
            action: OPERATION,
            name: "远程辅助操作",
            operator: &context.operator,
            instance_id: &plan.instance_id,
            targets: &[target],
            started_at: Some(&plan.created_at),
        })
        .await?;
    state.task_repository.link_operation(id, id).await?;
    let held = HeldScreenLeases::acquire(
        context,
        id,
        &plan.instance_id,
        &[plan.targets[0].id.clone()],
        false,
        false,
    )
    .await?;
    shared.held = Some(held);
    Ok(())
}

async fn release_matching_lease(
    context: &ScreenWriteContext,
    id: &str,
    plan: &ScreenPlan,
) -> AppResult<()> {
    context.validate_plan(plan)?;
    let rows=sqlx::query("SELECT resource_type,resource_key,operation_id,owner_instance_id,lease_token,fencing_token FROM resource_lease WHERE operation_id=? AND owner_instance_id=? AND resource_type='smart_screen' AND resource_key=? AND lease_state='active'")
        .bind(id).bind(&plan.instance_id).bind(format!("{}:{}",context.business,plan.targets[0].id)).fetch_all(&context.shared).await.map_err(|e|AppError::database("核对辅助操作旧占用",&e))?;
    let repository = ResourceLeaseRepository::new(context.shared.clone());
    for row in rows {
        let grant = LeaseGrant {
            resource_type: row
                .try_get("resource_type")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
            resource_key: row
                .try_get("resource_key")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
            operation_id: row
                .try_get("operation_id")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
            owner_instance_id: row
                .try_get("owner_instance_id")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
            lease_token: row
                .try_get("lease_token")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
            fencing_token: row
                .try_get("fencing_token")
                .map_err(|e| AppError::database("核对辅助操作占用", &e))?,
        };
        repository.release(&grant).await.map_err(map_formal_error)?;
    }
    Ok(())
}
async fn validate_shared(
    state: &FormalAppState,
    plan: &ScreenPlan,
    shared: Option<&SharedControl>,
) -> AppResult<()> {
    if let Some(shared) = shared {
        Stage75Adapter::new(state)
            .require_project_access(&plan.project_id, ProjectAccessRequirement::ActiveSession)
            .await?;
        shared
            .held
            .as_ref()
            .ok_or_else(|| AppError::Conflict("辅助操作占用已释放".into()))?
            .valid()
            .await?;
        let current = platform_write::record(&shared.context.read, &plan.targets[0].id)
            .await?
            .ok_or_else(|| AppError::Conflict("平台屏记录已移除".into()))?;
        if current.business.as_deref() != Some(shared.context.business.as_str())
            || !crate::application::smart_screen::operations::same_target(
                &plan.targets[0],
                &current.asset,
            )
        {
            return Err(AppError::Conflict("平台屏资料已改变，请刷新资料".into()));
        }
    }
    Ok(())
}

async fn run_control(
    state: Arc<FormalAppState>,
    session: Arc<Session>,
    handle: Arc<ControlHandle>,
    plan: ScreenPlan,
    io: Arc<dyn ControlPort>,
    mut receiver: mpsc::Receiver<ActionEnvelope>,
    ready: oneshot::Sender<AppResult<ControlView>>,
    cancel: CancellationToken,
    idle: Duration,
) -> AppResult<()> {
    let mut shared = None;
    let mut count = 0u64;
    let mut uncertain = false;
    let prepare = tokio::time::timeout(PREPARE_TIMEOUT, async {
        if session.cancel.is_cancelled() || handle.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        state
            .task_repository
            .transition(
                &handle.task_id,
                TaskState::Queued,
                TaskState::Running,
                None,
                Some("正在准备远程辅助操作"),
            )
            .await?;
        prepare_shared(&state, &handle.task_id, &plan, &mut shared).await?;
        validate_binding(&state, &session).await?;
        validate_shared(&state, &plan, shared.as_ref()).await?;
        let facts = tokio::time::timeout(
            Duration::from_secs(20),
            io.probe(&session.view.ip, session.cancel.clone()),
        )
        .await
        .map_err(|_| AppError::timeout("核对辅助操作设备"))??;
        validate_device(&session.binding.screen, None, &facts)?;
        let mode = input::initial_control_mode(&facts)?;
        handle
            .wake_only
            .store(mode == "wake_only", Ordering::SeqCst);
        *handle
            .identity
            .lock()
            .map_err(|_| AppError::Conflict("辅助操作身份不可用".into()))? = Some(facts);
        session.frame_history()?.advance();
        if session.cancel.is_cancelled() || handle.stop.is_cancelled() || cancel.is_cancelled() {
            return Err(AppError::Cancelled);
        }
        state
            .task_repository
            .update_target(
                &handle.task_id,
                target_update(
                    &session.view.screen_id,
                    TargetState::Running,
                    if mode == "wake_only" {
                        "屏幕已休眠，仅可执行唤醒"
                    } else {
                        "远程辅助操作已就绪"
                    },
                    0,
                ),
            )
            .await?;
        Ok::<(), AppError>(())
    })
    .await
    .unwrap_or_else(|_| Err(AppError::timeout("准备辅助操作")));
    let reason = match prepare {
        Err(error) => {
            let _ = ready.send(Err(error));
            "辅助操作未建立，未发送输入"
        }
        Ok(()) => {
            handle.active.store(true, Ordering::SeqCst);
            let text_input_behavior = handle
                .identity
                .lock()
                .ok()
                .and_then(|facts| {
                    facts
                        .as_ref()
                        .and_then(|facts| input::text_input_behavior(facts.android_sdk).ok())
                })
                .ok_or_else(|| AppError::Conflict("辅助操作系统版本不可用".into()))?;
            let _ = ready.send(Ok(ControlView {
                task_id: handle.task_id.clone(),
                idle_timeout_ms: idle.as_millis().min(u128::from(u64::MAX)) as u64,
                frame_max_age_ms: input::FRAME_MAX_AGE.as_millis() as u64,
                max_text_length: input::MAX_TEXT_LENGTH,
                text_input_behavior,
                input_generation: session.input_generation(),
                control_mode: handle.control_mode(),
            }));
            let mut last_input = tokio::time::Instant::now();
            let lease_lost = shared
                .as_ref()
                .and_then(|s| s.held.as_ref())
                .map(|h| h.lost.clone())
                .unwrap_or_default();
            loop {
                let request = tokio::select! {
                    biased;
                    _=session.cancel.cancelled()=>break "窗口已关闭，辅助操作已结束",
                    _=handle.stop.cancelled()=>break "辅助操作已结束",
                    _=cancel.cancelled()=>break "辅助操作已取消",
                    _=lease_lost.cancelled()=>break "设备占用已失效，辅助操作已停止",
                    _=tokio::time::sleep_until(last_input+idle)=>break "辅助操作因闲置已结束",
                    request=receiver.recv()=>match request{Some(value)=>value,None=>break "辅助操作已结束"},
                };
                let result = tokio::time::timeout(
                    Duration::from_secs(30),
                    perform_action(
                        &state,
                        &session,
                        &handle,
                        &plan,
                        shared.as_ref(),
                        &*io,
                        &request.input,
                        &request.frame.stamp,
                        &cancel,
                    ),
                )
                .await
                .unwrap_or(Err(ActionFailure::Unconfirmed));
                drop(request.frame);
                handle.in_flight.store(false, Ordering::SeqCst);
                match result {
                    Ok(message) => {
                        count += 1;
                        last_input = tokio::time::Instant::now();
                        let _ = request.reply.send(Ok(ActionReceipt {
                            status: "sent".into(),
                            message: message.into(),
                            control_active: !handle.stop.is_cancelled()
                                && !cancel.is_cancelled()
                                && !session.cancel.is_cancelled(),
                            input_generation: session.input_generation(),
                            control_mode: handle.control_mode(),
                        }));
                    }
                    Err(ActionFailure::Rejected(error)) => {
                        let _ = request.reply.send(Err(error));
                    }
                    Err(ActionFailure::Unconfirmed) => {
                        uncertain = true;
                        handle.stop();
                        let _ = request.reply.send(Ok(ActionReceipt {
                            status: "unconfirmed".into(),
                            message: "最后一次操作结果未确认，辅助操作已停止，请刷新屏幕核对"
                                .into(),
                            control_active: false,
                            input_generation: session.input_generation(),
                            control_mode: handle.control_mode(),
                        }));
                        break "最后一次输入结果未确认，已停止辅助操作";
                    }
                }
            }
        }
    };
    handle.active.store(false, Ordering::SeqCst);
    receiver.close();
    while let Ok(request) = receiver.try_recv() {
        let _ = request.reply.send(Err(AppError::Cancelled));
    }
    let finalized = tokio::time::timeout(
        Duration::from_secs(20),
        finalize(&state, &handle.task_id, &plan, count, uncertain, reason),
    )
    .await
    .unwrap_or_else(|_| Err(AppError::timeout("保存辅助操作结果")));
    let released = if let Some(shared) = &mut shared {
        if let Some(held) = shared.held.take() {
            tokio::time::timeout(Duration::from_secs(10), held.release())
                .await
                .unwrap_or_else(|_| Err(AppError::timeout("释放辅助操作占用")))
        } else {
            tokio::time::timeout(
                Duration::from_secs(10),
                release_matching_lease(&shared.context, &handle.task_id, &plan),
            )
            .await
            .unwrap_or_else(|_| Err(AppError::timeout("核实并释放辅助操作占用")))
        }
    } else {
        Ok(())
    };
    if finalized.is_ok() && released.is_ok() {
        state
            .task_repository
            .resolve_results(&handle.task_id)
            .await?;
    }
    if released.is_err() {
        state
            .task_repository
            .protect_results(&handle.task_id, "辅助操作占用释放尚未确认")
            .await?;
    }
    finalized.and(released)
}

enum ActionFailure {
    Rejected(AppError),
    Unconfirmed,
}
async fn perform_action(
    state: &FormalAppState,
    session: &Session,
    handle: &ControlHandle,
    plan: &ScreenPlan,
    shared: Option<&SharedControl>,
    io: &dyn ControlPort,
    input: &ControlInput,
    frame: &FrameStamp,
    cancel: &CancellationToken,
) -> Result<&'static str, ActionFailure> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(29);
    let reject = ActionFailure::Rejected;
    if handle.stop.is_cancelled() || session.cancel.is_cancelled() || cancel.is_cancelled() {
        return Err(reject(AppError::Cancelled));
    }
    validate_binding(state, session).await.map_err(reject)?;
    validate_shared(state, plan, shared).await.map_err(reject)?;
    if frame.captured.elapsed() > input::FRAME_MAX_AGE {
        return Err(reject(AppError::Conflict(
            "画面已过期，请刷新后再操作".into(),
        )));
    }
    let facts = tokio::time::timeout(
        Duration::from_secs(15),
        io.probe(&session.view.ip, session.cancel.clone()),
    )
    .await
    .map_err(|_| reject(AppError::timeout("核对远程输入设备")))?
    .map_err(|_| {
        reject(AppError::Conflict(
            "无法确认设备状态，未发送输入，请刷新后重试".into(),
        ))
    })?;
    {
        let identity = handle
            .identity
            .lock()
            .map_err(|_| reject(AppError::Conflict("辅助操作身份不可用".into())))?;
        if let Err(error) = validate_device(&session.binding.screen, identity.as_ref(), &facts) {
            handle.stop();
            return Err(reject(error));
        }
    }
    if facts.width != frame.width || facts.height != frame.height || facts.serial != frame.serial {
        handle.stop();
        return Err(reject(AppError::Conflict(
            "设备画面尺寸或身份已变化，请刷新后再操作".into(),
        )));
    }
    let wake = matches!(
        input.action,
        input::ControlAction::Key {
            key: input::ControlKey::WakeUp
        }
    );
    if facts.awake.is_none() {
        return Err(reject(AppError::Conflict(
            "无法确认屏幕电源状态，未发送输入".into(),
        )));
    }
    if facts.awake == Some(false) {
        handle.wake_only.store(true, Ordering::SeqCst);
    }
    if !wake
        && (handle.wake_only.load(Ordering::SeqCst)
            || facts.awake != Some(true)
            || facts.foreground_package != super::super::device::XIAOXIN_PACKAGE)
    {
        return Err(reject(AppError::Conflict(
            "屏幕尚未处于小新可操作状态，请先唤醒后重新读取".into(),
        )));
    }
    if handle.stop.is_cancelled() || session.cancel.is_cancelled() || cancel.is_cancelled() {
        return Err(reject(AppError::Cancelled));
    }
    input::validate_action_for_sdk(&input.action, facts.android_sdk).map_err(reject)?;
    validate_shared(state, plan, shared).await.map_err(reject)?;
    if frame.captured.elapsed() > input::FRAME_MAX_AGE {
        return Err(reject(AppError::Conflict(
            "画面已过期，未发送输入，请刷新后重试".into(),
        )));
    }
    // 已发出的短命令不因关闭窗口截断/重放；等待有限时间收敛后再释放占用。
    match tokio::time::timeout_at(
        deadline,
        io.send(
            &session.view.ip,
            &input.action,
            input::ControlSendContext {
                android_sdk: facts.android_sdk,
                deadline,
            },
            CancellationToken::new(),
        ),
    )
    .await
    {
        Ok(Ok(())) if wake => {
            // 唤醒只发一次；系统回到小新前台后才开放普通输入，整个只读核实仍在原动作期限内。
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let verification = async {
                loop {
                    let current = io.probe(&session.view.ip, CancellationToken::new()).await?;
                    validate_device(&session.binding.screen, Some(&facts), &current)?;
                    if current.awake == Some(true)
                        && current.foreground_package == super::super::device::XIAOXIN_PACKAGE
                    {
                        return Ok::<(), AppError>(());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            };
            let confirmed = matches!(
                tokio::time::timeout(remaining.min(Duration::from_secs(6)), verification).await,
                Ok(Ok(()))
            );
            if confirmed {
                handle.wake_only.store(false, Ordering::SeqCst);
                Ok("屏幕已唤醒，刷新画面后可继续操作")
            } else {
                handle.wake_only.store(true, Ordering::SeqCst);
                handle.stop();
                Ok("唤醒已发送，请待画面恢复后重新开启辅助操作")
            }
        }
        Ok(Ok(())) => Ok("操作已发送，请查看屏端画面确认结果"),
        Ok(Err(error @ AppError::InvalidConfig(_))) => Err(ActionFailure::Rejected(error)),
        _ => Err(ActionFailure::Unconfirmed),
    }
}

fn target_update(screen: &str, state: TargetState, message: &str, _count: u64) -> TargetUpdate {
    TargetUpdate {
        resource_type: "smart_screen".into(),
        resource_key: screen.into(),
        state,
        stage: message.into(),
        progress_current: if matches!(
            state,
            TargetState::Succeeded | TargetState::Failed | TargetState::Cancelled
        ) {
            1
        } else {
            0
        },
        progress_total: 1,
        fencing_token: None,
        message_code: Some("REMOTE_ASSISTANCE".into()),
        message_params_json: None,
    }
}
async fn finalize(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    count: u64,
    uncertain: bool,
    reason: &str,
) -> AppResult<()> {
    let original_state = state.task_repository.get(id).await?.state;
    let message = format!("{reason}；已确认发送 {count} 次操作。登录结果以屏端页面为准");
    let mut result = ScreenTargetResult {
        format_version: 1,
        screen_id: plan.targets[0].id.clone(),
        device: if uncertain {
            ResultState::Failed
        } else {
            ResultState::Succeeded
        },
        business: ResultState::NotRequired,
        shared: if plan.targets[0].source == "platform" {
            ResultState::Pending
        } else {
            ResultState::NotRequired
        },
        message: message.clone(),
        evidence: json!({"remoteAssistance":{"actionCount":count,"lastInputUnconfirmed":uncertain}}),
        ..Default::default()
    };
    if reason == "辅助操作未建立，未发送输入" || reason == "设备占用已失效，辅助操作已停止"
    {
        result.device = ResultState::Failed;
    }
    if original_state == TaskState::Cancelled || reason == "辅助操作已取消" {
        result.device = ResultState::Cancelled;
    }
    let mut results = ScreenResults {
        targets: BTreeMap::from([(result.screen_id.clone(), result)]),
        finished: true,
    };
    complete_results(state, id, plan, &mut results).await
}

async fn complete_results(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    results: &mut ScreenResults,
) -> AppResult<()> {
    let original_state = state.task_repository.get(id).await?.state;
    let target = results
        .targets
        .get(&plan.targets[0].id)
        .ok_or_else(|| AppError::Conflict("辅助操作缺少安全摘要".into()))?;
    let message = target.message.clone();
    let count = target.evidence["remoteAssistance"]["actionCount"]
        .as_u64()
        .unwrap_or(0);
    let desired = match target.device {
        ResultState::Succeeded => TaskState::Succeeded,
        ResultState::Cancelled => TaskState::Cancelled,
        _ => TaskState::Failed,
    };
    task_data::save_results(state.local_store.pool(), &plan.project_id, id, &results).await?;
    let shared = tasks::save_shared(state, id, plan, results).await;
    task_data::save_results(state.local_store.pool(), &plan.project_id, id, &results).await?;
    let next = if original_state.is_terminal() && original_state != TaskState::Interrupted {
        original_state
    } else if shared.is_err() {
        TaskState::FinalizingFailed
    } else {
        desired
    };
    let current = state.task_repository.get(id).await?.state;
    if current == TaskState::Queued {
        state
            .task_repository
            .transition(id, current, TaskState::Running, None, None)
            .await?;
    }
    if current == TaskState::Interrupted {
        state
            .task_repository
            .transition(id, current, TaskState::FinalizingFailed, None, None)
            .await?;
    }
    let current = state.task_repository.get(id).await?.state;
    if current == TaskState::Running && next == TaskState::Cancelled {
        state
            .task_repository
            .transition(id, current, TaskState::Cancelling, None, None)
            .await?;
    }
    let current = state.task_repository.get(id).await?.state;
    if current != next {
        state
            .task_repository
            .transition(id, current, next, None, Some(&message))
            .await?;
    }
    state
        .task_repository
        .update_target(
            id,
            target_update(
                &plan.targets[0].id,
                if next == TaskState::Succeeded {
                    TargetState::Succeeded
                } else if next == TaskState::Cancelled {
                    TargetState::Cancelled
                } else if next == TaskState::FinalizingFailed {
                    TargetState::Pending
                } else {
                    TargetState::Failed
                },
                &message,
                count,
            ),
        )
        .await?;
    if !original_state.is_terminal() {
        let _ = tasks::event(
            state,
            id,
            None,
            "completed",
            &message,
            if next == TaskState::Succeeded {
                TaskEventLevel::Info
            } else {
                TaskEventLevel::Warn
            },
        )
        .await;
    }
    shared
}

/// 只补存会话已停止的摘要与释放本次占用，不重新发送任何输入。
pub fn recover<'a>(
    state: &'a FormalAppState,
    task: &'a TaskRecord,
    _force: bool,
) -> crate::infrastructure::task_recovery::TaskRecoveryFuture<'a> {
    Box::pin(async move {
        if state.job_supervisor.contains(&task.id).await {
            return Err(AppError::Conflict("辅助操作仍在运行，请先结束".into()));
        }
        let (plan, _) =
            task_data::read_plan(state.local_store.pool(), &task.local_project_id, &task.id)
                .await?;
        if plan.input.action != OPERATION || plan.targets.len() != 1 {
            return Err(AppError::Conflict("不是可核实的辅助操作记录".into()));
        }
        let mut saved =
            task_data::read_results(state.local_store.pool(), &plan.project_id, &task.id).await?;
        if !saved.finished || !saved.targets.contains_key(&plan.targets[0].id) {
            let result=ScreenTargetResult{format_version:1,screen_id:plan.targets[0].id.clone(),device:ResultState::Failed,business:ResultState::NotRequired,
                shared:if plan.targets[0].source=="platform"{ResultState::Pending}else{ResultState::NotRequired},
                message:"上次辅助操作已中断，已停止发送；历史输入次数和结果未确认，请查看屏端核对。未重新发送输入".into(),
                evidence:json!({"remoteAssistance":{"actionCount":null,"lastInputUnconfirmed":true}}),..Default::default()};
            saved = ScreenResults {
                targets: BTreeMap::from([(result.screen_id.clone(), result)]),
                finished: true,
            };
        }
        // 已存摘要必须原样重用，共享结果可能已经确认，不能改写文案再覆盖。
        let result = complete_results(state, &task.id, &plan, &mut saved).await;
        if plan.targets[0].source == "platform" {
            let context = write_context::open(state, &plan.project_id).await?;
            context.validate_plan(&plan)?;
            tokio::time::timeout(
                Duration::from_secs(10),
                release_matching_lease(&context, &task.id, &plan),
            )
            .await
            .map_err(|_| AppError::timeout("核实并释放辅助操作旧占用"))??;
        }
        result?;
        state.task_repository.resolve_results(&task.id).await?;
        Ok(crate::infrastructure::task_recovery::TaskRecoveryOutcome::Completed)
    })
}
