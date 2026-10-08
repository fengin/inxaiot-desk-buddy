use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    device::{AdbDevice, AndroidTools, ping},
    shared_results::{ScreenSharedResults, SharedScreenOperation},
    task_data,
};
use crate::application::{
    ports::{
        project_access::ProjectAccessPort,
        task_event::{TaskEventInput, TaskEventSink},
    },
    project_access::ProjectAccessRequirement,
    smart_screen::operations::{same_target, validate_input},
};
use crate::core::error::{AppError, AppResult};
use crate::domain::{
    common::task::{TargetState, TaskEventLevel, TaskRecord, TaskState},
    smart_screen::{model::*, operation::*},
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::{
    client_instance::application_instance_id,
    local_sqlite::{
        screen_repository::{ScreenRepository, now},
        task_repository::{CreateTask, TargetUpdate},
    },
    project_context::{map_formal_error, project_database_for_finalization, project_operator},
    stage75_adapter::Stage75Adapter,
};
use crate::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry};

pub const DOMAIN: &str = "smart_screen";
pub const PREFLIGHT: &str = "screen_preflight";
pub fn submission_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}
fn local(state: &FormalAppState) -> ScreenRepository {
    ScreenRepository::new(state.local_store.pool().clone())
}

pub async fn preflight(
    state: &FormalAppState,
    project: &str,
    input: ScreenOperationInput,
) -> AppResult<ScreenPreflight> {
    validate_retry(state, project, &input).await?;
    if WRITE_ACTIONS.contains(&input.action.as_str()) {
        return super::maintenance::preflight(state, project, input).await;
    }
    validate_input(&input)?;
    Stage75Adapter::new(state)
        .require_project_access(project, ProjectAccessRequirement::Configured)
        .await?;
    let snapshot = local(state).snapshot(project).await?;
    let mut items = Vec::new();
    let mut targets = Vec::new();
    for id in &input.target_ids {
        let Some(screen) = snapshot.screens.iter().find(|s| &s.id == id) else {
            items.push(ScreenPreflightItem {
                screen_id: id.clone(),
                name: "已移除的屏".into(),
                ip: String::new(),
                state: "blocked".into(),
                reason: "屏不属于当前项目或已移除".into(),
                observation: None,
            });
            continue;
        };
        let reason = if screen.fields.ip.parse::<std::net::Ipv4Addr>().is_err() {
            Some("设备 IP 无效，请先修正资料".into())
        } else if let Err(error) = local(state).require_idle(project, id).await {
            Some(error.to_string())
        } else {
            None
        };
        items.push(ScreenPreflightItem {
            screen_id: id.clone(),
            name: screen.fields.name.clone(),
            ip: screen.fields.ip.clone(),
            state: if reason.is_some() { "blocked" } else { "ready" }.into(),
            observation: None,
            reason: reason
                .clone()
                .unwrap_or_else(|| "可以按当前确认地址执行只读检查".into()),
        });
        if reason.is_none() {
            targets.push(screen.clone());
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    if targets.is_empty() {
        return Ok(ScreenPreflight { id, items });
    }
    let scope = local(state).scope(project).await?;
    let operator = project_operator(state, project)
        .await
        .unwrap_or_else(|_| "本机操作（未登录平台）".into());
    let plan = ScreenPlan {
        project_id: project.into(),
        input,
        targets,
        business_project_id: scope.as_ref().map(|s| s.0.clone()),
        data_source_id: scope.map(|s| s.1),
        operator,
        instance_id: application_instance_id().into(),
        created_at: now(),
        detail: serde_json::Value::Null,
    };
    let hash = task_data::save_plan(state.local_store.pool(), &id, &plan).await?;
    let mut create = create_task(state, &id, &plan, PREFLIGHT)?;
    create
        .targets
        .push(("preflight_internal".into(), "common".into()));
    state.task_repository.create(create).await?;
    state
        .task_repository
        .transition(&id, TaskState::Draft, TaskState::Checking, None, None)
        .await?;
    for (kind, key) in plan
        .targets
        .iter()
        .map(|s| (DOMAIN.to_string(), s.id.clone()))
        .chain(std::iter::once((
            "preflight_internal".into(),
            "common".into(),
        )))
    {
        state
            .task_repository
            .update_target(
                &id,
                TargetUpdate {
                    resource_type: kind,
                    resource_key: key,
                    state: TargetState::Succeeded,
                    stage: "检查完成".into(),
                    progress_current: 1,
                    progress_total: 1,
                    fencing_token: None,
                    message_code: Some("PREFLIGHT_TARGET_PASSED".into()),
                    message_params_json: None,
                },
            )
            .await?;
    }
    state
        .task_repository
        .bind_preflight_snapshot(&id, project, PREFLIGHT, &hash)
        .await?;
    state
        .task_repository
        .transition(&id, TaskState::Checking, TaskState::Succeeded, None, None)
        .await?;
    Ok(ScreenPreflight { id, items })
}

pub async fn validate_retry(state: &FormalAppState, project: &str, input: &ScreenOperationInput) -> AppResult<()> {
    let Some(id) = &input.retry_of_operation_id else { return Ok(()); };
    let task = state.task_repository.get(id).await?;
    let (previous, _) = task_data::read_plan(state.local_store.pool(), project, id).await?;
    if task.local_project_id != project || task.domain_type != DOMAIN || !task.state.is_terminal() || state.task_repository.results_protected(id).await?
        || previous.input.action != input.action || input.target_ids.iter().any(|id| !previous.targets.iter().any(|s| &s.id == id || s.aliases.contains(id))) {
        return Err(AppError::Conflict("重试来源必须是本项目已结束的同类操作，且只能选择原操作中的屏".into()));
    }
    Ok(())
}

pub async fn submit(
    state: &FormalAppState,
    project: &str,
    preflight_id: &str,
    input: ScreenOperationInput,
) -> AppResult<String> {
    if WRITE_ACTIONS.contains(&input.action.as_str()) {
        return super::maintenance::submit(state, project, preflight_id, input).await;
    }
    let _guard = submission_lock().lock().await;
    validate_input(&input)?;
    Stage75Adapter::new(state)
        .require_project_access(project, ProjectAccessRequirement::Configured)
        .await?;
    let (mut plan, hash) =
        task_data::read_plan(state.local_store.pool(), project, preflight_id).await?;
    let mut comparable = input.clone();
    comparable.target_ids = plan.input.target_ids.clone();
    comparable.expected_targets = plan.input.expected_targets.clone();
    if comparable != plan.input {
        return Err(AppError::Conflict(
            "操作参数与检查时不同，请重新检查".into(),
        ));
    }
    let ids: BTreeSet<_> = input.target_ids.iter().cloned().collect();
    if !ids.is_subset(&plan.targets.iter().map(|s| s.id.clone()).collect()) {
        return Err(AppError::Conflict("只能执行已检查通过的目标".into()));
    }
    plan.targets.retain(|s| ids.contains(&s.id));
    for screen in &plan.targets {
        local(state).require_idle(project, &screen.id).await?;
        let current = local(state).asset(project, &screen.id).await?;
        if !same_target(screen, &current) {
            return Err(AppError::Conflict(
                "设备地址、身份或尺寸已变化，请重新检查".into(),
            ));
        }
    }
    plan.input.target_ids = input.target_ids;
    let id = uuid::Uuid::now_v7().to_string();
    task_data::save_plan(state.local_store.pool(), &id, &plan).await?;
    let create = create_task(state, &id, &plan, &plan.input.action)?;
    state
        .task_repository
        .create_queued_from_preflight_selection(preflight_id, PREFLIGHT, &hash, create)
        .await?;
    let envelope = TaskEnvelope {
        local_task_id: id.clone(),
        local_project_id: project.into(),
        domain_type: DOMAIN.into(),
        operation_type: plan.input.action.clone(),
        resource_keys: plan
            .targets
            .iter()
            .map(|s| format!("{project}:{}", s.id))
            .collect(),
        priority: 0,
        payload_ref: None,
        payload_sha256: None,
    };
    if let Err(error) = state.task_queue.enqueue(envelope).await {
        state
            .task_repository
            .transition(
                &id,
                TaskState::Queued,
                TaskState::Cancelled,
                Some("SCREEN_QUEUE_REJECTED"),
                Some("任务未进入队列，未执行设备动作"),
            )
            .await?;
        return Err(error);
    }
    Ok(id)
}

pub(crate) fn create_task(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    operation: &str,
) -> AppResult<CreateTask> {
    Ok(CreateTask {
        id: id.into(),
        local_project_id: plan.project_id.clone(),
        remote_operation_record_id: None,
        domain_type: DOMAIN.into(),
        operation_type: operation.into(),
        name: action_label(&plan.input.action).into(),
        priority: 0,
        batch_size: plan.targets.len() as u32,
        concurrency: plan.input.concurrency.min(plan.targets.len() as u32),
        payload_ref: None,
        log_path: state
            .paths
            .project_task_log_path(&plan.project_id, id)
            .map_err(map_formal_error)?
            .to_string_lossy()
            .into_owned(),
        targets: plan
            .targets
            .iter()
            .map(|s| (DOMAIN.into(), s.id.clone()))
            .collect(),
    })
}

pub fn register(registry: &TaskHandlerRegistry, app: AppHandle) -> AppResult<()> {
    for action in READ_ACTIONS {
        let app = app.clone();
        registry.register(DOMAIN, action, move |envelope, cancel| {
            let app = app.clone();
            async move { run(&app.state::<FormalAppState>(), envelope, cancel).await }
        })?;
    }
    Ok(())
}

pub async fn run(
    state: &FormalAppState,
    envelope: TaskEnvelope,
    cancel: CancellationToken,
) -> AppResult<()> {
    let id = &envelope.local_task_id;
    let project = &envelope.local_project_id;
    let (plan, _) = task_data::read_plan(state.local_store.pool(), project, id).await?;
    if plan.input.action != envelope.operation_type || envelope.domain_type != DOMAIN {
        return Err(AppError::Conflict("任务业务或操作类型不符".into()));
    }
    state
        .task_repository
        .transition(id, TaskState::Queued, TaskState::Running, None, None)
        .await?;
    if plan.targets.iter().any(|s| s.source == "platform") {
        state
            .task_repository
            .protect_results(id, "已注册屏的检查结果尚未全部保存")
            .await?;
    }
    event(
        state,
        id,
        None,
        "running",
        "开始设备检查",
        TaskEventLevel::Info,
    )
    .await?;
    let mut results = ScreenResults {
        targets: BTreeMap::new(),
        finished: false,
    };
    for screen in &plan.targets {
        results.targets.insert(
            screen.id.clone(),
            ScreenTargetResult {
                format_version: 1,
                screen_id: screen.id.clone(),
                device: ResultState::Pending,
                business: ResultState::NotRequired,
                shared: if screen.source == "platform" {
                    ResultState::Pending
                } else {
                    ResultState::NotRequired
                },
                ..Default::default()
            },
        );
    }
    task_data::save_results(state.local_store.pool(), project, id, &results).await?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(plan.input.concurrency as usize));
    let mut workers = tokio::task::JoinSet::new();
    for screen in plan.targets.clone() {
        let semaphore = semaphore.clone();
        let token = cancel.clone();
        let action = plan.input.action.clone();
        let repo = local(state);
        let project = project.clone();
        let task_repo = state.task_repository.clone();
        let task_id = id.clone();
        let pipeline = state.task_event_pipeline.clone();
        workers.spawn(async move {
            let permit=tokio::select!{_=token.cancelled()=>return (screen,Err(AppError::Cancelled)),p=semaphore.acquire_owned()=>p};
            let Ok(_permit)=permit else{return (screen,Err(AppError::Cancelled));};
            match repo.asset(&project,&screen.id).await {
                Ok(current) if same_target(&screen,&current)=>{},
                Ok(_)=>return (screen,Err(AppError::Conflict("设备关键资料已改变，本次未执行".into()))),
                Err(error)=>return (screen,Err(error))
            }
            if let Err(error)=task_repo.update_target(&task_id,TargetUpdate{resource_type:DOMAIN.into(),resource_key:screen.id.clone(),state:TargetState::Running,stage:"读取设备".into(),progress_current:0,progress_total:1,fencing_token:None,message_code:Some("SCREEN_READING".into()),message_params_json:None}).await{return (screen,Err(error));}
            let _=pipeline.emit(&task_id,TaskEventInput{resource_type:Some(DOMAIN.into()),resource_key:Some(screen.id.clone()),stage:"screen_operation".into(),status:"running".into(),progress_current:Some(0),progress_total:Some(1),level:TaskEventLevel::Info,message_code:"SCREEN_READING".into(),message_params:BTreeMap::new(),message:Some(format!("正在读取 {}（{}）",screen.fields.name,screen.fields.ip))}).await;
            let output=if action=="ping"{
                match ping(&screen.fields.ip,token.clone()).await{
                    Ok(online)=>Ok(ScreenObservation{id:uuid::Uuid::now_v7().to_string(),screen_id:screen.id.clone(),operation_type:action.clone(),observed_ip:screen.fields.ip.clone(),observed_at:now(),ping:Some(if online{"online"}else{"offline"}.into()),..Default::default()}),
                    Err(error)=>Err(error)
                }
            }else{match AndroidTools::discover(){Ok(tools)=>AdbDevice::new(tools).inspect(&screen,&action,token).await,Err(error)=>Err(error)}};
            (screen,output)
        });
    }
    while let Some(joined) = workers.join_next().await {
        let (screen, output) =
            joined.map_err(|_| AppError::Conflict("设备检查任务异常结束".into()))?;
        let mut result = results.targets.remove(&screen.id).expect("planned target");
        let (target_state, message) = match output {
            Ok(mut observation) => {
                observation.task_id = Some(id.clone());
                let ok = if plan.input.action == "ping" {
                    observation.ping.is_some()
                } else if plan.input.action == "mac" {
                    observation.observed_mac.is_some() && observation.errors.is_empty()
                } else {
                    observation.adb_available && observation.errors.is_empty()
                };
                result.device = if ok {
                    ResultState::Succeeded
                } else {
                    ResultState::Failed
                };
                let message = if ok {
                    "检查完成，结果已保存在本机".into()
                } else {
                    observation.errors.join("；")
                };
                local(state)
                    .append_observation(project, &observation)
                    .await?;
                result.observation = Some(observation);
                (
                    if ok {
                        TargetState::Succeeded
                    } else {
                        TargetState::Failed
                    },
                    message,
                )
            }
            Err(AppError::Cancelled) => {
                result.device = ResultState::Cancelled;
                (
                    TargetState::Cancelled,
                    "检查已取消，未生成新的成功结果".into(),
                )
            }
            Err(error) => {
                result.device = ResultState::Failed;
                (TargetState::Failed, error.to_string())
            }
        };
        result.message = message.clone();
        let mut target_state = target_state;
        if plan.input.action == "diagnostics" {
            if let Some(observation) = result.observation.as_ref() {
                match super::diagnostics::collect(state, &plan, id, &screen, observation).await {
                    Ok(metadata) => {
                        result.evidence = serde_json::json!({"diagnostic":metadata});
                        result
                            .message
                            .push_str("；诊断文件已保存在来源电脑，可查看或导出");
                    }
                    Err(error) => {
                        result.device = ResultState::Failed;
                        target_state = TargetState::Failed;
                        result.message = format!("设备检查已保存，但诊断文件未生成：{error}");
                    }
                }
            }
        }
        task_data::save_target(state.local_store.pool(), project, id, &result).await?;
        state.task_repository.update_target(id,TargetUpdate{resource_type:DOMAIN.into(),resource_key:screen.id.clone(),state:target_state,stage:"检查完成".into(),progress_current:1,progress_total:1,fencing_token:None,message_code:Some("SCREEN_TARGET_RESULT".into()),message_params_json:Some(serde_json::json!({"name":screen.fields.name,"ip":screen.fields.ip,"summary":message}).to_string())}).await?;
        event(
            state,
            id,
            Some(&screen.id),
            target_state.as_str(),
            &message,
            if target_state == TargetState::Succeeded {
                TaskEventLevel::Info
            } else {
                TaskEventLevel::Warn
            },
        )
        .await?;
        results.targets.insert(screen.id.clone(), result);
    }
    results.finished = true;
    task_data::save_results(state.local_store.pool(), project, id, &results).await?;
    let shared = save_shared(state, id, &plan, &mut results).await;
    task_data::save_results(state.local_store.pool(), project, id, &results).await?;
    let mut current = state.task_repository.get(id).await?.state;
    let final_state = if shared.is_err() {
        TaskState::FinalizingFailed
    } else {
        completed_state(&results)
    };
    if shared.is_ok() {
        state.task_repository.resolve_results(id).await?;
    }
    if current == TaskState::Running && final_state == TaskState::Cancelled {
        state
            .task_repository
            .transition(id, current, TaskState::Cancelling, None, None)
            .await?;
        current = TaskState::Cancelling;
    }
    state
        .task_repository
        .transition(
            id,
            current,
            final_state,
            None,
            Some(if shared.is_err() {
                "设备检查结果已保存在本机，共享记录待保存"
            } else {
                "设备检查已完成"
            }),
        )
        .await?;
    event(
        state,
        id,
        None,
        "completed",
        if shared.is_err() {
            "设备检查已完成，共享记录暂未保存，可稍后补存"
        } else {
            "操作结果已保存"
        },
        TaskEventLevel::Info,
    )
    .await?;
    Ok(())
}

pub(crate) fn completed_state(results: &ScreenResults) -> TaskState {
    let ok = results
        .targets
        .values()
        .filter(|r| target_succeeded(r))
        .count();
    let cancelled = results
        .targets
        .values()
        .filter(|r| target_cancelled(r))
        .count();
    if ok == results.targets.len() {
        TaskState::Succeeded
    } else if cancelled == results.targets.len() {
        TaskState::Cancelled
    } else if ok > 0 {
        TaskState::PartiallySucceeded
    } else {
        TaskState::Failed
    }
}

pub async fn start_shared(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
) -> AppResult<Option<(ScreenSharedResults, String)>> {
    let registered: Vec<_> = plan
        .targets
        .iter()
        .filter(|s| s.source == "platform")
        .collect();
    if registered.is_empty() {
        return Ok(None);
    }
    Stage75Adapter::new(state)
        .require_project_access(&plan.project_id, ProjectAccessRequirement::ActiveSession)
        .await?;
    let business = plan
        .business_project_id
        .as_deref()
        .ok_or_else(|| AppError::Conflict("任务缺少平台业务项目".into()))?;
    let source = plan
        .data_source_id
        .as_deref()
        .ok_or_else(|| AppError::Conflict("任务缺少平台数据源".into()))?;
    let pools = project_database_for_finalization(state, &plan.project_id).await?;
    if super::platform::source_id(&pools.platform).await? != source {
        return Err(AppError::Conflict(
            "平台数据源已变化，不能保存旧任务结果".into(),
        ));
    }
    let repository = ScreenSharedResults::new(pools.workbench.clone());
    let schema = source.rsplit(':').next().unwrap_or("");
    repository.bind_source(source, schema).await?;
    let targets: Vec<String> = registered
        .iter()
        .map(|s| format!("{business}:{}", s.id))
        .collect();
    let original_task = state.task_repository.get(id).await?;
    repository
        .start(&SharedScreenOperation {
            id,
            business_project_id: business,
            action: &plan.input.action,
            name: action_label(&plan.input.action),
            operator: &plan.operator,
            instance_id: &plan.instance_id,
            targets: &targets,
            started_at: original_task
                .started_at
                .as_deref()
                .or(Some(&original_task.created_at)),
        })
        .await?;
    if let Some(previous) = &plan.input.retry_of_operation_id {
        sqlx::query("UPDATE operation_record current_op JOIN operation_record previous ON previous.id=? AND previous.domain_type='smart_screen' AND previous.business_project_id=current_op.business_project_id AND previous.operation_type=current_op.operation_type SET current_op.retry_of_operation_id=previous.id WHERE current_op.id=? AND current_op.retry_of_operation_id IS NULL")
            .bind(previous).bind(id).execute(&pools.workbench).await.map_err(|e|AppError::database("关联智能屏重试记录", &e))?;
    }
    Ok(Some((repository, business.to_string())))
}
pub async fn save_shared(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    results: &mut ScreenResults,
) -> AppResult<()> {
    let Some((repository, business)) = start_shared(state, id, plan).await? else {
        return Ok(());
    };
    let business = business.as_str();
    let registered = plan.targets.iter().filter(|s| s.source == "platform");
    for screen in registered {
        let original = results
            .targets
            .get_mut(&screen.id)
            .ok_or_else(|| AppError::Conflict("缺少逐台设备结果".into()))?;
        let mut shared = original.clone();
        shared.screen_id = format!("{business}:{}", screen.id);
        if !shared.evidence.is_object() {
            shared.evidence = serde_json::json!({});
        }
        shared.evidence["targetName"] = serde_json::json!(screen.fields.name);
        shared.evidence["targetIp"] = serde_json::json!(screen.fields.ip);
        if plan.input.action == "install" {
            let package = &plan.detail["apk"];
            shared.evidence["package"] = serde_json::json!({"name":package["name"],"version":package["appVersion"],"versionCode":package["appVersionCode"],"sha256":package["sha256"]});
        }
        if plan.input.action == "app_config" && original.evidence.get("config").is_some() {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(format!("screen-config:{id}:{}", screen.id).as_bytes());
            let audit_id = uuid::Uuid::from_slice(&digest[..16]).map_err(|_|AppError::InvalidConfig("生成配置修改记录失败".into()))?.to_string();
            let config = &original.evidence["config"];
            repository.audit(&audit_id,business,&shared.screen_id,&plan.input.action,&plan.operator,&plan.instance_id,
                &serde_json::json!({"operationId":id,"fields":config["fields"],"changes":crate::domain::smart_screen::app_config::shared_changes(config),"savedFields":config["savedFields"],"save":config["save"],"restart":config["restart"],"readback":config["readback"]})).await?;
        }
        if original.business == ResultState::Succeeded
            && ["install", "version_sync", "status"].contains(&plan.input.action.as_str())
        {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(format!("screen-business:{id}:{}", screen.id).as_bytes());
            let audit_id = uuid::Uuid::from_slice(&digest[..16])
                .map_err(|_| AppError::InvalidConfig("生成结果修改记录失败".into()))?
                .to_string();
            let change = if plan.input.action == "install" {
                serde_json::json!({"field":"app_version","before":screen.app_version,"after":original.after_app_version})
            } else {
                let item = plan.detail["items"]
                    .as_array()
                    .and_then(|items| {
                        items
                            .iter()
                            .find(|item| item["screen"]["id"].as_str() == Some(screen.id.as_str()))
                    })
                    .ok_or_else(|| AppError::Conflict("原业务更新字段记录缺失".into()))?;
                serde_json::json!({"field":item["field"],"before":item["before"],"after":item["after"]})
            };
            repository
                .audit(
                    &audit_id,
                    business,
                    &shared.screen_id,
                    &plan.input.action,
                    &plan.operator,
                    &plan.instance_id,
                    &serde_json::json!({"operationId":id,"change":change}),
                )
                .await?;
        }
        let result_state = if target_succeeded(original) {
            "succeeded"
        } else if target_cancelled(original) {
            "cancelled"
        } else if original.device == ResultState::Unknown
            || original.business == ResultState::Unknown
        {
            "interrupted"
        } else {
            "failed"
        };
        repository
            .save_target(id, business, &shared, result_state)
            .await?;
        original.shared = ResultState::Succeeded;
    }
    repository.finish(id, business).await?;
    match state
        .task_repository
        .get(id)
        .await?
        .remote_operation_record_id
        .as_deref()
    {
        None => {
            state.task_repository.link_operation(id, id).await?;
        }
        Some(existing) if existing == id => {}
        _ => return Err(AppError::Conflict("本机任务已经关联其他共享操作".into())),
    }
    Ok(())
}

pub fn recover<'a>(
    state: &'a FormalAppState,
    task: &'a TaskRecord,
    _takeover: bool,
) -> crate::infrastructure::task_recovery::TaskRecoveryFuture<'a> {
    Box::pin(async move {
        let (plan, _) =
            task_data::read_plan(state.local_store.pool(), &task.local_project_id, &task.id)
                .await?;
        let mut results =
            task_data::read_results(state.local_store.pool(), &task.local_project_id, &task.id)
                .await?;
        if !results.finished {
            for screen in &plan.targets {
                let result = results.targets.entry(screen.id.clone()).or_insert_with(|| {
                    ScreenTargetResult {
                        format_version: 1,
                        screen_id: screen.id.clone(),
                        device: ResultState::Pending,
                        shared: if screen.source == "platform" {
                            ResultState::Pending
                        } else {
                            ResultState::NotRequired
                        },
                        ..Default::default()
                    }
                });
                if matches!(result.device, ResultState::Pending | ResultState::Unknown) {
                    result.device = ResultState::Failed;
                    result.message = "上次检查中断，未取得完整结果；可重新检查".into();
                }
                state
                    .task_repository
                    .update_target(
                        &task.id,
                        TargetUpdate {
                            resource_type: DOMAIN.into(),
                            resource_key: screen.id.clone(),
                            state: if result.device == ResultState::Succeeded {
                                TargetState::Succeeded
                            } else if result.device == ResultState::Cancelled {
                                TargetState::Cancelled
                            } else {
                                TargetState::Failed
                            },
                            stage: "核实已保存结果".into(),
                            progress_current: 1,
                            progress_total: 1,
                            fencing_token: None,
                            message_code: Some("SCREEN_CHECK_RECOVERED".into()),
                            message_params_json: None,
                        },
                    )
                    .await?;
            }
            results.finished = true;
        }
        save_shared(state, &task.id, &plan, &mut results).await?;
        task_data::save_results(
            state.local_store.pool(),
            &task.local_project_id,
            &task.id,
            &results,
        )
        .await?;
        state.task_repository.resolve_results(&task.id).await?;
        state
            .task_repository
            .transition(
                &task.id,
                TaskState::FinalizingFailed,
                completed_state(&results),
                None,
                Some("已补存共享结果，未重复执行设备检查"),
            )
            .await?;
        Ok(crate::infrastructure::task_recovery::TaskRecoveryOutcome::Completed)
    })
}

