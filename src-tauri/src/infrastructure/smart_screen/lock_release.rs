use super::write_context::{self, ScreenWriteContext};
use crate::{
    core::error::{AppError, AppResult},
    formal::app_state::FormalAppState,
};
use serde::{Deserialize, Serialize};
use sqlx::{MySql, Row, Transaction};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenLock {
    pub resource_type: String,
    pub resource_key: String,
    pub owner_instance_id: String,
    pub owner_user: String,
    pub fencing_token: u64,
}
fn db(e: sqlx::Error) -> AppError {
    AppError::database("处理智能屏操作占用", &e)
}

pub async fn lock_operation(
    tx: &mut Transaction<'_, MySql>,
    business: &str,
    operation: &str,
) -> AppResult<()> {
    let id: Option<String> = sqlx::query_scalar("SELECT id FROM operation_record WHERE id=? AND domain_type='smart_screen' AND business_project_id=? FOR UPDATE")
        .bind(operation).bind(business).fetch_optional(&mut **tx).await.map_err(db)?;
    if id.is_none() {
        return Err(AppError::NotFound("操作记录不属于当前智能屏项目".into()));
    }
    Ok(())
}

pub async fn ensure_not_released(
    tx: &mut Transaction<'_, MySql>,
    operation: &str,
) -> AppResult<()> {
    let found: Option<String> = sqlx::query_scalar("SELECT id FROM audit_event WHERE domain_type='smart_screen' AND object_key=? AND action='force_release' LIMIT 1")
        .bind(operation).fetch_optional(&mut **tx).await.map_err(db)?;
    if found.is_some() {
        return Err(AppError::Conflict(
            "该操作的占用已被人工释放，不能恢复写入；请检查设备后发起新操作".into(),
        ));
    }
    Ok(())
}

pub(crate) async fn locks(
    tx: &mut Transaction<'_, MySql>,
    operation: &str,
) -> AppResult<Vec<ScreenLock>> {
    let rows = sqlx::query("SELECT resource_type,resource_key,owner_instance_id,owner_user,fencing_token FROM resource_lease WHERE operation_id=? AND domain_type='smart_screen' AND resource_type IN ('smart_screen','smart_screen_registry') AND lease_state='active' ORDER BY resource_type,resource_key FOR UPDATE")
        .bind(operation).fetch_all(&mut **tx).await.map_err(db)?;
    rows.into_iter()
        .map(|r| {
            Ok(ScreenLock {
                resource_type: r.try_get("resource_type").map_err(db)?,
                resource_key: r.try_get("resource_key").map_err(db)?,
                owner_instance_id: r.try_get("owner_instance_id").map_err(db)?,
                owner_user: r.try_get("owner_user").map_err(db)?,
                fencing_token: r.try_get("fencing_token").map_err(db)?,
            })
        })
        .collect()
}

pub async fn preview_with_context(
    context: &ScreenWriteContext,
    operation: &str,
) -> AppResult<Vec<ScreenLock>> {
    let mut tx = context.shared.begin().await.map_err(db)?;
    lock_operation(&mut tx, &context.business, operation).await?;
    let result = locks(&mut tx, operation).await?;
    tx.commit().await.map_err(db)?;
    Ok(result)
}

pub async fn release_with_context(
    context: &ScreenWriteContext,
    operation: &str,
    expected: &[ScreenLock],
    confirmed: bool,
    instance: &str,
) -> AppResult<()> {
    if !confirmed || expected.is_empty() {
        return Err(AppError::InvalidConfig(
            "请先查看占用信息并再次确认释放".into(),
        ));
    }
    let mut tx = context.shared.begin().await.map_err(db)?;
    release_in_transaction(&mut tx, context, operation, expected, instance).await?;
    tx.commit().await.map_err(db)
}

pub(crate) async fn release_in_transaction(
    tx: &mut Transaction<'_, MySql>,
    context: &ScreenWriteContext,
    operation: &str,
    expected: &[ScreenLock],
    instance: &str,
) -> AppResult<()> {
    lock_operation(tx, &context.business, operation).await?;
    let current = locks(tx, operation).await?;
    if current != expected {
        return Err(AppError::Conflict(
            "占用已发生变化，请重新查看并确认".into(),
        ));
    }
    let audit = serde_json::json!({"releasedLocks":current,"message":"人工确认释放；设备是否已执行需另行检查"});
    sqlx::query("INSERT INTO audit_event(id,domain_type,object_type,object_key,action,operator_name,instance_id,changed_fields_json,created_at,business_project_id,request_id) VALUES(?,'smart_screen','operation',?,'force_release',?,?,?,UTC_TIMESTAMP(6),?,?)")
        .bind(uuid::Uuid::now_v7().to_string()).bind(operation).bind(&context.operator).bind(instance).bind(audit.to_string()).bind(&context.business).bind(uuid::Uuid::now_v7().to_string())
        .execute(&mut **tx).await.map_err(db)?;
    sqlx::query("UPDATE resource_lease SET lease_state='released',expires_at=UTC_TIMESTAMP(6),heartbeat_at=UTC_TIMESTAMP(6) WHERE operation_id=? AND domain_type='smart_screen' AND resource_type IN ('smart_screen','smart_screen_registry') AND lease_state='active'")
        .bind(operation).execute(&mut **tx).await.map_err(db)?;
    // 已保存的逐屏事实仍保留；释放不等于撤销已提交给 Android 的命令。
    sqlx::query("UPDATE operation_record SET state='interrupted' WHERE id=? AND state='running'")
        .bind(operation)
        .execute(&mut **tx)
        .await
        .map_err(db)?;
    Ok(())
}

pub async fn preview(
    state: &FormalAppState,
    project: &str,
    operation: &str,
) -> AppResult<Vec<ScreenLock>> {
    preview_with_context(&write_context::open(state, project).await?, operation).await
}
pub async fn release(
    state: &FormalAppState,
    project: &str,
    operation: &str,
    expected: &[ScreenLock],
    confirmed: bool,
) -> AppResult<()> {
    release_with_context(
        &write_context::open(state, project).await?,
        operation,
        expected,
        confirmed,
        &crate::infrastructure::client_instance::application_instance_id(),
    )
    .await
}
