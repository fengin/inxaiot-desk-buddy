use super::{
    device::{AdbDevice, AndroidTools},
    leases::HeldScreenLeases,
    platform_write::{self, MutationError, MutationReceipt},
    previews, task_data, tasks, write_context,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::{
    common::task::{TargetState, TaskEventLevel, TaskRecord, TaskState},
    smart_screen::{
        model::*,
        operation::*,
        rules::{normalize_mac, valid_mac},
    },
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::{
    client_instance::application_instance_id,
    local_sqlite::{
        screen_repository::{ScreenRepository, now},
        task_repository::TargetUpdate,
    },
    task_recovery::{TaskRecoveryFuture, TaskRecoveryOutcome},
};
use crate::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

pub const PREVIEW: &str = "screen_value_preview";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValueItem {
    pub screen: ScreenAsset,
    pub field: String,
    pub before: Option<String>,
    pub after: String,
    pub request_id: String,
    pub observation: ScreenObservation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValuePlan {
    pub items: Vec<ValueItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionPreviewItem {
    pub screen_id: String,
    pub name: String,
    pub ip: String,
    pub platform_version: Option<String>,
    pub device_version: Option<String>,
    pub checked_at: String,
    pub state: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionPreview {
    pub id: String,
    pub project_id: String,
    pub created_at: String,
    pub items: Vec<VersionPreviewItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusChange {
    pub id: String,
    pub ip: String,
    pub expected: String,
    pub next: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusResult {
    pub id: String,
    pub name: String,
    pub ok: bool,
    pub message: String,
}
fn local(state: &FormalAppState) -> ScreenRepository {
    ScreenRepository::new(state.local_store.pool().clone())
}
pub fn critical_draft(snapshot: &ScreenSnapshot, screen: &ScreenAsset) -> bool {
    snapshot.platform_drafts.get(&screen.id).is_some_and(|d| {
        (d.values.ip != d.base.ip && d.values.ip != screen.fields.ip)
            || (normalize_mac(&d.values.mac) != normalize_mac(&d.base.mac)
                && normalize_mac(&d.values.mac) != normalize_mac(&screen.fields.mac))
            || (d.values.size != d.base.size && d.values.size != screen.fields.size)
    })
}
pub fn reliable_identity(screen: &ScreenAsset, observation: &ScreenObservation) -> bool {
    observation.adb_available
        && observation.observed_ip == screen.fields.ip
        && observation.observed_mac.as_deref().is_some_and(valid_mac)
        && (!valid_mac(&screen.fields.mac)
            || observation
                .observed_mac
                .as_deref()
                .is_some_and(|m| normalize_mac(m) == normalize_mac(&screen.fields.mac)))
}
fn detail(plan: &ScreenPlan) -> AppResult<ValuePlan> {
    serde_json::from_value(plan.detail.clone())
        .map_err(|_| AppError::Conflict("业务结果检查记录无效".into()))
}
fn make_plan(
    project: &str,
    action: &str,
    items: Vec<ValueItem>,
    business: Option<String>,
    source: Option<String>,
    operator: String,
) -> AppResult<ScreenPlan> {
    Ok(ScreenPlan {
        project_id: project.into(),
        input: ScreenOperationInput {
            action: action.into(),
            target_ids: items.iter().map(|i| i.screen.id.clone()).collect(),
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: String::new(),
            reinstall: false,
            concurrency: 1,
            retry_of_operation_id: None, expected_targets: BTreeMap::new(),
        },
        targets: items.iter().map(|i| i.screen.clone()).collect(),
        business_project_id: business,
        data_source_id: source,
        operator,
        instance_id: application_instance_id().into(),
        created_at: now(),
        detail: serde_json::to_value(ValuePlan { items })
            .map_err(|_| AppError::InvalidConfig("保存检查内容失败".into()))?,
    })
}
pub async fn version_preview(
    state: &FormalAppState,
    project: &str,
    ids: Vec<String>,
) -> AppResult<VersionPreview> {
    let snapshot = local(state).snapshot(project).await?;
    let context = write_context::open(state, project).await;
    let mut view = Vec::new();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for requested in ids {
        let screen = snapshot
            .screens
            .iter()
            .find(|s| s.id == requested || s.aliases.contains(&requested))
            .ok_or_else(|| AppError::NotFound("所选屏记录已变化".into()))?;
        if !seen.insert(screen.id.clone()) {
            continue;
        }
        let mut row = VersionPreviewItem {
            screen_id: screen.id.clone(),
            name: screen.fields.name.clone(),
            ip: screen.fields.ip.clone(),
            platform_version: screen.app_version.clone(),
            device_version: None,
            checked_at: now(),
            state: "blocked".into(),
            reason: String::new(),
        };
        let inspected = match AndroidTools::discover() {
            Ok(tools) => {
                AdbDevice::new(tools)
                    .inspect(screen, "inspect", CancellationToken::new())
                    .await
            }
            Err(error) => Err(error),
        };
        match inspected {
            Err(error) => row.reason = error.to_string(),
            Ok(mut observation) => {
                observation.operation_type = "version_sync".into();
                local(state)
                    .append_observation(project, &observation)
                    .await?;
                row.checked_at = observation.observed_at.clone();
                row.device_version = observation.observed_app_version.clone();
                if !reliable_identity(screen, &observation) {
                    row.reason = "设备身份或管理连接未通过核对，不能同步版本".into();
                } else if observation.app_installed != Some(true) || row.device_version.is_none() {
                    row.reason = "小新未安装或版本读取失败，保留平台原版本".into();
                } else if screen.source == "local" {
                    row.state = "local".into();
                    row.reason = "已保存本机实测，登记平台后可同步版本".into();
                } else if critical_draft(&snapshot, screen) {
                    row.reason = "IP、MAC 或尺寸有待提交修改，请先处理".into();
                } else if let Err(error) = local(state).require_idle(project, &screen.id).await {
                    row.reason = error.to_string();
                } else if let Err(error) = &context {
                    row.reason = format!("本机版本已读取，平台同步条件不可用：{error}");
                } else {
                    let ctx = context.as_ref().expect("checked context");
                    let current = platform_write::record(&ctx.read, &screen.id)
                        .await?
                        .ok_or_else(|| AppError::Conflict("平台屏已不存在".into()))?;
                    row.platform_version = current.asset.app_version.clone();
                    if current.business.as_deref() != Some(&ctx.business)
                        || !crate::application::smart_screen::operations::same_target(
                            screen,
                            &current.asset,
                        )
                    {
                        row.reason = "平台地址或身份已变化，请刷新后重新核对".into();
                    } else if row.platform_version == row.device_version {
                        row.state = "skip".into();
                        row.reason = "平台与设备版本一致，无需同步".into();
                    } else {
                        row.state = "ready".into();
                        row.reason = "已读取设备版本，确认后只更新平台版本信息".into();
                        items.push(ValueItem {
                            screen: current.asset,
                            field: "app_version".into(),
                            before: row.platform_version.clone(),
                            after: row.device_version.clone().unwrap(),
                            request_id: uuid::Uuid::now_v7().to_string(),
                            observation,
                        });
                    }
                }
            }
        }
        view.push(row);
    }
    let id = uuid::Uuid::now_v7().to_string();
    if !items.is_empty() {
        let context = context?;
        let plan = make_plan(
            project,
            "version_sync",
            items,
            Some(context.business),
            Some(context.source),
            context.operator,
        )?;
        previews::save(state, &id, &plan, PREVIEW).await?;
    }
    Ok(VersionPreview {
        id,
        project_id: project.into(),
        created_at: now(),
        items: view,
    })
}
pub async fn version_submit(
    state: &FormalAppState,
    project: &str,
    preview: &str,
    ids: Vec<String>,
) -> AppResult<String> {
    let context = write_context::open(state, project).await?;
    let _guard = tasks::submission_lock().lock().await;
    let (mut plan, hash) = task_data::read_preflight_plan(state.local_store.pool(), project, preview).await?;
    if plan.input.action != "version_sync"
        || plan.business_project_id.as_deref() != Some(&context.business)
        || plan.data_source_id.as_deref() != Some(&context.source)
    {
        return Err(AppError::Conflict("版本检查不属于当前项目".into()));
    }
    let selected = ids.iter().cloned().collect::<BTreeSet<_>>();
    if selected.is_empty()
        || selected.len() != ids.len()
        || !selected.is_subset(&plan.targets.iter().map(|s| s.id.clone()).collect())
    {
        return Err(AppError::Conflict("请选择本次检查通过的版本差异".into()));
    }
    let snapshot = local(state).snapshot(project).await?;
    for screen in plan.targets.iter().filter(|s| selected.contains(&s.id)) {
        local(state).require_idle(project, &screen.id).await?;
        if critical_draft(&snapshot, screen) {
            return Err(AppError::Conflict(
                "关键资料有待提交修改，请重新核对".into(),
            ));
        }
    }
    let mut values = detail(&plan)?;
    values.items.retain(|i| selected.contains(&i.screen.id));
    plan.targets.retain(|s| selected.contains(&s.id));
    plan.input.target_ids = ids;
    plan.detail = serde_json::to_value(values).unwrap();
    plan.operator = context.operator;
    previews::queue(state, preview, PREVIEW, &hash, &plan).await
}
pub async fn cover_status(
    state: &FormalAppState,
    project: &str,
    changes: Vec<StatusChange>,
) -> AppResult<Vec<StatusResult>> {
    let context = write_context::open(state, project).await?;
    let _guard = tasks::submission_lock().lock().await;
    let snapshot = local(state).snapshot(project).await?;
    let mut results = Vec::new();
    let mut items = Vec::new();
    let mut seen = BTreeSet::new();
    for change in changes {
        if !seen.insert(change.id.clone()) {
            continue;
        }
        let screen = snapshot
            .screens
            .iter()
            .find(|s| s.id == change.id && s.source == "platform")
            .ok_or_else(|| AppError::NotFound("平台屏记录已变化".into()))?;
        let observation = snapshot
            .observations
            .get(&screen.id)
            .into_iter()
            .flatten()
            .find(|o| {
                o.observed_ip == screen.fields.ip
                    && ["ping", "inspect", "diagnostics"].contains(&o.operation_type.as_str())
            });
        let reason = if screen.fields.ip != change.ip
            || screen.revision != change.revision
            || screen.platform_status != change.expected
        {
            Some("平台资料或检查地址已变化，请重新核对".into())
        } else if !["online", "offline"].contains(&change.expected.as_str())
            || !["online", "offline"].contains(&change.next.as_str())
        {
            Some("平台状态无法识别，请先核实".into())
        } else if observation.and_then(|o| o.ping.as_deref()) != Some(change.next.as_str()) {
            Some("没有本次地址对应的有效 IP 检查，不能覆盖平台状态".into())
        } else {
            local(state)
                .require_idle(project, &screen.id)
                .await
                .err()
                .map(|e| e.to_string())
        };
        if let Some(reason) = reason {
            results.push(StatusResult {
                id: screen.id.clone(),
                name: screen.fields.name.clone(),
                ok: false,
                message: reason,
            });
        } else if change.expected == change.next {
            results.push(StatusResult {
                id: screen.id.clone(),
                name: screen.fields.name.clone(),
                ok: true,
                message: "状态一致，无需覆盖".into(),
            });
        } else {
            items.push(ValueItem {
                screen: screen.clone(),
                field: "status".into(),
                before: Some(change.expected),
                after: change.next,
                request_id: uuid::Uuid::now_v7().to_string(),
                observation: observation.unwrap().clone(),
            });
        }
    }
    if items.is_empty() {
        return Ok(results);
    }
    let id = uuid::Uuid::now_v7().to_string();
    let plan = make_plan(
        project,
        "status",
        items,
        Some(context.business),
        Some(context.source),
        context.operator,
    )?;
    previews::save(state, &id, &plan, PREVIEW).await?;
    let (_, hash) = task_data::read_plan(state.local_store.pool(), project, &id).await?;
    let task = previews::queue(state, &id, PREVIEW, &hash, &plan).await?;
    drop(_guard);
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let t = state.task_repository.get(&task).await?;
            if t.state.is_terminal() || t.state == TaskState::FinalizingFailed {
                return Ok::<(), AppError>(());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| {
        AppError::Conflict(format!(
            "状态覆盖仍在处理，请查看原任务 {task}，不要重复提交"
        ))
    })??;
    let stored = task_data::read_results(state.local_store.pool(), project, &task).await?;
    for screen in &plan.targets {
        let result = stored.targets.get(&screen.id);
        results.push(StatusResult {
            id: screen.id.clone(),
            name: screen.fields.name.clone(),
            ok: result.is_some_and(|r| r.business == ResultState::Succeeded),
            message: result
                .map(|r| r.message.clone())
                .unwrap_or_else(|| "结果待核实".into()),
        });
    }
    Ok(results)
}
pub fn register(registry: &TaskHandlerRegistry, app: AppHandle) -> AppResult<()> {
    for action in ["version_sync", "status"] {
        let app = app.clone();
        registry.register(tasks::DOMAIN, action, move |envelope, cancel| {
            let app = app.clone();
            async move { run(&app.state::<FormalAppState>(), envelope, cancel).await }
        })?;
    }
    Ok(())
}
async fn save_target_state(
    state: &FormalAppState,
    id: &str,
    result: &ScreenTargetResult,
) -> AppResult<()> {
    state
        .task_repository
        .update_target(
            id,
            TargetUpdate {
                resource_type: tasks::DOMAIN.into(),
                resource_key: result.screen_id.clone(),
                state: if target_succeeded(result) {
                    TargetState::Succeeded
                } else if target_cancelled(result) {
                    TargetState::Cancelled
                } else if result.business == ResultState::Unknown {
                    TargetState::Unknown
                } else {
                    TargetState::Failed
                },
                stage: "保存业务结果".into(),
                progress_current: 1,
                progress_total: 1,
                fencing_token: None,
                message_code: Some("SCREEN_BUSINESS_VALUE".into()),
                message_params_json: Some(
                    serde_json::json!({"summary":result.message}).to_string(),
                ),
            },
        )
        .await?;
    Ok(())
}
async fn finish(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    results: &mut ScreenResults,
) -> AppResult<()> {
    let unknown = results
        .targets
        .values()
        .any(|r| matches!(r.business, ResultState::Pending | ResultState::Unknown));
    let shared = if unknown {
        Err(AppError::Conflict("业务结果仍待核实".into()))
    } else {
        tasks::save_shared(state, id, plan, results).await
    };
    results.finished = true;
    task_data::save_results(state.local_store.pool(), &plan.project_id, id, results).await?;
    let current = state.task_repository.get(id).await?.state;
    let next = if shared.is_ok() {
        tasks::completed_state(results)
    } else {
        TaskState::FinalizingFailed
    };
    if shared.is_ok() {
        state.task_repository.resolve_results(id).await?;
    }
    let current = if current == TaskState::Running && next == TaskState::Cancelled {
        state
            .task_repository
            .transition(id, current, TaskState::Cancelling, None, None)
            .await?;
        TaskState::Cancelling
    } else {
        current
    };
    if current != next {
        state
            .task_repository
            .transition(
                id,
                current,
                next,
                None,
                Some(if shared.is_ok() {
                    "业务结果已保存"
                } else {
                    "结果待核实或补存，不重放旧更新"
                }),
            )
            .await?;
    }
    tasks::event(
        state,
        id,
        None,
        "completed",
        if shared.is_ok() {
            "业务结果已保存"
        } else {
            "本机结果已保留，平台或共享记录待核实"
        },
        TaskEventLevel::Info,
    )
    .await?;
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
    let values = detail(&plan)?;
    state
        .task_repository
        .transition(id, TaskState::Queued, TaskState::Running, None, None)
        .await?;
    let mut results = ScreenResults {
        targets: values
            .items
            .iter()
            .map(|i| {
                (
                    i.screen.id.clone(),
                    ScreenTargetResult {
                        format_version: 1,
                        screen_id: i.screen.id.clone(),
                        device: ResultState::NotRequired,
                        business: ResultState::Pending,
                        shared: ResultState::Pending,
                        observation: Some(i.observation.clone()),
                        ..Default::default()
                    },
                )
            })
            .collect(),
        finished: false,
    };
    task_data::save_results(state.local_store.pool(), project, id, &results).await?;
    let context = match write_context::open(state, project).await {
        Ok(context) => context,
        Err(error) => {
            for result in results.targets.values_mut() {
                result.business = ResultState::Failed;
                result.shared = ResultState::NotRequired;
                result.message = format!("平台条件不可用，未执行更新：{error}");
            }
            results.finished = true;
            task_data::save_results(state.local_store.pool(), project, id, &results).await?;
            state.task_repository.resolve_results(id).await?;
            state
                .task_repository
                .transition(
                    id,
                    TaskState::Running,
                    TaskState::Failed,
                    None,
                    Some("平台条件不可用，未执行更新"),
                )
                .await?;
            return Ok(());
        }
    };
    tasks::start_shared(state, id, &plan).await?;
    for item in &values.items {
        let mut result = results.targets[&item.screen.id].clone();
        if cancel.is_cancelled() {
            result.business = ResultState::Cancelled;
            result.message = "尚未执行的更新已取消".into();
        } else {
            let target_result = apply_item(
                state,
                &context,
                id,
                &plan,
                item,
                &mut result,
                cancel.clone(),
            )
            .await;
            if let Err(error) = target_result {
                let intent = local(state)
                    .intents(project)
                    .await?
                    .into_iter()
                    .find(|i| i.request_id == item.request_id);
                if intent.as_ref().is_some_and(|i| i.state == "submitted") {
                    result.business = ResultState::Unknown;
                } else {
                    if intent.as_ref().is_some_and(|i| i.state == "prepared") {
                        local(state)
                            .update_intent(
                                project,
                                &item.request_id,
                                "prepared",
                                "not_applied",
                                None,
                            )
                            .await?;
                    }
                    result.business = ResultState::Failed;
                }
                result.message = error.to_string();
            }
        }
        task_data::save_target(state.local_store.pool(), project, id, &result).await?;
        save_target_state(state, id, &result).await?;
        tasks::event(
            state,
            id,
            Some(&item.screen.id),
            "result",
            &result.message,
            if target_succeeded(&result) {
                TaskEventLevel::Info
            } else {
                TaskEventLevel::Warn
            },
        )
        .await?;
        results.targets.insert(item.screen.id.clone(), result);
    }
    finish(state, id, &plan, &mut results).await
}
async fn apply_item(
    state: &FormalAppState,
    context: &write_context::ScreenWriteContext,
    id: &str,
    plan: &ScreenPlan,
    item: &ValueItem,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
) -> AppResult<()> {
    context.validate_plan(plan)?;
    let held = HeldScreenLeases::acquire(
        context,
        id,
        &plan.instance_id,
        &[item.screen.id.clone()],
        false,
        false,
    )
    .await?;
    let outcome=async{
        if critical_draft(&local(state).snapshot(&plan.project_id).await?,&item.screen)&&item.field=="app_version"{return Err(AppError::Conflict("关键资料存在待提交修改，版本未同步".into()));}
        if item.field=="app_version"{
            let device=AdbDevice::new(AndroidTools::discover()?);let mut current=device.inspect(&item.screen,"inspect",cancel).await?;current.operation_type="version_sync".into();current.task_id=Some(id.into());local(state).append_observation(&plan.project_id,&current).await?;
            if !reliable_identity(&item.screen,&current)||current.observed_app_version.as_deref()!=Some(item.after.as_str())||current.observed_mac!=item.observation.observed_mac{return Err(AppError::Conflict("设备版本或身份在确认后发生变化，请重新核对".into()));}
            result.observation=Some(current);
        }
        held.valid().await?;
        let intent=WriteIntent{request_id:item.request_id.clone(),business_project_id:context.business.clone(),screen_id:item.screen.id.clone(),platform_screen_id:item.screen.id.clone(),operation_type:plan.input.action.clone(),payload:serde_json::json!({"field":item.field,"before":item.before,"after":item.after,"ip":item.screen.fields.ip}),state:"prepared".into(),result:None};
        local(state).prepare_intent(&plan.project_id,&intent).await?;
        local(state).update_intent(&plan.project_id,&item.request_id,"prepared","submitted",None).await?;
        let grant=held.grants.iter().find(|g|g.resource_type=="smart_screen").expect("screen grant");
        let written=tokio::time::timeout(Duration::from_secs(30),platform_write::set_business_value(context,grant,&item.screen.id,&item.screen.fields,&item.field,item.before.as_deref(),&item.after)).await.unwrap_or_else(|_|Err(MutationError::Uncertain(AppError::timeout("业务更新提交"))));
        match written{
            Ok(receipt)=>{
                let proof=serde_json::to_value(&receipt).map_err(|_|AppError::InvalidConfig("保存业务回读失败".into()))?;
                match local(state).confirm_business_value(&plan.project_id,&context.business,&item.request_id,&receipt.after,&proof).await{
                    Ok(())=>{result.business=ResultState::Succeeded;result.message=if item.field=="app_version"{"平台应用版本已确认同步"}else{"平台在线状态已确认更新"}.into();result.evidence=proof;},
                    Err(error)=>{result.business=ResultState::Unknown;result.message=format!("平台已更新，本机确认待补存：{error}");result.evidence=proof;}
                }
            },
            Err(MutationError::Rejected(error))=>{local(state).update_intent(&plan.project_id,&item.request_id,"submitted","not_applied",None).await?;result.business=ResultState::Failed;result.message=error.to_string();},
            Err(MutationError::Uncertain(error))=>{result.business=ResultState::Unknown;result.message=format!("结果待核实：{error}");}
        }
        if item.field=="app_version"{result.before_app_version=item.before.clone();result.after_app_version=Some(item.after.clone());}
        Ok(())
    }.await;
    let submitted = if outcome.is_err() {
        local(state)
            .intents(&plan.project_id)
            .await
            .map(|items| {
                items
                    .iter()
                    .any(|i| i.request_id == item.request_id && i.state == "submitted")
            })
            .unwrap_or(true)
    } else {
        false
    };
    if result.business != ResultState::Unknown && !submitted {
        let _ = held.release().await;
    }
    outcome
}
pub fn recover<'a>(
    state: &'a FormalAppState,
    task: &'a TaskRecord,
    _force: bool,
) -> TaskRecoveryFuture<'a> {
    Box::pin(async move {
        let (plan, _) =
            task_data::read_plan(state.local_store.pool(), &task.local_project_id, &task.id)
                .await?;
        let values = detail(&plan)?;
        let mut results =
            task_data::read_results(state.local_store.pool(), &plan.project_id, &task.id).await?;
        let context = write_context::open(state, &plan.project_id).await?;
        context.validate_plan(&plan)?;
        tasks::start_shared(state, &task.id, &plan).await?;
        let intents = local(state).intents(&plan.project_id).await?;
        for item in &values.items {
            let result = results
                .targets
                .entry(item.screen.id.clone())
                .or_insert_with(|| ScreenTargetResult {
                    format_version: 1,
                    screen_id: item.screen.id.clone(),
                    device: ResultState::NotRequired,
                    business: ResultState::Unknown,
                    shared: ResultState::Pending,
                    ..Default::default()
                });
            if matches!(
                result.business,
                ResultState::Succeeded | ResultState::Failed | ResultState::Cancelled
            ) {
                continue;
            }
            match intents.iter().find(|i| i.request_id == item.request_id) {
                Some(intent) if intent.state == "confirmed" => {
                    result.business = ResultState::Succeeded;
                    result.evidence = intent.result.clone().unwrap_or_default();
                    result.message = if item.field == "app_version" {
                        "平台应用版本已确认同步"
                    } else {
                        "平台在线状态已确认更新"
                    }
                    .into();
                }
                Some(intent) if intent.state == "submitted" => {
                    let held = HeldScreenLeases::acquire(
                        &context,
                        &task.id,
                        &plan.instance_id,
                        &[item.screen.id.clone()],
                        false,
                        true,
                    )
                    .await?;
                    let mut tx = context
                        .write
                        .begin()
                        .await
                        .map_err(|e| AppError::database("核实原业务提交", &e))?;
                    platform_write::guard_lease(&mut tx, &context.shared_schema, &held.grants[0])
                        .await?;
                    let current = platform_write::record_locked(&mut tx, &item.screen.id).await?;
                    tx.commit()
                        .await
                        .map_err(|e| AppError::database("结束原提交核实", &e))?;
                    if let Some(current) = current.filter(|r| {
                        r.business.as_deref() == Some(&context.business)
                            && crate::application::smart_screen::operations::same_target(
                                &item.screen,
                                &r.asset,
                            )
                            && (if item.field == "app_version" {
                                r.asset.app_version.as_deref() == Some(item.after.as_str())
                            } else {
                                r.asset.platform_status == item.after
                            })
                    }) {
                        let receipt = MutationReceipt {
                            before: None,
                            after: current.asset,
                            wrote: false,
                        };
                        let proof = serde_json::to_value(&receipt)
                            .map_err(|_| AppError::InvalidConfig("核实业务结果失败".into()))?;
                        local(state)
                            .confirm_business_value(
                                &plan.project_id,
                                &context.business,
                                &item.request_id,
                                &receipt.after,
                                &proof,
                            )
                            .await?;
                        result.business = ResultState::Succeeded;
                        result.evidence = proof;
                        result.message = if item.field == "app_version" {
                            "平台应用版本已确认同步"
                        } else {
                            "平台在线状态已确认更新"
                        }
                        .into();
                    } else {
                        local(state)
                            .update_intent(
                                &plan.project_id,
                                &item.request_id,
                                "submitted",
                                "conflict",
                                None,
                            )
                            .await?;
                        result.business = ResultState::Failed;
                        result.message =
                            "平台当前值与原更新不一致，未重放旧值，请重新检查并确认".into();
                    }
                    let _ = held.release().await;
                }
                Some(intent) if intent.state == "prepared" => {
                    local(state)
                        .update_intent(
                            &plan.project_id,
                            &item.request_id,
                            "prepared",
                            "not_applied",
                            None,
                        )
                        .await?;
                    result.business = ResultState::Failed;
                    result.message = "上次更新尚未提交，未自动重放".into();
                }
                _ => {
                    result.business = ResultState::Failed;
                    result.message = "上次更新未完成，请重新检查".into();
                }
            }
            if item.field == "app_version" {
                result.before_app_version = item.before.clone();
                result.after_app_version = Some(item.after.clone());
            }
            task_data::save_target(state.local_store.pool(), &plan.project_id, &task.id, result)
                .await?;
            save_target_state(state, &task.id, result).await?;
        }
        finish(state, &task.id, &plan, &mut results).await?;
        if state.task_repository.get(&task.id).await?.state == TaskState::FinalizingFailed {
            return Err(AppError::Conflict("仍有结果待核实或补存".into()));
        }
        Ok(TaskRecoveryOutcome::Completed)
    })
}