pub async fn verify(state: &FormalAppState, project: &str, id: &str) -> AppResult<()> {
    let task = state.task_repository.get(id).await?;
    if task.local_project_id != project || task.domain_type != DOMAIN {
        return Err(AppError::Conflict("任务不属于当前智能屏项目".into()));
    }
    if state.job_supervisor.contains(id).await {
        return Err(AppError::Conflict("任务尚在结束处理中，请稍后核实".into()));
    }
    if task.state == TaskState::Interrupted && READ_ACTIONS.contains(&task.operation_type.as_str())
    {
        let (plan, _) = task_data::read_plan(state.local_store.pool(), project, id).await?;
        let mut results = task_data::read_results(state.local_store.pool(), project, id).await?;
        for screen in &plan.targets {
            let result =
                results
                    .targets
                    .entry(screen.id.clone())
                    .or_insert_with(|| ScreenTargetResult {
                        format_version: 1,
                        screen_id: screen.id.clone(),
                        device: ResultState::Pending,
                        shared: if screen.source == "platform" {
                            ResultState::Pending
                        } else {
                            ResultState::NotRequired
                        },
                        ..Default::default()
                    });
            if matches!(result.device, ResultState::Pending | ResultState::Unknown) {
                result.device = ResultState::Failed;
                result.message = "上次只读检查中断，未保留有效结果；可重新发起检查".into();
            }
            state
                .task_repository
                .update_target(
                    id,
                    TargetUpdate {
                        resource_type: DOMAIN.into(),
                        resource_key: screen.id.clone(),
                        state: if result.device == ResultState::Succeeded {
                            TargetState::Succeeded
                        } else if result.device == ResultState::Cancelled {
                            TargetState::Cancelled
                        } else {
                            TargetState::Failed
                        },
                        stage: "核实已有检查结果".into(),
                        progress_current: 1,
                        progress_total: 1,
                        fencing_token: None,
                        message_code: Some("SCREEN_CHECK_RECOVERED".into()),
                        message_params_json: None,
                    },
                )
                .await?;
        }
        results.finished = true;
        task_data::save_results(state.local_store.pool(), project, id, &results).await?;
        state
            .task_repository
            .transition(
                id,
                TaskState::Interrupted,
                TaskState::FinalizingFailed,
                None,
                Some("已核实本机记录，未重新连接设备"),
            )
            .await?;
    }
    crate::infrastructure::task_runtime::retry_pending_local_finalization(state, id, false).await?;
    event(
        state,
        id,
        None,
        "completed",
        "已有操作结果已核实保存",
        TaskEventLevel::Info,
    )
    .await?;
    Ok(())
}

