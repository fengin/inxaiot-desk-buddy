use super::{
    lock_release::{self, ScreenLock},
    write_context::{self, ScreenWriteContext},
};
use crate::{
    core::error::{AppError, AppResult},
    domain::smart_screen::operation::{READ_ACTIONS, ScreenPlan},
    formal::app_state::FormalAppState,
};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, QueryBuilder, Row};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TakeoverConflict {
    pub operation_id: String,
    pub operation_name: String,
    pub operation_type: String,
    #[serde(default)]
    pub target_count: u32,
    pub owner_user: String,
    pub owner_instance_id: String,
    pub targets: Vec<String>,
    pub locks: Vec<ScreenLock>,
}
fn db(e: sqlx::Error) -> AppError {
    AppError::database("查询其他电脑的操作", &e)
}

pub fn resources(plan: &ScreenPlan) -> Vec<(String, String)> {
    if READ_ACTIONS.contains(&plan.input.action.as_str()) {
        return vec![];
    }
    let Some(business) = &plan.business_project_id else {
        return vec![];
    };
    let registry = ["register", "merge"].contains(&plan.input.action.as_str());
    let mut keys = BTreeSet::new();
    for screen in &plan.targets {
        let platform_id = if registry {
            plan.detail["platformIds"][&screen.id].as_str()
        } else {
            (screen.source == "platform").then_some(screen.id.as_str())
        };
        if let Some(id) = platform_id {
            keys.insert(("smart_screen".into(), format!("{business}:{id}")));
        }
    }
    if registry {
        if let Some(source) = &plan.data_source_id {
            keys.insert(("smart_screen_registry".into(), source.clone()));
        }
    }
    keys.into_iter().collect()
}

pub async fn conflicts(
    context: &ScreenWriteContext,
    keys: &[(String, String)],
) -> AppResult<Vec<TakeoverConflict>> {
    if keys.is_empty() {
        return Ok(vec![]);
    }
    let mut query = QueryBuilder::<MySql>::new(
        "SELECT DISTINCT operation_id FROM resource_lease WHERE domain_type='smart_screen' AND lease_state='active' AND (",
    );
    for (index, (kind, key)) in keys.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        query
            .push("(resource_type=")
            .push_bind(kind)
            .push(" AND resource_key=")
            .push_bind(key)
            .push(")");
    }
    query.push(") ORDER BY operation_id");
    let ids: Vec<String> = query
        .build_query_scalar()
        .fetch_all(&context.shared)
        .await
        .map_err(db)?;
    let mut found = Vec::new();
    for id in ids {
        let mut tx = context.shared.begin().await.map_err(db)?;
        let record=sqlx::query("SELECT operation_name,operation_type,target_count,operator_name,instance_id,business_project_id FROM operation_record WHERE id=? AND domain_type='smart_screen' FOR UPDATE")
            .bind(&id).fetch_optional(&mut *tx).await.map_err(db)?.ok_or_else(||AppError::Conflict("占用缺少对应的操作记录，请在项目操作记录中核对".into()))?;
        if record
            .try_get::<Option<String>, _>("business_project_id")
            .map_err(db)?
            .as_deref()
            != Some(&context.business)
        {
            return Err(AppError::Conflict(
                "平台登记正在由其他业务项目操作，请在对应业务项目处理占用".into(),
            ));
        }
        let locks = lock_release::locks(&mut tx, &id).await?;
        if !locks
            .iter()
            .any(|lock| keys.contains(&(lock.resource_type.clone(), lock.resource_key.clone())))
        {
            continue;
        }
        let mut targets = Vec::new();
        for lock in &locks {
            if lock.resource_type != "smart_screen" {
                continue;
            }
            let screen_id = lock
                .resource_key
                .strip_prefix(&format!("{}:", context.business))
                .ok_or_else(|| AppError::Conflict("原操作包含其他项目的屏，不能接手".into()))?;
            let value: Option<(String, String)> = sqlx::query_as(
                "SELECT name,ip FROM smart_terminal_screen WHERE id=? AND delete_flag=0",
            )
            .bind(screen_id)
            .fetch_optional(&context.read)
            .await
            .map_err(db)?;
            targets.push(
                value
                    .map(|(name, ip)| format!("{name}（{ip}）"))
                    .unwrap_or_else(|| "待登记或已移除的智能屏".into()),
            );
        }
        if targets.is_empty() {
            targets.push("平台登记操作".into());
        }
        found.push(TakeoverConflict {
            operation_id: id,
            operation_name: record.try_get("operation_name").map_err(db)?,
            operation_type: record.try_get("operation_type").map_err(db)?,
            target_count: record.try_get("target_count").map_err(db)?,
            owner_user: record.try_get("operator_name").map_err(db)?,
            owner_instance_id: record.try_get("instance_id").map_err(db)?,
            targets,
            locks,
        });
        tx.commit().await.map_err(db)?;
    }
    Ok(found)
}

/// 提交时只查询冲突，不在编辑、预览或等待用户确认期间加锁。
pub async fn before_submit(state: &FormalAppState, plan: &ScreenPlan) -> AppResult<()> {
    let keys = resources(plan);
    if keys.is_empty() {
        return Ok(());
    }
    let context = write_context::open(state, &plan.project_id).await?;
    context.validate_plan(plan)?;
    let found = conflicts(&context, &keys).await?;
    if !found.is_empty() {
        return Err(AppError::ConfirmationRequired {
            code: "SCREEN_TAKEOVER_REQUIRED",
            details: serde_json::json!({"conflicts":found}),
        });
    }
    Ok(())
}

pub async fn release_with_context(
    context: &ScreenWriteContext,
    expected: &[TakeoverConflict],
    confirmed: bool,
    instance: &str,
) -> AppResult<()> {
    if !confirmed || expected.is_empty() || expected.iter().any(|item| item.locks.is_empty()) {
        return Err(AppError::InvalidConfig("请确认是否接手操作".into()));
    }
    let mut sorted = expected.to_vec();
    sorted.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
    if sorted
        .windows(2)
        .any(|pair| pair[0].operation_id == pair[1].operation_id)
    {
        return Err(AppError::InvalidConfig("接手确认包含重复操作".into()));
    }
    let mut tx = context.shared.begin().await.map_err(db)?;
    for item in &sorted {
        lock_release::lock_operation(&mut tx, &context.business, &item.operation_id).await?;
        let current = lock_release::locks(&mut tx, &item.operation_id).await?;
        // 等待确认期间，对方可以正常完成并释放原锁；不能因此阻止继续。
        // 仍存在的锁必须属于用户刚刚确认的那一组，新增或更换的锁不能顺带释放。
        if !current.iter().all(|lock| item.locks.contains(lock)) {
            return Err(AppError::Conflict("操作锁已变化，请重新提交并确认当前操作人".into()));
        }
        lock_release::release_in_transaction(
            &mut tx,
            context,
            &item.operation_id,
            &current,
            instance,
        )
        .await?;
    }
    tx.commit().await.map_err(db)
}
pub async fn release(
    state: &FormalAppState,
    project: &str,
    expected: &[TakeoverConflict],
    confirmed: bool,
) -> AppResult<()> {
    release_with_context(
        &write_context::open(state, project).await?,
        expected,
        confirmed,
        crate::infrastructure::client_instance::application_instance_id(),
    )
    .await
}
