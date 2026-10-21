use super::{
    assets_service::ScreenAssetsService,
    device::{AdbDevice, AndroidTools},
    leases::HeldScreenLeases,
    platform_write::{self, MutationError, MutationReceipt},
    previews,
    shared_results::{ScreenSharedResults, SharedScreenOperation},
    task_data, tasks, write_context,
};
use crate::application::ports::smart_screen::ScreenAssetsPort;
use crate::core::error::{AppError, AppResult};
use crate::domain::{
    common::task::{TargetState, TaskEventLevel, TaskRecord, TaskState},
    smart_screen::{
        model::*,
        operation::*,
        registration::*,
        rules::{normalize_mac, space_path, valid_mac, validate_fields, validate_space},
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
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

pub const PREVIEW: &str = "screen_registration_preview";
fn success_message(mode: &str) -> &str {
    match mode {
        "create" => "注册成功，平台写入与本机关联已确认",
        "merge" => "资料与本机关联已合并，未安装应用或改变屏端配置",
        _ => "资料更新成功，未修改状态或应用版本",
    }
}
fn local(state: &FormalAppState) -> ScreenRepository {
    ScreenRepository::new(state.local_store.pool().clone())
}
fn detail(plan: &ScreenPlan) -> AppResult<RegistrationPlan> {
    serde_json::from_value(plan.detail.clone())
        .map_err(|_| AppError::Conflict("注册检查记录格式无效".into()))
}
fn needs_mac_collection(before: Option<&ScreenFields>, after: &ScreenFields) -> bool {
    before.is_none_or(|before| {
        before.mac.trim().is_empty()
            || !field_semantically_equal("ip", &before.ip, &after.ip)
            || !field_semantically_equal("mac", &before.mac, &after.mac)
    })
}

fn historical_mac(
    snapshot: &ScreenSnapshot,
    screen: &ScreenAsset,
    target_ip: &str,
) -> Option<String> {
    std::iter::once(&screen.id)
        .chain(screen.aliases.iter())
        .filter_map(|id| snapshot.observations.get(id))
        .flatten()
        .filter(|observation| {
            observation.observed_ip == target_ip
                && observation.adb_available
                && observation.mac_candidates.len() <= 1
                && observation.observed_mac.as_deref().is_some_and(valid_mac)
        })
        .filter_map(|observation| {
            time::OffsetDateTime::parse(
                &observation.observed_at,
                &time::format_description::well_known::Rfc3339,
            )
            .ok()
            .map(|time| (time, observation))
        })
        .max_by_key(|(time, observation)| (*time, observation.id.as_str()))
        .and_then(|(_, observation)| observation.observed_mac.clone())
}

#[derive(Debug)]
struct MacDecision {
    source: &'static str,
    message: String,
    confirmation: Option<&'static str>,
    errors: Vec<String>,
}

fn apply_mac_observation(
    current: &ScreenFields,
    before: Option<&ScreenFields>,
    after: &mut ScreenFields,
    previous: Option<String>,
    observed: Result<&ScreenObservation, String>,
) -> MacDecision {
    let mut decision = MacDecision {
        source: "unchanged",
        message: String::new(),
        confirmation: None,
        errors: Vec::new(),
    };
    match observed {
        Ok(observation) => {
            if observation.observed_ip != after.ip {
                decision.source = "conflict";
                decision.message = "采集地址不符合本次目标，请重新检查".into();
                decision.errors.push(decision.message.clone());
                return decision;
            }
            if observation.adb_available && observation.mac_candidates.len() > 1 {
                decision.source = "conflict";
                decision.message = observation.mac_candidates.join("；");
                decision.errors.push("设备存在多个有线网卡，请核实身份，不能按空 MAC 绕过".into());
                return decision;
            }
            if let Some(mac) = observation
                .observed_mac
                .as_ref()
                .filter(|mac| observation.adb_available && valid_mac(mac))
            {
                if before.is_some_and(|before| {
                    valid_mac(&before.mac)
                        && !field_semantically_equal("mac", &before.mac, mac)
                }) || (valid_mac(&after.mac)
                    && !field_semantically_equal("mac", &after.mac, mac))
                {
                    decision.source = "conflict";
                    decision.message = format!("本次有线 MAC：{mac}");
                    decision.errors.push("采集 MAC 与已有身份资料冲突，请先核实".into());
                } else {
                    after.mac = mac.clone();
                    decision.source = "collected";
                    decision.message = format!("已读取有线网卡 MAC：{mac}");
                }
                return decision;
            }
            decision.message = if !observation.adb_available && observation.errors.is_empty() {
                "设备 ADB 连接未确认，不能采用返回的 MAC".into()
            } else {
                observation.errors.join("；")
            };
        }
        Err(message) => decision.message = message,
    }
    let known = before
        .map(|before| before.mac.clone())
        .filter(|mac| valid_mac(mac))
        .or(previous.filter(|mac| valid_mac(mac)))
        .or_else(|| valid_mac(&current.mac).then(|| current.mac.clone()));
    if valid_mac(&after.mac)
        && known.as_ref().is_some_and(|mac| !field_semantically_equal("mac", mac, &after.mac))
    {
        decision.source = "conflict";
        decision.errors.push("待提交 MAC 与可信历史冲突，采集失败不能跳过".into());
    } else if before.is_some_and(|before| {
        known.is_none()
            && valid_mac(&after.mac)
            && !field_semantically_equal("mac", &before.mac, &after.mac)
    }) {
        decision.source = "conflict";
        decision.message = "新增 MAC 尚未通过现场采集核实，不能作为历史地址提交".into();
        decision.errors.push(decision.message.clone());
    } else {
        after.mac = known.unwrap_or_default();
        let empty = after.mac.is_empty();
        decision.source = if empty { "empty" } else { "history" };
        decision.confirmation = Some(if empty { "empty" } else { "existing" });
        decision.message = format!(
            "本次未取得可靠 MAC（{}）；{}",
            decision.message,
            if empty { "需确认暂空登记" } else { "需确认沿用历史地址" },
        );
    }
    decision
}
fn diffs(
    before: Option<&ScreenFields>,
    after: &ScreenFields,
    spaces: &[SpaceNode],
) -> Vec<RegistrationDiff> {
    let old = before.map(field_values).unwrap_or_default();
    field_values(after)
        .into_iter()
        .filter(|(key, value)| {
            old.get(key).is_none_or(|before| !field_semantically_equal(key, before, value))
        })
        .map(|(key, value)| {
            let display = |value: String| {
                if key == "spaceId" {
                    space_path(spaces, &value)
                        .map(|nodes| {
                            nodes
                                .iter()
                                .map(|n| n.name.as_str())
                                .collect::<Vec<_>>()
                                .join("/")
                        })
                        .unwrap_or(value)
                } else if key == "size" {
                    match value.as_str() {
                        "4" => "4 寸".into(),
                        "10" => "10 寸".into(),
                        "unknown" => "待确认".into(),
                        _ => value,
                    }
                } else {
                    value
                }
            };
            RegistrationDiff {
                field: key.into(),
                label: label(key).into(),
                before: display(old.get(key).cloned().unwrap_or_default()),
                after: display(value),
            }
        })
        .collect()
}
pub async fn preview(
    state: &FormalAppState,
    project: &str,
    ids: Vec<String>,
) -> AppResult<RegistrationPreview> {
    preview_with_mac(state, project, ids, |target| async move {
        match AndroidTools::discover() {
            Ok(tools) => {
                AdbDevice::new(tools)
                    .inspect(&target, "mac", CancellationToken::new())
                    .await
            }
            Err(error) => Err(error),
        }
    })
    .await
}
pub async fn preview_with_mac<F, Fut>(
    state: &FormalAppState,
    project: &str,
    ids: Vec<String>,
    collect_mac: F,
) -> AppResult<RegistrationPreview>
where
    F: Fn(ScreenAsset) -> Fut + Send + Sync,
    Fut: std::future::Future<Output = AppResult<ScreenObservation>> + Send,
{
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(AppError::InvalidConfig("请选择不重复的屏记录".into()));
    }
    let context = write_context::open(state, project).await?;
    let snapshot = ScreenAssetsService::new(state)
        .snapshot(project, true)
        .await?;
    if !snapshot.platform_available || !snapshot.spaces_available {
        return Err(AppError::Conflict("请恢复平台和空间读取后再登记".into()));
    }
    let id = uuid::Uuid::now_v7().to_string();
    let mut items = Vec::new();
    let mut originals = BTreeMap::new();
    let mut revisions = BTreeMap::new();
    for screen_id in ids {
        let screen = snapshot
            .screens
            .iter()
            .find(|s| s.id == screen_id)
            .ok_or_else(|| AppError::NotFound("所选屏不在当前项目中".into()))?;
        let before = (screen.source == "platform").then_some(screen.fields.clone());
        let draft = snapshot.platform_drafts.get(&screen_id);
        let mut after = draft
            .map(|d| effective_draft_fields(&screen.fields, d))
            .unwrap_or_else(|| screen.fields.clone());
        let mut errors = Vec::new();
        if let Err(error) = local(state).require_idle(project, &screen_id).await {
            errors.push(error.to_string());
        }
        if let Some(draft) = draft {
            for key in draft_conflicting_fields(&screen.fields, draft) {
                errors.push(format!("平台{}已变化，请重新编辑草稿确认", label(key)));
            }
            revisions.insert(screen_id.clone(), draft.revision);
        }
        if let Err(error) = validate_fields(&after, true) {
            errors.push(error.to_string());
        }
        if let Err(error) = validate_space(&after, &snapshot.spaces, true) {
            errors.push(error.to_string());
        }
        let mut source = "unchanged".to_string();
        let mut mac_message = "设备地址与身份未改变，无需连接设备".to_string();
        let mut confirmation = None;
        if needs_mac_collection(before.as_ref(), &after)
            && after.ip.parse::<std::net::Ipv4Addr>().is_ok()
        {
            let mut target = screen.clone();
            target.fields = after.clone();
            let observed = collect_mac(target).await;
            let previous = historical_mac(&snapshot, screen, &after.ip);
            if let Ok(observation) = &observed {
                local(state).append_observation(project, observation).await?;
            }
            let decision = apply_mac_observation(
                &screen.fields,
                before.as_ref(),
                &mut after,
                previous,
                observed.as_ref().map_err(ToString::to_string),
            );
            source = decision.source.into();
            mac_message = decision.message;
            confirmation = decision.confirmation.map(String::from);
            errors.extend(decision.errors);
        }
        let needs_space = before
            .as_ref()
            .is_some_and(|b| b.space_id != after.space_id);
        let mut duplicates = platform_write::duplicate_ids(
            &context.read,
            &after,
            before.as_ref().map(|_| screen.id.as_str()),
        )
        .await?;
        for other in &snapshot.screens {
            if other.id != screen.id
                && other.source == "local"
                && (other.fields.ip == after.ip
                    || (valid_mac(&after.mac)
                        && normalize_mac(&other.fields.mac) == normalize_mac(&after.mac)))
            {
                duplicates.push(other.id.clone());
            }
        }
        duplicates.sort();
        duplicates.dedup();
        if !duplicates.is_empty() {
            errors.push("IP 或 MAC 存在其他记录，请先核对重复资料".into());
        }
        let differences = diffs(before.as_ref(), &after, &snapshot.spaces);
        let state_name = if !errors.is_empty() {
            "blocked"
        } else if before.is_some() && differences.is_empty() {
            "skip"
        } else {
            "ready"
        };
        let reason = if !errors.is_empty() {
            errors.join("；")
        } else if state_name == "skip" {
            "平台资料无变化，无需提交".into()
        } else if confirmation.is_some() {
            "资料检查通过，提交前需确认 MAC 处理方式".into()
        } else {
            "资料检查通过，请核对后提交".into()
        };
        items.push(RegistrationItem {
            screen_id: screen.id.clone(),
            mode: if before.is_some() { "update" } else { "create" }.into(),
            state: state_name.into(),
            reason,
            before,
            after,
            diffs: differences,
            expected_revision: screen.revision,
            needs_space_confirmation: needs_space,
            mac_source: source,
            mac_message,
            required_mac_confirmation: confirmation,
            duplicate_ids: duplicates,
        });
        originals.insert(screen.id.clone(), screen.clone());
    }
    for index in 0..items.len() {
        let duplicate = items.iter().enumerate().any(|(j, other)| {
            j != index
                && (other.after.ip == items[index].after.ip
                    || (valid_mac(&items[index].after.mac)
                        && normalize_mac(&other.after.mac)
                            == normalize_mac(&items[index].after.mac)))
        });
        if duplicate {
            items[index].state = "blocked".into();
            items[index].reason = "本批次待提交资料存在重复 IP 或 MAC，请先核对".into();
        }
    }
    let preview = RegistrationPreview {
        id: id.clone(),
        project_id: project.into(),
        created_at: now(),
        items,
    };
    let targets = preview
        .items
        .iter()
        .filter(|i| i.state == "ready")
        .map(|i| originals[&i.screen_id].clone())
        .collect::<Vec<_>>();
    if !targets.is_empty() {
        let data = RegistrationPlan {
            preview: preview.clone(),
            original_assets: originals,
            draft_revisions: revisions,
            request_ids: BTreeMap::new(),
            platform_ids: BTreeMap::new(),
            confirmations: None,
            merge_versions: BTreeMap::new(),
            merge_sources: BTreeMap::new(),
        };
        let plan = ScreenPlan {
            project_id: project.into(),
            input: ScreenOperationInput {
                action: "register".into(),
                target_ids: targets.iter().map(|s| s.id.clone()).collect(),
                application_id: None,
                apk: None,
                app_version: String::new(),
                abi: String::new(),
                reinstall: false,
                concurrency: 1,
                retry_of_operation_id: None, expected_targets: BTreeMap::new(),
            },
            targets,
            business_project_id: Some(context.business),
            data_source_id: Some(context.source),
            operator: context.operator,
            instance_id: application_instance_id().into(),
            created_at: now(),
            detail: serde_json::to_value(data)
                .map_err(|_| AppError::InvalidConfig("保存注册检查失败".into()))?,
        };
        previews::save(state, &id, &plan, PREVIEW).await?;
    }
    Ok(preview)
}
pub async fn submit(
    state: &FormalAppState,
    project: &str,
    input: RegistrationSubmission,
) -> AppResult<String> {
    let context = write_context::open(state, project).await?;
    let _guard = tasks::submission_lock().lock().await;
    let (mut plan, hash) =
        task_data::read_preflight_plan(state.local_store.pool(), project, &input.preview_id).await?;
    if plan.input.action != "register"
        || plan.business_project_id.as_deref() != Some(&context.business)
        || plan.data_source_id.as_deref() != Some(&context.source)
    {
        return Err(AppError::Conflict("注册检查不属于当前项目".into()));
    }
    let mut data = detail(&plan)?;
    let selected = input.screen_ids.iter().cloned().collect::<BTreeSet<_>>();
    if selected.is_empty() || selected.len() != input.screen_ids.len() {
        return Err(AppError::InvalidConfig("请选择不重复的登记目标".into()));
    }
    for id in &selected {
        let item = data
            .preview
            .items
            .iter()
            .find(|i| &i.screen_id == id && i.state == "ready")
            .ok_or_else(|| AppError::Conflict("只能提交本次检查通过的屏".into()))?;
        if item
            .required_mac_confirmation
            .as_ref()
            .is_some_and(|c| input.mac_confirmations.get(id) != Some(c))
        {
            return Err(AppError::Conflict(
                "请逐屏确认 MAC 采集失败的处理方式".into(),
            ));
        }
        if item.needs_space_confirmation && !input.space_confirmations.contains(id) {
            return Err(AppError::Conflict("请确认该屏的空间变更".into()));
        }
        local(state).require_idle(project, id).await?;
        let snapshot = local(state).snapshot(project).await?;
        let current = snapshot
            .screens
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| AppError::NotFound("屏资料已变化".into()))?;
        if current.fields != data.original_assets[id].fields
            || current.source != data.original_assets[id].source
            || current.revision != item.expected_revision
        {
            return Err(AppError::Conflict(
                "本机资料或平台缓存已变化，请重新检查".into(),
            ));
        }
        if snapshot
            .platform_drafts
            .get(id)
            .map(|d| d.revision)
            .unwrap_or(0)
            != data.draft_revisions.get(id).copied().unwrap_or(0)
        {
            return Err(AppError::Conflict("草稿已变化，请重新检查".into()));
        }
        data.request_ids
            .insert(id.clone(), uuid::Uuid::now_v7().to_string());
        data.platform_ids.insert(
            id.clone(),
            if item.mode == "create" {
                platform_write::allocate_id(&context.read).await?
            } else {
                id.clone()
            },
        );
    }
    plan.targets.retain(|s| selected.contains(&s.id));
    plan.input.target_ids = input.screen_ids.clone();
    plan.operator = context.operator;
    data.confirmations = Some(input.clone());
    plan.detail = serde_json::to_value(data)
        .map_err(|_| AppError::InvalidConfig("保存登记确认失败".into()))?;
    previews::queue(state, &input.preview_id, PREVIEW, &hash, &plan).await
}
pub fn register(registry: &TaskHandlerRegistry, app: AppHandle) -> AppResult<()> {
    for action in ["register", "merge"] {
        let app = app.clone();
        registry.register(tasks::DOMAIN, action, move |envelope, cancel| {
            let app = app.clone();
            async move { run(&app.state::<FormalAppState>(), envelope, cancel).await }
        })?;
    }
    Ok(())
}
async fn start_shared(
    state: &FormalAppState, task: &str, plan: &ScreenPlan, data: &RegistrationPlan,
) -> AppResult<(ScreenSharedResults, String)> {
    let context = write_context::open(state, &plan.project_id).await?;
    let targets: Vec<String> = plan.targets.iter()
        .map(|s| format!("{}:{}", context.business, data.platform_ids[&s.id])).collect();
    let shared = ScreenSharedResults::new(context.shared.clone());
    let original = state.task_repository.get(task).await?;
    shared.start(&SharedScreenOperation { id:task, business_project_id:&context.business,
        action:&plan.input.action,name:action_label(&plan.input.action),operator:&plan.operator,
        instance_id:&plan.instance_id,targets:&targets,started_at:original.started_at.as_deref().or(Some(&original.created_at)) }).await?;
    Ok((shared, context.business))
}
async fn save_shared(
    state: &FormalAppState,
    task: &str,
    plan: &ScreenPlan,
    data: &RegistrationPlan,
    results: &mut ScreenResults,
) -> AppResult<()> {
    if results
        .targets
        .values()
        .any(|r| matches!(r.business, ResultState::Pending | ResultState::Unknown))
    {
        return Err(AppError::Conflict(
            "仍有平台结果待核实，尚未形成最终共享记录".into(),
        ));
    }
    let (shared, business) = start_shared(state, task, plan, data).await?;
    for screen in &plan.targets {
        let result = results
            .targets
            .get_mut(&screen.id)
            .ok_or_else(|| AppError::Conflict("缺少登记结果".into()))?;
        let mut remote = result.clone();
        if !remote.evidence.is_object() { remote.evidence = serde_json::json!({}); }
        remote.evidence["targetName"] = serde_json::json!(screen.fields.name);
        remote.evidence["targetIp"] = serde_json::json!(screen.fields.ip);
        remote.screen_id = format!("{}:{}", business, data.platform_ids[&screen.id]);
        shared
            .save_target(
                task,
                &business,
                &remote,
                if result.business == ResultState::Succeeded {
                    "succeeded"
                } else if result.business == ResultState::Skipped {
                    "cancelled"
                } else {
                    "failed"
                },
            )
            .await?;
        if result.business == ResultState::Succeeded {
            let item = data
                .preview
                .items
                .iter()
                .find(|i| i.screen_id == screen.id)
                .ok_or_else(|| AppError::Conflict("缺少原资料检查记录".into()))?;
            let summary = serde_json::json!({"operationId":task,"mode":item.mode,"macSource":item.mac_source,"macConfirmation":data.confirmations.as_ref().and_then(|c|c.mac_confirmations.get(&screen.id)),"mergeSources":data.merge_sources.get(&screen.id),"receipt":result.evidence});
            shared
                .audit(
                    &data.request_ids[&screen.id],
                    &business,
                    &data.platform_ids[&screen.id],
                    &item.mode,
                    &plan.operator,
                    &plan.instance_id,
                    &summary,
                )
                .await?;
        }
        result.shared = ResultState::Succeeded;
    }
    shared.finish(task, &business).await?;
    if state
        .task_repository
        .get(task)
        .await?
        .remote_operation_record_id
        .is_none()
    {
        state.task_repository.link_operation(task, task).await?;
    }
    Ok(())
}
fn final_state(results: &ScreenResults) -> TaskState {
    let success = results
        .targets
        .values()
        .filter(|r| r.business == ResultState::Succeeded)
        .count();
    let cancelled = results
        .targets
        .values()
        .filter(|r| r.business == ResultState::Skipped)
        .count();
    if success == results.targets.len() {
        TaskState::Succeeded
    } else if cancelled == results.targets.len() {
        TaskState::Cancelled
    } else if success > 0 {
        TaskState::PartiallySucceeded
    } else {
        TaskState::Failed
    }
}
async fn finish(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &RegistrationPlan,
    results: &mut ScreenResults,
) -> AppResult<()> {
    let saved = save_shared(state, id, plan, data, results).await;
    if let Err(error) = &saved {
        tasks::event(
            state,
            id,
            None,
            "finalizing_failed",
            &format!("结果仍待核实或补存：{error}"),
            TaskEventLevel::Warn,
        )
        .await?;
    }
    results.finished = true;
    task_data::save_results(state.local_store.pool(), &plan.project_id, id, results).await?;
    let mut current = state.task_repository.get(id).await?.state;
    let next = if saved.is_ok() {
        final_state(results)
    } else {
        TaskState::FinalizingFailed
    };
    if saved.is_ok() {
        state.task_repository.resolve_results(id).await?;
    }
    if current == TaskState::Interrupted {
        state
            .task_repository
            .transition(id, current, TaskState::FinalizingFailed, None, None)
            .await?;
        current = TaskState::FinalizingFailed;
    }
    if current == TaskState::Running && next == TaskState::Cancelled {
        state
            .task_repository
            .transition(id, current, TaskState::Cancelling, None, None)
            .await?;
        current = TaskState::Cancelling;
    }
    if current != next {
        state
            .task_repository
            .transition(
                id,
                current,
                next,
                None,
                Some(if saved.is_ok() {
                    "登记结果已保存"
                } else {
                    "平台或共享结果待核实，请处理原任务，不重新登记"
                }),
            )
            .await?;
    }
    tasks::event(
        state,
        id,
        None,
        "completed",
        if saved.is_ok() {
            "登记结果已保存"
        } else {
            "登记记录仍需核实或补存"
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
    let data = detail(&plan)?;
    state
        .task_repository
        .transition(id, TaskState::Queued, TaskState::Running, None, None)
        .await?;
    let mut results = ScreenResults {
        targets: plan
            .targets
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    ScreenTargetResult {
                        format_version: 1,
                        screen_id: s.id.clone(),
                        device: ResultState::NotRequired,
                        business: ResultState::Pending,
                        shared: ResultState::Pending,
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
                result.message = format!("任务未开始：{error}");
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
                    Some("平台条件不可用，未执行登记"),
                )
                .await?;
            return Ok(());
        }
    };
    start_shared(state, id, &plan, &data).await?;
    let pids = plan
        .targets
        .iter()
        .map(|s| data.platform_ids[&s.id].clone())
        .collect::<Vec<_>>();
    let held = match HeldScreenLeases::acquire(&context, id, &plan.instance_id, &pids, true, false)
        .await
    {
        Ok(held) => held,
        Err(error) => {
            for result in results.targets.values_mut() {
                result.business = ResultState::Failed;
                result.message = error.to_string();
            }
            finish(state, id, &plan, &data, &mut results).await?;
            return Ok(());
        }
    };
    for screen in &plan.targets {
        let item = data
            .preview
            .items
            .iter()
            .find(|i| i.screen_id == screen.id)
            .ok_or_else(|| AppError::Conflict("登记检查记录缺少目标".into()))?;
        let request = &data.request_ids[&screen.id];
        let platform_id = &data.platform_ids[&screen.id];
        let mut result = results.targets[&screen.id].clone();
        let source_changed = if screen.source == "local" {
            local(state)
                .asset(project, &screen.id)
                .await
                .map(|current| current.source != screen.source || current.fields != screen.fields)
                .unwrap_or(true)
        } else {
            false
        };
        if source_changed {
            result.business = ResultState::Failed;
            result.message = "原本机资料已经变化，本次未提交平台，请重新检查".into();
        } else if cancel.is_cancelled() {
            result.business = ResultState::Skipped;
            result.message = "未执行的登记已取消".into();
        } else {
            let intent = WriteIntent {
                request_id: request.clone(),
                business_project_id: context.business.clone(),
                screen_id: screen.id.clone(),
                platform_screen_id: platform_id.clone(),
                operation_type: if item.mode == "create" {
                    "register"
                } else {
                    "update"
                }
                .into(),
                payload: serde_json::json!({"item":item,"taskId":id}),
                state: "prepared".into(),
                result: None,
            };
            local(state).prepare_intent(project, &intent).await?;
            held.valid().await?;
            local(state)
                .update_intent(project, request, "prepared", "submitted", None)
                .await?;
            let mutation = tokio::time::timeout(Duration::from_secs(30), async {
                if item.mode == "create" {
                    platform_write::insert(&context, &held.grants, platform_id, &item.after).await
                } else {
                    platform_write::update(
                        &context,
                        &held.grants,
                        platform_id,
                        item.before.as_ref().expect("update before"),
                        &item.after,
                        data.merge_versions
                            .get(&screen.id)
                            .map(|v| (v.before.as_deref(), v.after.as_deref())),
                        item.mode == "merge",
                    )
                    .await
                }
            })
            .await
            .unwrap_or_else(|_| {
                Err(MutationError::Uncertain(AppError::timeout(
                    "平台登记提交，需核实原请求",
                )))
            });
            match mutation {
                Ok(receipt) => {
                    let evidence = serde_json::to_value(&receipt)
                        .map_err(|_| AppError::InvalidConfig("保存平台回执失败".into()))?;
                    match local(state)
                        .confirm_registration(
                            project,
                            &context.business,
                            &screen.id,
                            screen.source == "local",
                            request,
                            &receipt.after,
                            &item.after,
                            data.draft_revisions.get(&screen.id).copied().unwrap_or(0),
                            &evidence,
                        )
                        .await
                    {
                        Ok(()) => {
                            result.business = ResultState::Succeeded;
                            result.message = success_message(&item.mode).into();
                            if item.mode == "merge" {
                                result.before_app_version =
                                    receipt.before.as_ref().and_then(|s| s.app_version.clone());
                                result.after_app_version = receipt.after.app_version.clone();
                            }
                            result.evidence = evidence;
                        }
                        Err(error) => {
                            result.business = ResultState::Unknown;
                            result.message = format!("平台已提交，本机关联待补存：{error}");
                            result.evidence = evidence;
                        }
                    }
                }
                Err(MutationError::Rejected(error)) => {
                    local(state)
                        .update_intent(
                            project,
                            request,
                            "submitted",
                            "not_applied",
                            Some(&serde_json::json!({"reason":error.to_string()})),
                        )
                        .await?;
                    result.business = ResultState::Failed;
                    result.message = error.to_string();
                }
                Err(MutationError::Uncertain(error)) => {
                    result.business = ResultState::Unknown;
                    result.message = format!("提交结果待核实：{error}");
                }
            }
        }
        task_data::save_target(state.local_store.pool(), project, id, &result).await?;
        let state_value = match result.business {
            ResultState::Succeeded => TargetState::Succeeded,
            ResultState::Skipped => TargetState::Cancelled,
            ResultState::Unknown => TargetState::Unknown,
            _ => TargetState::Failed,
        };
        state
            .task_repository
            .update_target(
                id,
                TargetUpdate {
                    resource_type: tasks::DOMAIN.into(),
                    resource_key: screen.id.clone(),
                    state: state_value,
                    stage: "登记结果".into(),
                    progress_current: 1,
                    progress_total: 1,
                    fencing_token: None,
                    message_code: Some("SCREEN_REGISTRATION_RESULT".into()),
                    message_params_json: Some(
                        serde_json::json!({"summary":result.message}).to_string(),
                    ),
                },
            )
            .await?;
        tasks::event(
            state,
            id,
            Some(&screen.id),
            state_value.as_str(),
            &result.message,
            if state_value == TargetState::Succeeded {
                TaskEventLevel::Info
            } else {
                TaskEventLevel::Warn
            },
        )
        .await?;
        results.targets.insert(screen.id.clone(), result);
    }
    if results
        .targets
        .values()
        .all(|r| !matches!(r.business, ResultState::Pending | ResultState::Unknown))
    {
        let _ = held.release().await;
    }
    finish(state, id, &plan, &data, &mut results).await
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
        let data = detail(&plan)?;
        let context = write_context::open(state, &plan.project_id).await?;
        start_shared(state, &task.id, &plan, &data).await?;
        let pids = plan
            .targets
            .iter()
            .map(|s| data.platform_ids[&s.id].clone())
            .collect::<Vec<_>>();
        let held =
            HeldScreenLeases::acquire(&context, &task.id, &plan.instance_id, &pids, true, true)
                .await?;
        let mut results =
            task_data::read_results(state.local_store.pool(), &plan.project_id, &task.id).await?;
        let intents = local(state).intents(&plan.project_id).await?;
        for screen in &plan.targets {
            let item = data
                .preview
                .items
                .iter()
                .find(|i| i.screen_id == screen.id)
                .ok_or_else(|| AppError::Conflict("原登记请求缺少目标".into()))?;
            let request = &data.request_ids[&screen.id];
            let result =
                results
                    .targets
                    .entry(screen.id.clone())
                    .or_insert_with(|| ScreenTargetResult {
                        format_version: 1,
                        screen_id: screen.id.clone(),
                        device: ResultState::NotRequired,
                        business: ResultState::Pending,
                        shared: ResultState::Pending,
                        ..Default::default()
                    });
            if result.business == ResultState::Succeeded {
                continue;
            }
            let intent = intents.iter().find(|i| &i.request_id == request);
            match intent {
                Some(intent) if intent.state == "confirmed" => {
                    result.business = ResultState::Succeeded;
                    result.message = success_message(&item.mode).into();
                    result.evidence = intent.result.clone().unwrap_or_default();
                    if item.mode == "merge" {
                        if let Some(change) = data.merge_versions.get(&screen.id) {
                            result.before_app_version = change.before.clone();
                            result.after_app_version = change.after.clone();
                        }
                    }
                }
                Some(intent) if intent.state == "submitted" => {
                    held.valid().await?;
                    if let Some(record) =
                        platform_write::record(&context.read, &intent.platform_screen_id).await?
                    {
                        if record.business.as_deref() == Some(&context.business)
                            && field_values(&item.after).iter().all(|(key, value)| {
                                item.before
                                    .as_ref()
                                    .is_some_and(|b| field_values(b)[key] == *value)
                                    || field_values(&record.asset.fields)[key] == *value
                            })
                            && data
                                .merge_versions
                                .get(&screen.id)
                                .is_none_or(|v| v.after == record.asset.app_version)
                        {
                            let receipt = MutationReceipt {
                                before: None,
                                after: record.asset,
                                wrote: false,
                            };
                            let evidence = if result.evidence["after"]["id"].as_str()
                                == Some(receipt.after.id.as_str())
                            {
                                result.evidence.clone()
                            } else {
                                serde_json::to_value(&receipt).map_err(|_| {
                                    AppError::InvalidConfig("保存核实结果失败".into())
                                })?
                            };
                            local(state)
                                .confirm_registration(
                                    &plan.project_id,
                                    &context.business,
                                    &screen.id,
                                    screen.source == "local",
                                    request,
                                    &receipt.after,
                                    &item.after,
                                    data.draft_revisions.get(&screen.id).copied().unwrap_or(0),
                                    &evidence,
                                )
                                .await?;
                            result.business = ResultState::Succeeded;
                            result.message = success_message(&item.mode).into();
                            result.evidence = evidence;
                            if item.mode == "merge" {
                                if let Some(change) = data.merge_versions.get(&screen.id) {
                                    result.before_app_version = change.before.clone();
                                    result.after_app_version = change.after.clone();
                                }
                            }
                        } else {
                            local(state)
                                .update_intent(
                                    &plan.project_id,
                                    request,
                                    "submitted",
                                    "conflict",
                                    Some(&serde_json::json!({"current":record.asset})),
                                )
                                .await?;
                            result.business = ResultState::Failed;
                            result.message =
                                "平台当前资料与原请求不一致，已保留依据，请重新核对或人工合并"
                                    .into();
                        }
                    } else {
                        local(state)
                            .update_intent(
                                &plan.project_id,
                                request,
                                "submitted",
                                "not_applied",
                                None,
                            )
                            .await?;
                        result.business = ResultState::Failed;
                        result.message =
                            "未找到原请求对应记录；未重复插入，请重新检查后再提交".into();
                    }
                }
                Some(intent) if intent.state == "prepared" => {
                    local(state)
                        .update_intent(&plan.project_id, request, "prepared", "not_applied", None)
                        .await?;
                    result.business = ResultState::Failed;
                    result.message = "上次登记尚未提交平台，未重放操作".into();
                }
                _ => {
                    result.business = ResultState::Failed;
                    result.message = "上次登记未完成或条件冲突，请重新检查".into();
                }
            }
            task_data::save_target(state.local_store.pool(), &plan.project_id, &task.id, result)
                .await?;
            state
                .task_repository
                .update_target(
                    &task.id,
                    TargetUpdate {
                        resource_type: tasks::DOMAIN.into(),
                        resource_key: screen.id.clone(),
                        state: if result.business == ResultState::Succeeded {
                            TargetState::Succeeded
                        } else {
                            TargetState::Failed
                        },
                        stage: "核实登记结果".into(),
                        progress_current: 1,
                        progress_total: 1,
                        fencing_token: None,
                        message_code: Some("SCREEN_REGISTRATION_VERIFIED".into()),
                        message_params_json: None,
                    },
                )
                .await?;
        }
        if results
            .targets
            .values()
            .all(|r| !matches!(r.business, ResultState::Pending | ResultState::Unknown))
        {
            let _ = held.release().await;
        }
        finish(state, &task.id, &plan, &data, &mut results).await?;
        if state.task_repository.get(&task.id).await?.state == TaskState::FinalizingFailed {
            return Err(AppError::Conflict(
                "本机结果已保留，仍有记录待核实或补存，请检查连接后重试".into(),
            ));
        }
        Ok(TaskRecoveryOutcome::Completed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(mac: &str) -> ScreenFields {
        ScreenFields {
            name: "测试屏".into(),
            ip: "192.0.2.10".into(),
            mac: mac.into(),
            size: "4".into(),
            space_id: Some("1001".into()),
            location: String::new(),
        }
    }

    fn observation(ip: &str, mac: &str) -> ScreenObservation {
        ScreenObservation {
            id: "observation".into(),
            screen_id: "screen".into(),
            operation_type: "mac".into(),
            observed_ip: ip.into(),
            observed_at: "2026-10-21T01:00:00Z".into(),
            adb_available: true,
            observed_mac: Some(mac.into()),
            mac_candidates: vec![format!("eth0={mac}")],
            ..Default::default()
        }
    }

    #[test]
    fn empty_registered_mac_collects_and_creates_a_real_update_diff() {
        let before = fields("");
        let mut after = before.clone();
        assert!(needs_mac_collection(Some(&before), &after));
        let observed = observation(&after.ip, "02:11:22:33:44:55");
        let decision = apply_mac_observation(&before, Some(&before), &mut after, None, Ok(&observed));
        assert_eq!(decision.source, "collected");
        assert!(decision.errors.is_empty());
        assert!(decision.confirmation.is_none());
        let changes = diffs(Some(&before), &after, &[]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].field, "mac");
        assert_eq!(changes[0].before, "");
        assert_eq!(changes[0].after, "02:11:22:33:44:55");
    }

    #[test]
    fn valid_identity_does_not_reconnect_for_name_or_equivalent_mac_format() {
        let before = fields("02:AB:22:33:44:55");
        let mut after = before.clone();
        after.mac = "02-ab-22-33-44-55".into();
        assert!(!needs_mac_collection(Some(&before), &after));
        assert!(diffs(Some(&before), &after, &[]).is_empty());
        after.name = "另一个名称".into();
        assert!(!needs_mac_collection(Some(&before), &after));
        assert_eq!(diffs(Some(&before), &after, &[])[0].field, "name");
        after.ip = "192.0.2.11".into();
        assert!(needs_mac_collection(Some(&before), &after));
        assert!(needs_mac_collection(None, &before));
    }

    #[test]
    fn failed_collection_uses_only_trusted_same_ip_history_with_confirmation() {
        let before = fields("");
        let screen = ScreenAsset {
            id: "screen".into(),
            aliases: vec!["old-local-id".into()],
            fields: before.clone(),
            ..Default::default()
        };
        let trusted = observation(&before.ip, "02:11:22:33:44:55");
        let mut wrong_ip = observation("192.0.2.11", "02:11:22:33:44:66");
        wrong_ip.observed_at = "2026-10-21T02:00:00Z".into();
        let mut ambiguous = trusted.clone();
        ambiguous.observed_at = "2026-10-21T03:00:00Z".into();
        ambiguous.mac_candidates.push("eth1=02:11:22:33:44:77".into());
        let mut unconnected = trusted.clone();
        unconnected.observed_at = "2026-10-21T04:00:00Z".into();
        unconnected.adb_available = false;
        let mut snapshot = ScreenSnapshot::default();
        snapshot.observations.insert("screen".into(), vec![wrong_ip, ambiguous, unconnected]);
        snapshot.observations.insert("old-local-id".into(), vec![trusted]);
        let previous = historical_mac(&snapshot, &screen, &before.ip);
        assert_eq!(previous.as_deref(), Some("02:11:22:33:44:55"));
        let mut after = before.clone();
        let decision = apply_mac_observation(&before, Some(&before), &mut after, previous, Err("本次离线".into()));
        assert_eq!(decision.source, "history");
        assert_eq!(decision.confirmation, Some("existing"));
        assert_eq!(after.mac, "02:11:22:33:44:55");
        assert!(decision.errors.is_empty());
        assert!(historical_mac(&snapshot, &screen, "192.0.2.12").is_none());
    }

    #[test]
    fn failed_collection_without_history_requires_empty_confirmation() {
        let before = fields("");
        let mut after = before.clone();
        let decision = apply_mac_observation(&before, Some(&before), &mut after, None, Err("本次离线".into()));
        assert_eq!(decision.source, "empty");
        assert_eq!(decision.confirmation, Some("empty"));
        assert!(after.mac.is_empty());
    }

    #[test]
    fn collected_identity_conflict_and_multiple_interfaces_never_fall_back() {
        let before = fields("02:11:22:33:44:55");
        let mut after = before.clone();
        let mut observed = observation(&after.ip, "02:11:22:33:44:66");
        let decision = apply_mac_observation(&before, Some(&before), &mut after, Some(before.mac.clone()), Ok(&observed));
        assert_eq!(decision.source, "conflict");
        assert!(!decision.errors.is_empty());
        assert!(decision.confirmation.is_none());
        assert_eq!(after.mac, before.mac);
        observed.mac_candidates.push("eth1=02:11:22:33:44:77".into());
        let decision = apply_mac_observation(&before, Some(&before), &mut after, Some(before.mac.clone()), Ok(&observed));
        assert_eq!(decision.source, "conflict");
        assert!(decision.errors[0].contains("多个有线网卡"));
        assert!(decision.confirmation.is_none());
    }

    #[test]
    fn historical_conflict_or_wrong_live_ip_cannot_supply_a_new_identity() {
        let before = fields("");
        let mut after = fields("02:11:22:33:44:66");
        let decision = apply_mac_observation(&before, Some(&before), &mut after, Some("02:11:22:33:44:55".into()), Err("本次离线".into()));
        assert_eq!(decision.source, "conflict");
        assert!(decision.confirmation.is_none());
        let observed = observation("192.0.2.11", "02:11:22:33:44:66");
        let decision = apply_mac_observation(&before, Some(&before), &mut after, None, Ok(&observed));
        assert_eq!(decision.source, "conflict");
        assert!(decision.errors[0].contains("采集地址"));
    }

    #[test]
    fn unverified_manual_mac_blocks_registered_update_but_preserves_create_empty_rule() {
        let before = fields("");
        let mut after = fields("02:11:22:33:44:66");
        let wanted = after.mac.clone();
        let decision = apply_mac_observation(&before, Some(&before), &mut after, None, Err("本次离线".into()));
        assert_eq!(decision.source, "conflict");
        assert!(decision.errors[0].contains("尚未通过现场采集"));
        assert!(decision.confirmation.is_none());
        assert_eq!(after.mac, wanted);

        let decision = apply_mac_observation(&before, None, &mut after, None, Err("本次离线".into()));
        assert_eq!(decision.source, "empty");
        assert_eq!(decision.confirmation, Some("empty"));
        assert!(after.mac.is_empty());
    }

    #[test]
    fn mac_without_confirmed_adb_connection_is_not_a_successful_live_collection() {
        let before = fields("");
        let mut after = before.clone();
        let mut observed = observation(&after.ip, "02:11:22:33:44:55");
        observed.adb_available = false;
        let decision = apply_mac_observation(&before, Some(&before), &mut after, None, Ok(&observed));
        assert_eq!(decision.source, "empty");
        assert_eq!(decision.confirmation, Some("empty"));
        assert!(after.mac.is_empty());
        assert!(decision.message.contains("ADB 连接未确认"));
    }
}
