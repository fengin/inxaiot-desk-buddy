use super::{
    apk::{self, ApkInfo},
    device::{AdbDevice, AndroidTools},
    leases::HeldScreenLeases,
    platform_write::{self, MutationError},
    previews, task_data, tasks,
    value_updates::{critical_draft, reliable_identity},
    write_context,
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
    project_context::{map_formal_error, project_operator},
    task_recovery::{TaskRecoveryFuture, TaskRecoveryOutcome},
};
use crate::runtime::task_queue::{TaskEnvelope, TaskHandlerRegistry};
use futures_util::{StreamExt, stream};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

pub const PREVIEW: &str = "screen_maintenance_preview";
pub use crate::domain::smart_screen::operation::WRITE_ACTIONS;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenancePlan {
    pub apk: Option<ApkInfo>,
    pub observations: BTreeMap<String, ScreenObservation>,
    pub request_ids: BTreeMap<String, String>,
    #[serde(default)]
    pub app_config: Option<BTreeMap<String, crate::domain::smart_screen::app_config::AppConfigPatch>>,
}
fn local(state: &FormalAppState) -> ScreenRepository {
    ScreenRepository::new(state.local_store.pool().clone())
}
fn detail(plan: &ScreenPlan) -> AppResult<MaintenancePlan> {
    serde_json::from_value(plan.detail.clone())
        .map_err(|_| AppError::Conflict("设备操作检查记录无效".into()))
}
async fn device_idle(state: &FormalAppState, ip: &str, except: Option<&str>) -> AppResult<()> {
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM local_task t JOIN local_screen_task_data d ON d.local_task_id=t.id,json_each(d.plan_json,'$.targets') target WHERE t.domain_type='smart_screen' AND (? IS NULL OR t.id<>?) AND json_extract(target.value,'$.ip')=? AND (t.state IN ('queued','running','cancelling','finalizing_failed') OR EXISTS(SELECT 1 FROM local_task_result_guard g WHERE g.local_task_id=t.id))")
        .bind(except).bind(except).bind(ip).fetch_one(state.local_store.pool()).await.map_err(|e|AppError::database("检查同地址活动任务",&e))?;
    if count > 0 {
        return Err(AppError::Conflict(
            "本机已有任务正在处理该地址，请先完成或核实原任务".into(),
        ));
    }
    Ok(())
}
pub fn compatibility(
    apk: &ApkInfo,
    observation: &ScreenObservation,
    reinstall: bool,
) -> AppResult<bool> {
    let sdk = observation
        .sdk
        .ok_or_else(|| AppError::Conflict("未取得设备系统信息".into()))?;
    if sdk < apk.min_sdk {
        return Err(AppError::Conflict("设备系统低于安装包最低要求".into()));
    }
    if !apk.abis.is_empty() && !apk.abis.iter().any(|abi| observation.abis.contains(abi)) {
        return Err(AppError::Conflict(format!(
            "安装包架构不匹配，设备支持 {}",
            observation.abis.join(" / ")
        )));
    }
    if observation.free_space_mb.is_none() {
        return Err(AppError::Conflict(
            "未取得可用空间，不能确认安装条件".into(),
        ));
    }
    let reserve = (apk.size.saturating_mul(2) / 1024 / 1024).saturating_add(64);
    if observation.free_space_mb.unwrap_or(0) < reserve {
        return Err(AppError::Conflict(format!(
            "可用空间不足，本次至少预留 {reserve} MB；不会自动清理数据"
        )));
    }
    if observation.app_installed == Some(true) {
        let current = observation
            .app_version_code
            .ok_or_else(|| AppError::Conflict("未读到已安装小新的数字版本".into()))?;
        if apk.app_version_code < current {
            return Err(AppError::Conflict(format!(
                "所选包数字版本 {} 低于当前 {}，不自动降级",
                apk.app_version_code, current
            )));
        }
        return Ok(apk.app_version_code == current && !reinstall);
    }
    if observation.app_installed != Some(false) {
        return Err(AppError::Conflict("未确认小新安装状态".into()));
    }
    Ok(false)
}
pub async fn preflight(
    state: &FormalAppState,
    project: &str,
    input: ScreenOperationInput,
) -> AppResult<ScreenPreflight> {
    preflight_with_config(state, project, input, None).await
}
pub async fn preflight_with_config(
    state: &FormalAppState,
    project: &str,
    input: ScreenOperationInput,
    app_config: Option<BTreeMap<String, crate::domain::smart_screen::app_config::AppConfigPatch>>,
) -> AppResult<ScreenPreflight> {
    crate::application::smart_screen::operations::validate_input(&input)?;
    if input.action == "app_config" {
        let patches = app_config.as_ref().ok_or_else(||AppError::InvalidConfig("请先读取小新配置并选择修改内容".into()))?;
        if patches.keys().collect::<BTreeSet<_>>() != input.target_ids.iter().collect() {
            return Err(AppError::InvalidConfig("配置修改内容与所选屏不一致".into()));
        }
        let mut environments = BTreeSet::new();
        for patch in patches.values() {
            patch.validate()?;
            if patch.fields().is_empty() { return Err(AppError::InvalidConfig("请至少选择一项修改".into())); }
            for field in patch.fields() {
                if field.split('.').count() == 3 { environments.insert(field.split('.').nth(1).unwrap().to_string()); }
            }
            if let Some(env) = patch.set.get("environments.current").and_then(Value::as_str) { environments.insert(env.to_string()); }
        }
        if environments.len() > 1 { return Err(AppError::InvalidConfig("一批配置只修改一套指定环境".into())); }
    } else if app_config.is_some() { return Err(AppError::InvalidConfig("此操作不接受小新配置内容".into())); }
    tasks::validate_retry(state, project, &input).await?;
    let snapshot = local(state).snapshot(project).await?;
    let selected = input
        .target_ids
        .iter()
        .map(|id| {
            snapshot
                .screens
                .iter()
                .find(|s| &s.id == id)
                .cloned()
                .ok_or_else(|| AppError::NotFound("所选屏已经变化".into()))
        })
        .collect::<AppResult<Vec<_>>>()?;
    if input.action == "install" {
        let sizes = selected
            .iter()
            .map(|s| s.fields.size.as_str())
            .collect::<BTreeSet<_>>();
        if sizes.len() != 1 || sizes.contains("unknown") {
            return Err(AppError::InvalidConfig(
                "一批安装只能选择同一种明确尺寸，包括当前筛选隐藏的目标".into(),
            ));
        }
        if input.application_id.as_deref() != Some("xiaoxin") {
            return Err(AppError::InvalidConfig("首期只支持安装智能小新".into()));
        }
    }
    let id = uuid::Uuid::now_v7().to_string();
    let parsed = if input.action == "install" {
        let metadata = input
            .apk
            .as_ref()
            .ok_or_else(|| AppError::InvalidConfig("请先选择小新安装包".into()))?;
        let path = metadata
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidConfig("请通过文件选择读取真实 APK".into()))?;
        let expected = metadata
            .get("sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::InvalidConfig("安装包尚未解析，请重新选择".into()))?;
        let info = apk::inspect(Path::new(path), CancellationToken::new()).await?;
        if info.sha256 != expected {
            return Err(AppError::Conflict(
                "所选文件已变化，请重新选择安装包".into(),
            ));
        }
        Some(info)
    } else {
        None
    };
    let context = if selected.iter().any(|s| s.source == "platform") {
        Some(write_context::open(state, project).await)
    } else {
        None
    };
    if let Some(Err(error)) = &context {
        return Err(AppError::Conflict(format!("本批次包含平台注册屏，平台连接、登录或共享记录不可用，整批未执行：{error}。如仅处理本机未注册屏，请单独选择这些屏后重新操作。")));
    }
    let tools = AndroidTools::discover()?;
    let device = AdbDevice::new(tools);
    let mut items = Vec::new();
    let mut targets = Vec::new();
    let mut observations = BTreeMap::new();
    for screen in selected {
        let mut reason = None;
        if let Err(error) = local(state).require_idle(project, &screen.id).await {
            reason = Some(error.to_string());
        }
        if let Err(error) = device_idle(state, &screen.fields.ip, None).await {
            reason = Some(error.to_string());
        }
        if critical_draft(&snapshot, &screen) {
            reason = Some("IP、MAC 或尺寸有待提交修改，请先处理".into());
        }
        if snapshot.screens.iter().any(|other| {
            other.id != screen.id
                && (other.fields.ip == screen.fields.ip
                    || (valid_mac(&screen.fields.mac)
                        && normalize_mac(&other.fields.mac) == normalize_mac(&screen.fields.mac)))
        }) {
            reason = Some("当前项目有相同 IP 或 MAC 的记录，请先核对合并，避免重复操作".into());
        }
        if screen.source == "platform" {
            match context.as_ref().expect("platform context") {
                Err(error) => reason = Some(format!("平台写操作条件不可用：{error}")),
                Ok(ctx) => match platform_write::record(&ctx.read, &screen.id).await? {
                    Some(record)
                        if record.business.as_deref() == Some(&ctx.business)
                            && crate::application::smart_screen::operations::same_target(
                                &screen,
                                &record.asset,
                            ) => {}
                    _ => reason = Some("平台地址或设备身份已变化，请刷新后重新检查".into()),
                },
            }
        }
        let mut skip = false;
        if reason.is_none() {
            let inspected = device
                .inspect(&screen, "inspect", CancellationToken::new())
                .await;
            match inspected {
                Err(error) => reason = Some(error.to_string()),
                Ok(observation) => {
                    local(state)
                        .append_observation(project, &observation)
                        .await?;
                    if !reliable_identity(&screen, &observation) {
                        reason = Some("未取得可靠的设备身份，不能执行设备写操作".into());
                    } else if let Some(apk) = &parsed {
                        match compatibility(apk, &observation, input.reinstall) {
                            Err(error) => reason = Some(error.to_string()),
                            Ok(same) => skip = same,
                        }
                    }
                    if reason.is_none() && input.action == "app_config" {
                        let patch = &app_config.as_ref().expect("checked config")[&screen.id];
                        match super::app_config::read_device(&device, &screen.fields.ip, Some(patch)).await {
                            Err(error) => reason = Some(error.to_string()),
                            Ok((_, before)) if patch.restart_required(&before) => {
                                if let Err(error) = super::device_maintenance::capability(&device, &screen, &observation, "restart").await {
                                    reason = Some(error.to_string());
                                }
                            }
                            _ => {}
                        }
                    }
                    if reason.is_none() && parsed.is_none() && input.action != "app_config" {
                        if let Err(error) = super::device_maintenance::capability(
                            &device,
                            &screen,
                            &observation,
                            &input.action,
                        )
                        .await
                        {
                            reason = Some(error.to_string());
                        }
                    }
                    observations.insert(screen.id.clone(), observation);
                }
            }
        }
        let state_name = if reason.is_some() {
            "blocked"
        } else if skip {
            "skip"
        } else {
            "ready"
        };
        items.push(ScreenPreflightItem {
            observation: observations.get(&screen.id).cloned(),
            screen_id: screen.id.clone(),
            name: screen.fields.name.clone(),
            ip: screen.fields.ip.clone(),
            state: state_name.into(),
            reason: reason.unwrap_or_else(|| {
                if skip {
                    "已经是目标数字版本，无需重复安装".into()
                } else if parsed.is_some() {
                    "设备身份、安装包、系统及签名已通过检查".into()
                } else {
                    "设备身份和本次维护所需条件已通过检查".into()
                }
            }),
        });
        if state_name == "ready" {
            targets.push(screen);
        }
    }
    if !targets.is_empty() {
        let scope = local(state).scope(project).await?;
        let plan = ScreenPlan {
            project_id: project.into(),
            input,
            targets,
            business_project_id: scope.as_ref().map(|s| s.0.clone()),
            data_source_id: scope.map(|s| s.1),
            operator: project_operator(state, project)
                .await
                .unwrap_or_else(|_| "本机操作（未登录平台）".into()),
            instance_id: application_instance_id().into(),
            created_at: now(),
            detail: serde_json::to_value(MaintenancePlan {
                apk: parsed,
                observations,
                request_ids: BTreeMap::new(),
                app_config,
            })
            .map_err(|_| AppError::InvalidConfig("保存设备检查失败".into()))?,
        };
        previews::save(state, &id, &plan, PREVIEW).await?;
    }
    Ok(ScreenPreflight { id, items })
}
pub async fn submit(
    state: &FormalAppState,
    project: &str,
    preview: &str,
    input: ScreenOperationInput,
) -> AppResult<String> {
    crate::application::smart_screen::operations::validate_input(&input)?;
    let _guard = tasks::submission_lock().lock().await;
    let (mut plan, hash) = task_data::read_preflight_plan(state.local_store.pool(), project, preview).await?;
    let mut comparable = input.clone();
    comparable.target_ids = plan.input.target_ids.clone();
    comparable.expected_targets = plan.input.expected_targets.clone();
    if comparable != plan.input {
        return Err(AppError::Conflict("操作参数已变化，请重新检查".into()));
    }
    let ids = input.target_ids.iter().cloned().collect::<BTreeSet<_>>();
    if !ids.is_subset(&plan.targets.iter().map(|s| s.id.clone()).collect()) {
        return Err(AppError::Conflict("不能执行未检查通过的目标".into()));
    }
    let snapshot = local(state).snapshot(project).await?;
    for screen in plan.targets.iter().filter(|s| ids.contains(&s.id)) {
        local(state).require_idle(project, &screen.id).await?;
        device_idle(state, &screen.fields.ip, None).await?;
        let current = snapshot
            .screens
            .iter()
            .find(|s| s.id == screen.id)
            .ok_or_else(|| AppError::Conflict("设备资料已变化".into()))?;
        if !crate::application::smart_screen::operations::same_target(screen, current)
            || critical_draft(&snapshot, current)
        {
            return Err(AppError::Conflict("设备关键资料已变化或尚未提交".into()));
        }
    }
    plan.targets.retain(|s| ids.contains(&s.id));
    plan.input.target_ids = input.target_ids;
    let mut data = detail(&plan)?;
    let id = uuid::Uuid::now_v7().to_string();
    for target in &plan.targets {
        data.request_ids
            .insert(target.id.clone(), uuid::Uuid::now_v7().to_string());
    }
    if let Some(package) = &data.apk {
        data.apk = Some(
            apk::stage(
                package,
                &state
                    .paths
                    .project_task_dir(project, &id)
                    .map_err(map_formal_error)?,
            )
            .await?,
        );
    }
    plan.detail = serde_json::to_value(data)
        .map_err(|_| AppError::InvalidConfig("保存设备执行计划失败".into()))?;
    previews::queue_with_id(state, preview, PREVIEW, &hash, &plan, id).await
}
pub fn register(registry: &TaskHandlerRegistry, app: AppHandle) -> AppResult<()> {
    for action in WRITE_ACTIONS {
        let app = app.clone();
        registry.register(tasks::DOMAIN, action, move |envelope, cancel| {
            let app = app.clone();
            async move { run(&app.state::<FormalAppState>(), envelope, cancel).await }
        })?;
    }
    Ok(())
}
async fn target_state(
    state: &FormalAppState,
    id: &str,
    result: &ScreenTargetResult,
    finalized: bool,
) -> AppResult<()> {
    let pending = matches!(result.device, ResultState::Pending | ResultState::Unknown)
        || matches!(result.business, ResultState::Pending | ResultState::Unknown)
        || matches!(result.shared, ResultState::Pending | ResultState::Unknown);
    let current = if pending || !finalized {
        state
            .task_repository
            .targets(id)
            .await?
            .into_iter()
            .find(|t| t.resource_key == result.screen_id)
            .map(|t| {
                if t.progress_total > 0 {
                    t.progress_current.saturating_mul(100) / t.progress_total
                } else {
                    0
                }
            })
            .unwrap_or(0)
    } else {
        100
    };
    state
        .task_repository
        .update_target(
            id,
            TargetUpdate {
                resource_type: tasks::DOMAIN.into(),
                resource_key: result.screen_id.clone(),
                state: if target_succeeded(result) {
                    if finalized {
                        TargetState::Succeeded
                    } else {
                        TargetState::Running
                    }
                } else if target_cancelled(result) {
                    TargetState::Cancelled
                } else if result.device == ResultState::Unknown
                    || result.business == ResultState::Unknown
                {
                    TargetState::Unknown
                } else {
                    TargetState::Failed
                },
                stage: "设备操作结果".into(),
                progress_current: current,
                progress_total: 100,
                fencing_token: None,
                message_code: Some("SCREEN_MAINTENANCE_RESULT".into()),
                message_params_json: Some(json!({"summary":result.message}).to_string()),
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
    let unknown = results.targets.values().any(|r| {
        matches!(r.device, ResultState::Pending | ResultState::Unknown)
            || matches!(r.business, ResultState::Pending | ResultState::Unknown)
    });
    if plan.input.action == "install" && !unknown {
        for screen in &plan.targets {
            if results
                .targets
                .get(&screen.id)
                .is_some_and(|r| r.device == ResultState::Succeeded)
            {
                super::installation_progress::InstallationProgress {
                    state,
                    task: id,
                    screen,
                }
                .report("保存操作记录", 80, "设备操作已完成，正在保存操作结果", true)
                .await?;
            }
        }
    }
    let shared = if unknown {
        Err(AppError::Conflict("有设备或平台结果待核实".into()))
    } else {
        tasks::save_shared(state, id, plan, results).await
    };
    results.finished = true;
    task_data::save_results(state.local_store.pool(), &plan.project_id, id, results).await?;
    for result in results.targets.values() {
        target_state(state, id, result, true).await?;
    }
    let mut current = state.task_repository.get(id).await?.state;
    let next = if shared.is_ok() {
        tasks::completed_state(results)
    } else {
        TaskState::FinalizingFailed
    };
    if shared.is_ok() {
        state.task_repository.resolve_results(id).await?;
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
                Some(if shared.is_ok() {
                    "设备与结果处理已完成"
                } else {
                    "设备或记录结果待核实，不重复执行设备动作"
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
            "逐台结果已保存"
        } else {
            "已有结果保存在本机，请核实原任务"
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
        .initialize_target_progress(id, "等待执行", 100)
        .await?;
    state
        .task_repository
        .transition(id, TaskState::Queued, TaskState::Running, None, None)
        .await?;
    for screen in &plan.targets {
        state
            .task_repository
            .update_target(
                id,
                TargetUpdate {
                    resource_type: tasks::DOMAIN.into(),
                    resource_key: screen.id.clone(),
                    state: TargetState::Pending,
                    stage: "等待执行".into(),
                    progress_current: 0,
                    progress_total: 100,
                    fencing_token: None,
                    message_code: Some("SCREEN_WAITING".into()),
                    message_params_json: Some(json!({"summary":"等待执行"}).to_string()),
                },
            )
            .await?;
    }
    tasks::event(
        state,
        id,
        None,
        "running",
        &format!(
            "开始{}，共 {} 台设备",
            action_label(&plan.input.action),
            plan.targets.len()
        ),
        TaskEventLevel::Info,
    )
    .await?;
    // 在设备写入前保存已注册屏的操作起点，其他电脑能看到未完成操作。
    if let Err(error) = tasks::start_shared(state, id, &plan).await {
        let mut blocked = ScreenResults {
            targets: plan
                .targets
                .iter()
                .map(|screen| {
                    (
                        screen.id.clone(),
                        ScreenTargetResult {
                            format_version: 1,
                            screen_id: screen.id.clone(),
                            device: ResultState::Failed,
                            business: ResultState::NotRequired,
                            shared: if screen.source == "platform" {
                                ResultState::Pending
                            } else {
                                ResultState::NotRequired
                            },
                            message: format!("本批次包含平台注册屏，平台连接、登录或共享记录不可用，整批未执行：{error}。如仅处理本机未注册屏，请单独选择这些屏后重新操作。"),
                            ..Default::default()
                        },
                    )
                })
                .collect(),
            finished: false,
        };
        for result in blocked.targets.values() {
            target_state(state, id, result, false).await?;
        }
        return finish(state, id, &plan, &mut blocked).await;
    }
    let results = ScreenResults {
        targets: plan
            .targets
            .iter()
            .map(|s| {
                (
                    s.id.clone(),
                    ScreenTargetResult {
                        format_version: 1,
                        screen_id: s.id.clone(),
                        device: ResultState::Pending,
                        business: ResultState::NotRequired,
                        shared: if s.source == "platform" {
                            ResultState::Pending
                        } else {
                            ResultState::NotRequired
                        },
                        ..Default::default()
                    },
                )
            })
            .collect(),
        finished: false,
    };
    task_data::save_results(state.local_store.pool(), project, id, &results).await?;
    let requests = plan
        .targets
        .clone()
        .into_iter()
        .map(|screen| {
            let plan = &plan;
            let data = &data;
            let token = cancel.clone();
            async move { execute_target(state, id, plan, data, &screen, token).await }
        })
        .collect::<Vec<_>>();
    let mut pending = stream::iter(requests).buffer_unordered(plan.input.concurrency as usize);
    while let Some(result) = pending.next().await {
        let result = result?;
        task_data::save_target(state.local_store.pool(), project, id, &result).await?;
        target_state(state, id, &result, false).await?;
        tasks::event(
            state,
            id,
            Some(&result.screen_id),
            "result",
            &result.message,
            if target_succeeded(&result) {
                TaskEventLevel::Info
            } else {
                TaskEventLevel::Warn
            },
        )
        .await?;
    }
    let mut results = task_data::read_results(state.local_store.pool(), project, id).await?;
    finish(state, id, &plan, &mut results).await
}
async fn execute_target(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    cancel: CancellationToken,
) -> AppResult<ScreenTargetResult> {
    let mut result = ScreenTargetResult {
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
    };
    if cancel.is_cancelled() {
        result.device = ResultState::Cancelled;
        result.message = "尚未开始的设备已取消".into();
        return Ok(result);
    }
    if plan.input.action == "install" {
        super::installation_progress::InstallationProgress {
            state,
            task: id,
            screen,
        }
        .report(
            "检查安装条件",
            0,
            "正在核对当前设备、应用版本和安装条件",
            true,
        )
        .await?;
    }
    let context = if screen.source == "platform" {
        match write_context::open(state, &plan.project_id).await {
            Ok(ctx) => Some(ctx),
            Err(error) => {
                result.device = ResultState::Failed;
                result.message = format!("平台条件不可用，设备未执行：{error}");
                return Ok(result);
            }
        }
    } else {
        None
    };
    let held = if let Some(ctx) = &context {
        match HeldScreenLeases::acquire(
            ctx,
            id,
            &plan.instance_id,
            &[screen.id.clone()],
            false,
            false,
        )
        .await
        {
            Ok(held) => Some(held),
            Err(error) => {
                result.device = ResultState::Failed;
                result.message = error.to_string();
                return Ok(result);
            }
        }
    } else {
        None
    };
    if let Some(ctx) = &context {
        if let Err(error) = ctx.validate_plan(plan) {
            if let Some(held) = held {
                let _ = held.release().await;
            }
            result.device = ResultState::Failed;
            result.message = error.to_string();
            return Ok(result);
        }
    }
    let execution = if plan.input.action == "install" {
        execute_install(
            state,
            id,
            plan,
            data,
            screen,
            held.as_ref(),
            &mut result,
            cancel,
        )
        .await
    } else if plan.input.action == "app_config" {
        super::app_config::execute(state, id, plan, data, screen, held.as_ref(), &mut result, cancel).await
    } else {
        match data.observations.get(&screen.id) {
            Some(original) => {
                super::device_maintenance::execute(
                    state,
                    id,
                    plan,
                    screen,
                    original,
                    held.as_ref(),
                    &mut result,
                    cancel,
                )
                .await
            }
            None => Err(AppError::Conflict("缺少原设备检查依据".into())),
        }
    };
    if let Err(error) = execution {
        if result.device != ResultState::Unknown {
            result.device = if matches!(error, AppError::Cancelled) {
                ResultState::Cancelled
            } else {
                ResultState::Failed
            };
        }
        result.message = error.to_string();
    }
    if result.device == ResultState::Succeeded && plan.input.action == "install" {
        if let Some(ctx) = &context {
            result.business = ResultState::Pending;
            task_data::save_target(state.local_store.pool(), &plan.project_id, id, &result).await?;
            if let Err(error) = save_installed_version(
                state,
                id,
                plan,
                data,
                screen,
                ctx,
                held.as_ref().expect("registered lease"),
                &mut result,
            )
            .await
            {
                result.business = ResultState::Unknown;
                result.message = format!("设备安装已完成，平台记录待处理：{error}");
            }
        }
    }
    if let Some(held) = held {
        if result.device != ResultState::Unknown && result.business != ResultState::Unknown {
            let _ = held.release().await;
        }
    }
    Ok(result)
}
async fn execute_install(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
) -> AppResult<()> {
    let progress = super::installation_progress::InstallationProgress {
        state,
        task: id,
        screen,
    };
    let package = data
        .apk
        .as_ref()
        .ok_or_else(|| AppError::InvalidConfig("任务缺少安装包".into()))?;
    let observed = data
        .observations
        .get(&screen.id)
        .ok_or_else(|| AppError::Conflict("任务缺少设备检查依据".into()))?;
    let current = local(state).asset(&plan.project_id, &screen.id).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || critical_draft(&local(state).snapshot(&plan.project_id).await?, &current)
    {
        return Err(AppError::Conflict("设备资料已变化，未开始安装".into()));
    }
    if let Some(ctx) = if screen.source == "platform" {
        Some(write_context::open(state, &plan.project_id).await?)
    } else {
        None
    } {
        let record = platform_write::record(&ctx.read, &screen.id)
            .await?
            .ok_or_else(|| AppError::Conflict("平台屏已不存在".into()))?;
        if !crate::application::smart_screen::operations::same_target(screen, &record.asset) {
            return Err(AppError::Conflict(
                "平台地址或身份已变化，未开始安装".into(),
            ));
        }
    }
    if apk::sha256(Path::new(&package.path)).await? != package.sha256 {
        return Err(AppError::Integrity {
            operation: "执行前安装包校验",
        });
    }
    let device = AdbDevice::new(AndroidTools::discover()?);
    let mut before = progress
        .wait(
            "检查设备",
            0,
            "正在核对设备身份及当前应用版本",
            device.inspect(screen, "inspect", cancel.clone()),
        )
        .await?;
    if !reliable_identity(screen, &before)
        || before.observed_mac.as_deref().map(normalize_mac)
            != observed.observed_mac.as_deref().map(normalize_mac)
    {
        return Err(AppError::Conflict("设备身份与检查时不一致，未安装".into()));
    }
    if before.app_installed != observed.app_installed
        || before.app_version_code != observed.app_version_code
        || before.observed_app_version != observed.observed_app_version
    {
        return Err(AppError::Conflict(
            "设备应用版本在确认后已变化，请重新检查".into(),
        ));
    }
    compatibility(package, &before, true)?;
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    if let Some(held) = held {
        held.valid().await?;
    }
    let remote = format!("/data/local/tmp/inx-screen-{}.apk", uuid::Uuid::now_v7());
    let uploaded = progress
        .upload(
            &device,
            &package.path,
            &remote,
            package.size,
            cancel.clone(),
        )
        .await;
    if !uploaded.as_ref().is_ok_and(|output| output.success) {
        let _ = device
            .shell(
                &screen.fields.ip,
                &["rm", "-f", &remote],
                CancellationToken::new(),
            )
            .await;
        return Err(match uploaded {
            Err(error) => error,
            Ok(output) => {
                let reason = state.task_event_pipeline
                    .redact_text(&format!("{} {}", output.stdout, output.stderr));
                let reason: String = reason.trim().chars().take(600).collect();
                AppError::Conflict(if reason.is_empty() {
                    "安装包传输未完成，未开始安装".into()
                } else {
                    format!("安装包传输未完成，未开始安装：{reason}")
                })
            }
        });
    }
    if cancel.is_cancelled() {
        let _ = device
            .shell(
                &screen.fields.ip,
                &["rm", "-f", &remote],
                CancellationToken::new(),
            )
            .await;
        return Err(AppError::Cancelled);
    }
    if let Some(held) = held {
        held.valid().await?;
    }
    let before_stamp = installation_stamp(&device, &screen.fields.ip).await?;
    before.task_id = Some(id.into());
    before.operation_type = "install_before".into();
    local(state)
        .append_observation(&plan.project_id, &before)
        .await?;
    result.before_app_version = before.observed_app_version.clone();
    result.device = ResultState::Unknown;
    result.evidence = json!({"phase":"install_requested","remotePackage":remote,"beforeInstallStamp":before_stamp,"package":{"name":package.name,"version":package.app_version,"versionCode":package.app_version_code,"sha256":package.sha256},"before":before});
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await?;
    // 已发出的安装等待系统结果；取消不终止当前设备的安装。
    let installed = progress
        .wait(
            "安装小新",
            40,
            &format!(
                "正在安装小新{}-{}，保留应用数据；等待系统返回结果",
                package.app_version, package.app_version_code
            ),
            device.adb(
                &screen.fields.ip,
                &["shell", "pm", "install", "-r", "--user", "0", &remote],
                Duration::from_secs(300),
                CancellationToken::new(),
            ),
        )
        .await?;
    if !installed.success
        || !installed
            .stdout
            .lines()
            .any(|line| line.trim() == "Success")
    {
        result.device =
            if installed.stdout.contains("Failure [") || installed.stderr.contains("Failure [") {
                ResultState::Failed
            } else {
                ResultState::Unknown
            };
        if result.device == ResultState::Failed {
            let _ = device
                .shell(
                    &screen.fields.ip,
                    &["rm", "-f", &remote],
                    CancellationToken::new(),
                )
                .await;
        }
        result.evidence["systemResult"] = json!(
            state
                .task_event_pipeline
                .redact_text(&format!("{} {}", installed.stdout, installed.stderr))
        );
        return Err(AppError::Conflict(install_error(
            &installed.stdout,
            &installed.stderr,
        )));
    }
    result.evidence["phase"] = json!("installed");
    result.evidence["afterInstallStamp"] =
        json!(installation_stamp(&device, &screen.fields.ip).await?);
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await?;
    let _ = device
        .shell(
            &screen.fields.ip,
            &["rm", "-f", &remote],
            CancellationToken::new(),
        )
        .await;
    if let Some(held) = held {
        held.valid().await?;
    }
    let started = progress
        .wait(
            "启动小新",
            60,
            "系统已确认安装成功，正在启动小新",
            device.shell(
                &screen.fields.ip,
                &[
                    "am",
                    "start",
                    "-n",
                    &format!("{}/{}", package.package_id, package.activity),
                ],
                CancellationToken::new(),
            ),
        )
        .await?;
    if started.contains("Error:") || started.contains("Exception") {
        result.device = ResultState::Failed;
        return Err(AppError::Conflict(
            "安装完成，但小新启动未通过检查；不自动回写平台版本".into(),
        ));
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut after = progress
        .wait(
            "检查安装结果",
            70,
            "正在读取安装后的版本和运行进程",
            device.inspect(screen, "inspect", CancellationToken::new()),
        )
        .await?;
    after.task_id = Some(id.into());
    after.operation_type = "install".into();
    local(state)
        .append_observation(&plan.project_id, &after)
        .await?;
    result.after_app_version = after.observed_app_version.clone();
    result.observation = Some(after.clone());
    if !reliable_identity(screen, &after)
        || after.observed_app_version.as_deref() != Some(package.app_version.as_str())
        || after.app_version_code != Some(package.app_version_code)
        || after.app_running != Some(true)
    {
        result.device = ResultState::Failed;
        return Err(AppError::Conflict(
            "安装后版本或启动检查未通过；保留实际结果，不自动更新平台版本".into(),
        ));
    }
    result.device = ResultState::Succeeded;
    result.evidence["phase"] = json!("verified");
    result.message = format!("小新 {} 已安装并启动，应用数据保留", package.app_version);
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await?;
    Ok(())
}
async fn installation_stamp(device: &AdbDevice, ip: &str) -> AppResult<String> {
    let path = device
        .shell(
            ip,
            &["pm", "path", super::device::XIAOXIN_PACKAGE],
            CancellationToken::new(),
        )
        .await?;
    let details = device
        .shell(
            ip,
            &["dumpsys", "package", super::device::XIAOXIN_PACKAGE],
            CancellationToken::new(),
        )
        .await?;
    let active = details
        .split("Hidden system packages:")
        .next()
        .unwrap_or(&details);
    let updated = active
        .lines()
        .find(|line| line.trim().starts_with("lastUpdateTime="))
        .unwrap_or("")
        .trim();
    Ok(format!("{path}|{updated}"))
}
fn install_error(stdout: &str, stderr: &str) -> String {
    let text = format!("{stdout} {stderr}");
    if text.contains("UPDATE_INCOMPATIBLE") {
        "系统拒绝安装：签名不兼容，未自动卸载".into()
    } else if text.contains("VERSION_DOWNGRADE") {
        "系统拒绝降级，原应用数据保留".into()
    } else if text.contains("INSUFFICIENT_STORAGE") {
        "系统报告空间不足，未自动清理数据".into()
    } else if text.contains("OLDER_SDK") {
        "安装包要求更高版本的 Android 系统".into()
    } else {
        "系统未确认安装成功，请查看本机结果；不会自动卸载或重装".into()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn apk_rules_preserve_data_and_reject_unsafe_install_conditions() {
        let apk = ApkInfo {
            name: "test.apk".into(),
            path: String::new(),
            size: 10 * 1024 * 1024,
            last_modified: 0,
            sha256: String::new(),
            package_id: "chat.xiaoxin.app".into(),
            app_version: "2.0.9".into(),
            app_version_code: 5019,
            min_sdk: 24,
            abis: vec!["armeabi-v7a".into()],
            activity: String::new(),
        };
        let mut observation = ScreenObservation {
            sdk: Some(29),
            abis: vec!["armeabi-v7a".into()],
            free_space_mb: Some(1024),
            app_installed: Some(false),
            ..Default::default()
        };
        assert!(!compatibility(&apk, &observation, false).unwrap());
        observation.app_installed = Some(true);
        observation.app_version_code = Some(5019);
        assert!(compatibility(&apk, &observation, false).unwrap());
        assert!(!compatibility(&apk, &observation, true).unwrap());
        observation.app_version_code = Some(5018);
        assert!(!compatibility(&apk, &observation, false).unwrap());
        observation.app_version_code = Some(6019);
        assert!(
            compatibility(&apk, &observation, true)
                .unwrap_err()
                .to_string()
                .contains("降级")
        );
        observation.app_installed = Some(false);
        observation.free_space_mb = Some(1);
        assert!(compatibility(&apk, &observation, false).is_err());
        observation.free_space_mb = Some(1024);
        observation.abis = vec!["x86_64".into()];
        assert!(compatibility(&apk, &observation, false).is_err());
        observation.abis = apk.abis.clone();
        observation.sdk = Some(23);
        assert!(compatibility(&apk, &observation, false).is_err());
        assert!(install_error("Failure [INSTALL_FAILED_UPDATE_INCOMPATIBLE]", "").contains("签名"));
    }
}
async fn save_installed_version(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    context: &write_context::ScreenWriteContext,
    held: &HeldScreenLeases,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    super::installation_progress::InstallationProgress {
        state,
        task: id,
        screen,
    }
    .report(
        "登记平台版本",
        80,
        "设备版本和启动已确认，正在登记平台应用版本",
        true,
    )
    .await?;
    project_operator(state, &plan.project_id).await?;
    held.valid().await?;
    let version = result
        .observation
        .as_ref()
        .and_then(|o| o.observed_app_version.clone())
        .ok_or_else(|| AppError::Conflict("未取得安装后的实际版本".into()))?;
    let request = &data.request_ids[&screen.id];
    let intent = WriteIntent {
        request_id: request.clone(),
        business_project_id: context.business.clone(),
        screen_id: screen.id.clone(),
        platform_screen_id: screen.id.clone(),
        operation_type: "install_version".into(),
        payload: json!({"before":screen.app_version,"after":version,"taskId":id}),
        state: "prepared".into(),
        result: None,
    };
    local(state)
        .prepare_intent(&plan.project_id, &intent)
        .await?;
    local(state)
        .update_intent(&plan.project_id, request, "prepared", "submitted", None)
        .await?;
    let grant = held
        .grants
        .iter()
        .find(|g| g.resource_type == "smart_screen")
        .expect("screen grant");
    let updated = tokio::time::timeout(
        Duration::from_secs(30),
        platform_write::set_business_value(
            context,
            grant,
            &screen.id,
            &screen.fields,
            "app_version",
            screen.app_version.as_deref(),
            &version,
        ),
    )
    .await
    .unwrap_or_else(|_| {
        Err(MutationError::Uncertain(AppError::timeout(
            "保存已安装应用版本",
        )))
    });
    match updated {
        Ok(receipt) => {
            let evidence = serde_json::to_value(&receipt)
                .map_err(|_| AppError::InvalidConfig("保存版本回读失败".into()))?;
            local(state)
                .confirm_business_value(
                    &plan.project_id,
                    &context.business,
                    request,
                    &receipt.after,
                    &evidence,
                )
                .await?;
            result.business = ResultState::Succeeded;
            result.evidence["businessReceipt"] = evidence;
            result.message.push_str("；平台版本已登记");
        }
        Err(MutationError::Rejected(error)) => {
            local(state)
                .update_intent(&plan.project_id, request, "submitted", "not_applied", None)
                .await?;
            result.business = ResultState::Failed;
            result.message =
                format!("设备安装已完成，平台版本未更新：{error}；请独立核对版本，不要重装");
        }
        Err(MutationError::Uncertain(error)) => {
            result.business = ResultState::Unknown;
            result.message = format!("设备安装已完成，平台版本结果待核实：{error}");
        }
    }
    Ok(())
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
        let mut results =
            task_data::read_results(state.local_store.pool(), &plan.project_id, &task.id).await?;
        for screen in &plan.targets {
            let result =
                results
                    .targets
                    .entry(screen.id.clone())
                    .or_insert_with(|| ScreenTargetResult {
                        format_version: 1,
                        screen_id: screen.id.clone(),
                        device: ResultState::Unknown,
                        business: ResultState::NotRequired,
                        shared: if screen.source == "platform" {
                            ResultState::Pending
                        } else {
                            ResultState::NotRequired
                        },
                        ..Default::default()
                    });
            if matches!(result.device, ResultState::Pending | ResultState::Unknown)
                && result.evidence.get("phase").is_none()
            {
                result.device = ResultState::Cancelled;
                result.message = "原任务未记录设备动作提交，后续操作已停止".into();
            }
            if plan.input.action != "install" {
                let held = if screen.source == "platform" {
                    let context = write_context::open(state, &plan.project_id).await?;
                    Some(
                        HeldScreenLeases::acquire(
                            &context,
                            &task.id,
                            &plan.instance_id,
                            &[screen.id.clone()],
                            false,
                            true,
                        )
                        .await?,
                    )
                } else {
                    None
                };
                if matches!(result.device, ResultState::Pending | ResultState::Unknown) {
                    if plan.input.action == "app_config" {
                        super::app_config::verify(state, &task.id, &plan, &data, screen, result).await?;
                    } else {
                        super::device_maintenance::verify(state, &task.id, &plan, screen, result).await?;
                    }
                }
                task_data::save_target(
                    state.local_store.pool(),
                    &plan.project_id,
                    &task.id,
                    result,
                )
                .await?;
                target_state(state, &task.id, result, false).await?;
                if let Some(held) = held {
                    held.release().await?;
                }
                continue;
            }
            if matches!(result.device, ResultState::Pending | ResultState::Unknown) {
                let before = data
                    .observations
                    .get(&screen.id)
                    .ok_or_else(|| AppError::Conflict("原检查依据缺失".into()))?;
                let package = data
                    .apk
                    .as_ref()
                    .ok_or_else(|| AppError::Conflict("原安装包信息缺失".into()))?;
                let mut current = AdbDevice::new(AndroidTools::discover()?)
                    .inspect(screen, "inspect", CancellationToken::new())
                    .await?;
                if !reliable_identity(screen, &current)
                    || current.observed_mac.as_deref().map(normalize_mac)
                        != before.observed_mac.as_deref().map(normalize_mac)
                {
                    return Err(AppError::Conflict(
                        "尚未确认原设备身份，保留待核实记录".into(),
                    ));
                }
                current.task_id = Some(task.id.clone());
                current.operation_type = "install_verify".into();
                local(state)
                    .append_observation(&plan.project_id, &current)
                    .await?;
                result.observation = Some(current.clone());
                result.after_app_version = current.observed_app_version.clone();
                let acknowledged = matches!(
                    result.evidence["phase"].as_str(),
                    Some("installed" | "verified")
                );
                let changed = installation_stamp(
                    &AdbDevice::new(AndroidTools::discover()?),
                    &screen.fields.ip,
                )
                .await?;
                let install_seen = acknowledged
                    || result.evidence["beforeInstallStamp"]
                        .as_str()
                        .is_some_and(|stamp| stamp != changed);
                if install_seen
                    && current.observed_app_version.as_deref() == Some(package.app_version.as_str())
                    && current.app_version_code == Some(package.app_version_code)
                    && current.app_running == Some(true)
                {
                    result.device = ResultState::Succeeded;
                    result.message = "已核实安装变化、目标版本及小新进程，未重复安装或启动".into();
                } else if !install_seen && result.evidence["phase"] == "install_requested" {
                    return Err(AppError::Conflict(
                        "当前版本相同，但尚无证据确认原覆盖安装结束，保留待核实记录，不重装".into(),
                    ));
                } else {
                    result.device = ResultState::Failed;
                    result.business = ResultState::NotRequired;
                    result.message =
                        "当前版本或启动状态不满足原目标，已保留实测；未重复执行设备动作".into();
                }
            }
            if result.device == ResultState::Succeeded
                && screen.source == "platform"
                && matches!(
                    result.business,
                    ResultState::Pending | ResultState::Unknown | ResultState::NotRequired
                )
            {
                let context = write_context::open(state, &plan.project_id).await?;
                context.validate_plan(&plan)?;
                let held = HeldScreenLeases::acquire(
                    &context,
                    &task.id,
                    &plan.instance_id,
                    &[screen.id.clone()],
                    false,
                    true,
                )
                .await?;
                let intent = local(state)
                    .intents(&plan.project_id)
                    .await?
                    .into_iter()
                    .find(|i| Some(&i.request_id) == data.request_ids.get(&screen.id));
                let package = data
                    .apk
                    .as_ref()
                    .ok_or_else(|| AppError::Conflict("原安装包信息缺失".into()))?;
                match intent {
                    Some(intent) if intent.state == "confirmed" => {
                        result.business = ResultState::Succeeded;
                        result.evidence["businessReceipt"] = intent.result.unwrap_or_default();
                    }
                    Some(intent) if intent.state == "submitted" => {
                        let mut tx = context
                            .write
                            .begin()
                            .await
                            .map_err(|e| AppError::database("核实原版本提交", &e))?;
                        let grant = held
                            .grants
                            .iter()
                            .find(|g| g.resource_type == "smart_screen")
                            .expect("screen grant");
                        platform_write::guard_lease(&mut tx, &context.shared_schema, grant).await?;
                        let actual = platform_write::record_locked(&mut tx, &screen.id).await?;
                        tx.commit()
                            .await
                            .map_err(|e| AppError::database("完成版本提交核实", &e))?;
                        if let Some(actual) = actual.filter(|r| {
                            r.business.as_deref() == Some(context.business.as_str())
                                && crate::application::smart_screen::operations::same_target(
                                    screen, &r.asset,
                                )
                                && r.asset.app_version.as_deref()
                                    == Some(package.app_version.as_str())
                        }) {
                            let evidence = json!({"after":actual.asset,"verified":true});
                            local(state)
                                .confirm_business_value(
                                    &plan.project_id,
                                    &context.business,
                                    &intent.request_id,
                                    &actual.asset,
                                    &evidence,
                                )
                                .await?;
                            result.business = ResultState::Succeeded;
                            result.evidence["businessReceipt"] = evidence;
                        } else {
                            local(state)
                                .update_intent(
                                    &plan.project_id,
                                    &intent.request_id,
                                    "submitted",
                                    "conflict",
                                    None,
                                )
                                .await?;
                            result.business = ResultState::Failed;
                            result.message="设备安装已完成；平台当前值与原登记不符，未重放旧值，请独立核对版本".into();
                        }
                    }
                    Some(intent) => {
                        if intent.state == "prepared" {
                            local(state)
                                .update_intent(
                                    &plan.project_id,
                                    &intent.request_id,
                                    "prepared",
                                    "not_applied",
                                    None,
                                )
                                .await?;
                        }
                        result.business = ResultState::Failed;
                        result.message =
                            "设备安装已完成；原版本登记未成功，请独立核对版本，无需重装".into();
                    }
                    None => {
                        let current = AdbDevice::new(AndroidTools::discover()?)
                            .inspect(screen, "inspect", CancellationToken::new())
                            .await?;
                        if reliable_identity(screen, &current)
                            && current.observed_app_version.as_deref()
                                == Some(package.app_version.as_str())
                            && current.app_version_code == Some(package.app_version_code)
                            && current.app_running == Some(true)
                        {
                            result.observation = Some(current);
                            save_installed_version(
                                state, &task.id, &plan, &data, screen, &context, &held, result,
                            )
                            .await?;
                        } else {
                            result.business = ResultState::Failed;
                            result.message =
                                "安装后的设备状态已变化，未用旧结果覆盖平台，请重新核对版本".into();
                        }
                    }
                }
                if result.business != ResultState::Unknown {
                    held.release().await?;
                }
            }
            task_data::save_target(state.local_store.pool(), &plan.project_id, &task.id, result)
                .await?;
            target_state(state, &task.id, result, false).await?;
        }
        finish(state, &task.id, &plan, &mut results).await?;
        if state.task_repository.get(&task.id).await?.state == TaskState::FinalizingFailed {
            return Err(AppError::Conflict(
                "仍有结果待处理，原设备动作未重做".into(),
            ));
        }
        Ok(TaskRecoveryOutcome::Completed)
    })
}
