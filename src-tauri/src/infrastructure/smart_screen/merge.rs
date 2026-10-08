use super::{
    assets_service::ScreenAssetsService, platform_write, previews, task_data, tasks, write_context,
};
use crate::application::ports::smart_screen::ScreenAssetsPort;
use crate::core::error::{AppError, AppResult};
use crate::domain::{
    common::task::TaskState,
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
    local_sqlite::screen_repository::{ScreenRepository, now},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::time::Duration;

pub const PREVIEW: &str = "screen_merge_preview";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeDecision {
    pub kind: String,
    #[serde(default)]
    pub choices: BTreeMap<String, String>,
    #[serde(default)]
    pub identity_confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    pub local_id: String,
    pub platform_id: String,
    pub fields: Value,
    pub task_id: String,
    pub result_pending: bool,
}

fn observations<'a>(
    snapshot: &'a ScreenSnapshot,
    screen: &ScreenAsset,
) -> Vec<&'a ScreenObservation> {
    let mut values = std::iter::once(&screen.id)
        .chain(screen.aliases.iter())
        .flat_map(|id| snapshot.observations.get(id).into_iter().flatten())
        .filter(|o| o.observed_ip == screen.fields.ip)
        .collect::<Vec<_>>();
    values.sort_by(|a, b| b.observed_at.cmp(&a.observed_at));
    values
}
fn mac(snapshot: &ScreenSnapshot, screen: &ScreenAsset) -> String {
    let observed = observations(snapshot, screen)
        .into_iter()
        .find_map(|o| o.observed_mac.clone())
        .unwrap_or_default();
    if screen.source == "platform" && !screen.fields.mac.is_empty() {
        screen.fields.mac.clone()
    } else if !observed.is_empty() {
        observed
    } else {
        screen.fields.mac.clone()
    }
}
fn version(snapshot: &ScreenSnapshot, screen: &ScreenAsset) -> Option<String> {
    if screen.source == "platform" {
        screen.app_version.clone()
    } else {
        observations(snapshot, screen)
            .into_iter()
            .find(|o| o.app_installed.is_some())
            .and_then(|o| o.observed_app_version.clone())
    }
}
pub async fn merge(
    state: &FormalAppState,
    project: &str,
    candidate: Value,
    decision: MergeDecision,
) -> AppResult<Option<MergeResult>> {
    let snapshot = ScreenAssetsService::new(state)
        .snapshot(project, true)
        .await?;
    if !snapshot.platform_available || !snapshot.spaces_available {
        return Err(AppError::Conflict("请恢复平台和空间读取后再合并".into()));
    }
    let supplied_local: ScreenAsset =
        serde_json::from_value(candidate.get("local").cloned().unwrap_or(Value::Null))
            .map_err(|_| AppError::InvalidConfig("本机合并资料不完整".into()))?;
    let supplied_platform: ScreenAsset =
        serde_json::from_value(candidate.get("platform").cloned().unwrap_or(Value::Null))
            .map_err(|_| AppError::InvalidConfig("平台合并资料不完整".into()))?;
    let local = snapshot
        .screens
        .iter()
        .find(|s| s.id == supplied_local.id && s.source == "local")
        .ok_or_else(|| AppError::Conflict("本机记录已经注册、移出或不存在".into()))?;
    let platform = snapshot
        .screens
        .iter()
        .find(|s| s.id == supplied_platform.id && s.source == "platform")
        .ok_or_else(|| AppError::Conflict("平台目标不属于当前有效项目".into()))?;
    if local.fields != supplied_local.fields
        || platform.fields != supplied_platform.fields
        || local.revision != supplied_local.revision
        || platform.app_version != supplied_platform.app_version
    {
        return Err(AppError::Conflict(
            "合并双方资料已变化，请刷新后重新选源".into(),
        ));
    }
    let left_mac = mac(&snapshot, local);
    let right_mac = mac(&snapshot, platform);
    let left_version = version(&snapshot, local);
    if supplied_local.app_version != left_version {
        return Err(AppError::Conflict("本机版本记录已变化，请重新核对".into()));
    }
    if local.fields.ip != platform.fields.ip
        && !(valid_mac(&left_mac)
            && valid_mac(&right_mac)
            && normalize_mac(&left_mac) == normalize_mac(&right_mac))
    {
        return Err(AppError::Conflict(
            "两条记录没有相同 IP 或有效 MAC，不能作为疑似重复合并".into(),
        ));
    }
    let key = format!(
        "{}|{}|{}|{}|{}|{}",
        local.id,
        platform.id,
        local.fields.ip,
        platform.fields.ip,
        normalize_mac(&left_mac),
        normalize_mac(&right_mac)
    );
    let repository = ScreenRepository::new(state.local_store.pool().clone());
    if decision.kind == "ignore" {
        repository.ignore_pair(project, &key).await?;
        return Ok(None);
    }
    let identity_conflict = local.fields.ip == platform.fields.ip
        && valid_mac(&left_mac)
        && valid_mac(&right_mac)
        && normalize_mac(&left_mac) != normalize_mac(&right_mac);
    if decision.kind != "merge" || (identity_conflict && !decision.identity_confirmed) {
        return Err(AppError::Conflict(
            "MAC 存在冲突，请先确认两条记录是同一台物理屏".into(),
        ));
    }
    let context = write_context::open(state, project).await?;
    for field in [
        "name",
        "ip",
        "mac",
        "size",
        "space",
        "location",
        "appVersion",
    ] {
        if !decision
            .choices
            .get(field)
            .is_some_and(|s| s == "local" || s == "platform")
        {
            return Err(AppError::InvalidConfig("每个合并字段都需选择来源".into()));
        }
    }
    let _guard = tasks::submission_lock().lock().await;
    repository.require_idle(project, &local.id).await?;
    repository.require_idle(project, &platform.id).await?;
    let mut after = platform.fields.clone();
    for field in ["name", "ip", "size", "location"] {
        if decision.choices[field] == "local" {
            set_field(
                &mut after,
                field,
                field_values(&local.fields)[field].clone(),
            );
        }
    }
    after.mac = if decision.choices["mac"] == "local" {
        left_mac
    } else {
        platform.fields.mac.clone()
    };
    after.space_id = if decision.choices["space"] == "local" {
        local.fields.space_id.clone()
    } else {
        platform.fields.space_id.clone()
    };
    validate_fields(&after, true)?;
    validate_space(&after, &snapshot.spaces, true)?;
    let adopted_version = if decision.choices["appVersion"] == "local" {
        left_version.clone()
    } else {
        platform.app_version.clone()
    };
    if decision.choices["appVersion"] == "local"
        && adopted_version.is_none()
        && platform.app_version.is_some()
    {
        return Err(AppError::Conflict(
            "本机没有应用版本记录，不能用空值清除平台版本，请选择平台来源".into(),
        ));
    }
    if !platform_write::duplicate_ids(&context.read, &after, Some(&platform.id))
        .await?
        .is_empty()
    {
        return Err(AppError::Conflict(
            "合并后仍与其他平台屏重复，请先核实".into(),
        ));
    }
    let preview_id = uuid::Uuid::now_v7().to_string();
    let request_id = uuid::Uuid::now_v7().to_string();
    let preview = RegistrationPreview {
        id: preview_id.clone(),
        project_id: project.into(),
        created_at: now(),
        items: vec![RegistrationItem {
            screen_id: local.id.clone(),
            mode: "merge".into(),
            state: "ready".into(),
            reason: "已按双方当前资料确认字段来源".into(),
            before: Some(platform.fields.clone()),
            after: after.clone(),
            diffs: vec![],
            expected_revision: local.revision,
            needs_space_confirmation: false,
            mac_source: "history".into(),
            mac_message: "人工确认同一设备并选择资料来源".into(),
            required_mac_confirmation: None,
            duplicate_ids: vec![],
        }],
    };
    let mut merge_versions = BTreeMap::new();
    if adopted_version != platform.app_version {
        merge_versions.insert(
            local.id.clone(),
            VersionChange {
                before: platform.app_version.clone(),
                after: adopted_version.clone(),
            },
        );
    }
    let data = RegistrationPlan {
        preview,
        original_assets: BTreeMap::from([
            (local.id.clone(), local.clone()),
            (platform.id.clone(), platform.clone()),
        ]),
        draft_revisions: BTreeMap::new(),
        request_ids: BTreeMap::from([(local.id.clone(), request_id)]),
        platform_ids: BTreeMap::from([(local.id.clone(), platform.id.clone())]),
        confirmations: None,
        merge_versions,
        merge_sources: BTreeMap::from([(
            local.id.clone(),
            json!({"choices":decision.choices,"identityConfirmed":true,"localId":local.id,"platformId":platform.id}),
        )]),
    };
    let plan = ScreenPlan {
        project_id: project.into(),
        input: ScreenOperationInput {
            action: "merge".into(),
            target_ids: vec![local.id.clone()],
            application_id: None,
            apk: None,
            app_version: String::new(),
            abi: String::new(),
            reinstall: false,
            concurrency: 1,
            retry_of_operation_id: None, expected_targets: BTreeMap::new(),
        },
        targets: vec![local.clone()],
        business_project_id: Some(context.business),
        data_source_id: Some(context.source),
        operator: context.operator,
        instance_id: application_instance_id().into(),
        created_at: now(),
        detail: serde_json::to_value(data)
            .map_err(|_| AppError::InvalidConfig("保存合并确认失败".into()))?,
    };
    previews::save(state, &preview_id, &plan, PREVIEW).await?;
    let (_, hash) = task_data::read_plan(state.local_store.pool(), project, &preview_id).await?;
    let task_id = previews::queue(state, &preview_id, PREVIEW, &hash, &plan).await?;
    drop(_guard);
    let finished = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let task = state.task_repository.get(&task_id).await?;
            if task.state.is_terminal() || task.state == TaskState::FinalizingFailed {
                return Ok::<_, AppError>(task);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| {
        AppError::Conflict(format!(
            "合并仍在处理，请在任务面板查看原任务 {task_id}，不要重复提交"
        ))
    })??;
    let result = task_data::read_results(state.local_store.pool(), project, &task_id).await?;
    let target = result
        .targets
        .get(&local.id)
        .ok_or_else(|| AppError::Conflict("请在任务面板核实合并结果".into()))?;
    if target.business != ResultState::Succeeded {
        return Err(AppError::Conflict(target.message.clone()));
    }
    let mut fields = serde_json::to_value(after)
        .map_err(|_| AppError::InvalidConfig("显示合并结果失败".into()))?;
    fields["appVersion"] = json!(adopted_version);
    fields["spacePath"] = json!(
        space_path(&snapshot.spaces, fields["spaceId"].as_str().unwrap_or(""))
            .map(|nodes| nodes
                .iter()
                .map(|n| n.name.as_str())
                .collect::<Vec<_>>()
                .join("/"))
            .unwrap_or_default()
    );
    Ok(Some(MergeResult {
        local_id: local.id.clone(),
        platform_id: platform.id.clone(),
        fields,
        task_id,
        result_pending: finished.state == TaskState::FinalizingFailed,
    }))
}
