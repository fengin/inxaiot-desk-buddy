use crate::application::{
    ports::project_access::ProjectAccessPort, project_access::ProjectAccessRequirement,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::common::operation_history::*;
use crate::formal::{app_state::FormalAppState, operation_repository::OperationRepository};
use crate::infrastructure::{
    local_sqlite::screen_repository::ScreenRepository,
    operation_history::{map_history, map_history_target},
    project_context::{map_formal_error, project_database_for_finalization},
    stage75_adapter::Stage75Adapter,
};
use sqlx::Row;
use std::collections::BTreeMap;
async fn context(state: &FormalAppState, project: &str) -> AppResult<(sqlx::MySqlPool, String)> {
    Stage75Adapter::new(state)
        .require_project_access(project, ProjectAccessRequirement::ActiveSession)
        .await?;
    let (business, source) = ScreenRepository::new(state.local_store.pool().clone())
        .scope(project)
        .await?
        .ok_or_else(|| AppError::Conflict("请先确定本机项目对应的业务项目".into()))?;
    let pools = project_database_for_finalization(state, project).await?;
    if super::platform::source_id(&pools.platform).await? != source {
        return Err(AppError::Conflict("当前项目数据源不一致".into()));
    }
    let stored: Option<String> =
        sqlx::query_scalar("SELECT data_source_id FROM workbench_data_source WHERE singleton_id=1")
            .fetch_optional(&pools.workbench)
            .await
            .map_err(|e| AppError::database("核对共享历史数据源", &e))?;
    if stored.as_deref().is_some_and(|value| value != source) {
        return Err(AppError::Conflict("工作台专用库属于另一数据源".into()));
    }
    Ok((pools.workbench.clone(), business))
}
pub async fn list(
    state: &FormalAppState,
    project: &str,
    query: &OperationHistoryQuery,
) -> AppResult<OperationHistoryPage> {
    let (pool, business) = context(state, project).await?;
    let (items, total) = OperationRepository::new(pool)
        .list_history_scoped(
            "smart_screen",
            Some(&business),
            query.operation_type.as_deref(),
            query.state.as_deref(),
            query.page,
            query.page_size,
        )
        .await
        .map_err(map_formal_error)?;
    Ok(OperationHistoryPage {
        items: items.into_iter().map(map_history).collect(),
        total,
        page: query.page,
        page_size: query.page_size,
    })
}
pub async fn detail(
    state: &FormalAppState,
    project: &str,
    id: &str,
) -> AppResult<OperationHistoryDetail> {
    let (pool, business) = context(state, project).await?;
    let (operation, targets) = OperationRepository::new(pool.clone())
        .history_detail_scoped("smart_screen", Some(&business), id)
        .await
        .map_err(map_formal_error)?;
    let rows=sqlx::query("SELECT resource_key,CAST(result_detail_json AS CHAR) AS detail FROM operation_target_result WHERE operation_id=? AND resource_type='smart_screen'").bind(id).fetch_all(&pool).await.map_err(|e|AppError::database("读取屏共享结果详情",&e))?;
    let mut details = BTreeMap::new();
    let retry: Option<String> = sqlx::query_scalar("SELECT retry_of_operation_id FROM operation_record WHERE id=? AND domain_type='smart_screen' AND business_project_id=?")
        .bind(id).bind(&business).fetch_one(&pool).await.map_err(|e|AppError::database("读取重试来源", &e))?;
    for row in rows {
        let key: String = row
            .try_get("resource_key")
            .map_err(|e| AppError::database("读取共享目标", &e))?;
        let value: Option<String> = row
            .try_get("detail")
            .map_err(|e| AppError::database("读取共享结果", &e))?;
        details.insert(
            key,
            value
                .map(|v| {
                    serde_json::from_str(&v)
                        .map_err(|_| AppError::Conflict("共享结果格式无效".into()))
                })
                .transpose()?,
        );
    }
    Ok(OperationHistoryDetail {
        operation: map_history(operation),
        targets: targets
            .into_iter()
            .map(|target| {
                let mut target = map_history_target(target);
                target.details = details.remove(&target.resource_key).flatten();
                if let Some(retry) = &retry {
                    let detail = target.details.get_or_insert_with(|| serde_json::json!({}));
                    detail["retryOfOperationId"] = serde_json::json!(retry);
                }
                target
            })
            .collect(),
    })
}