pub async fn views(state: &FormalAppState, project: &str) -> AppResult<Vec<ScreenTaskView>> {
    use crate::application::ports::task_log::{TaskLogQuery, TaskLogStore};
    let mut views = Vec::new();
    for task in state
        .task_repository
        .list_recent(project, 500)
        .await?
        .into_iter()
        .filter(|t| {
            t.domain_type == DOMAIN
                && t.operation_type != PREFLIGHT
                && !t.operation_type.ends_with("_preview")
        })
    {
        let Ok((plan, _)) = task_data::read_plan(state.local_store.pool(), project, &task.id).await
        else {
            continue;
        };
        let results = task_data::read_results(state.local_store.pool(), project, &task.id).await?;
        let targets = state.task_repository.targets(&task.id).await?;
        let mapped = targets
            .into_iter()
            .filter_map(|target| {
                let screen = plan.targets.iter().find(|s| s.id == target.resource_key)?;
                let detail = results.targets.get(&screen.id).cloned();
                Some(ScreenTaskTarget {
                    screen_id: screen.id.clone(),
                    name: screen.fields.name.clone(),
                    ip: screen.fields.ip.clone(),
                    state: match target.state {
                        TargetState::Pending => "queued",
                        TargetState::Unknown | TargetState::Interrupted => "needs_review",
                        s => s.as_str(),
                    }
                    .into(),
                    progress: if target.progress_total > 0 {
                        ((target.progress_current * 100 / target.progress_total).min(100)) as u32
                    } else {
                        0
                    },
                    message: if matches!(target.state, TargetState::Running | TargetState::Pending)
                    {
                        target
                            .message_params_json
                            .as_deref()
                            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                            .and_then(|v| v["summary"].as_str().map(String::from))
                            .unwrap_or_else(|| {
                                if target.stage.is_empty() {
                                    "等待执行".into()
                                } else {
                                    target.stage.clone()
                                }
                            })
                    } else {
                        detail
                            .as_ref()
                            .map(|r| r.message.clone())
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| {
                                if target.state == TargetState::Running {
                                    "正在读取设备".into()
                                } else {
                                    "等待执行".into()
                                }
                            })
                    },
                    result: detail,
                })
            })
            .collect();
        let log_path = state
            .paths
            .project_task_log_path(project, &task.id)
            .map_err(map_formal_error)?;
        let logs = match state
            .task_event_pipeline
            .log_store()
            .read_page(
                &log_path,
                &TaskLogQuery {
                    limit: 100,
                    newest_first: true,
                    ..Default::default()
                },
            )
            .await
        {
            Ok(page) => page
                .items
                .into_iter()
                .map(|entry| ScreenTaskLog {
                    time: entry.timestamp,
                    level: match entry.level {
                        TaskEventLevel::Info => "INFO",
                        TaskEventLevel::Warn => "WARN",
                        TaskEventLevel::Error => "ERROR",
                    }
                    .into(),
                    message: entry.message.unwrap_or(entry.message_code),
                })
                .collect(),
            Err(_) => task
                .message
                .as_ref()
                .map(|message| {
                    vec![ScreenTaskLog {
                        time: task.updated_at.clone(),
                        level: "INFO".into(),
                        message: message.clone(),
                    }]
                })
                .unwrap_or_default(),
        };
        let protected = state.task_repository.results_protected(&task.id).await?;
        let input = if ["merge", "status", "version_sync"].contains(&task.operation_type.as_str()) {
            None
        } else {
            Some(plan.input)
        };
        views.push(ScreenTaskView {
            id: task.id,
            project_id: project.into(),
            action: task.operation_type,
            state: if protected && task.state.is_terminal() {
                "needs_review"
            } else {
                match task.state {
                    TaskState::Draft
                    | TaskState::Checking
                    | TaskState::Ready
                    | TaskState::Queued => "running",
                    TaskState::FinalizingFailed | TaskState::Interrupted => "needs_review",
                    TaskState::CheckFailed => "failed",
                    s => s.as_str(),
                }
            }
            .into(),
            created_at: task.created_at,
            updated_at: task.updated_at,
            targets: mapped,
            logs,
            input,
        });
    }
    Ok(views)
}

pub async fn event(
    state: &FormalAppState,
    id: &str,
    screen: Option<&str>,
    status: &str,
    message: &str,
    level: TaskEventLevel,
) -> AppResult<()> {
    state
        .task_event_pipeline
        .emit(
            id,
            TaskEventInput {
                resource_type: screen.map(|_| DOMAIN.into()),
                resource_key: screen.map(String::from),
                stage: "screen_operation".into(),
                status: status.into(),
                progress_current: None,
                progress_total: None,
                level,
                message_code: "SCREEN_OPERATION".into(),
                message_params: BTreeMap::new(),
                message: Some(message.into()),
            },
        )
        .await?;
    Ok(())
}
