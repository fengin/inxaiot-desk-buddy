use crate::core::error::AppResult;
use crate::domain::common::operation_history::*;
use crate::formal::app_state::FormalAppState;
use crate::formal::operation_repository::{
    OperationHistoryRecord, OperationHistoryTargetRecord, OperationRepository,
};
use crate::infrastructure::project_context::project_operator;
use crate::infrastructure::project_context::{map_formal_error, project_database};

pub async fn list_history(
    state: &FormalAppState,
    domain: &str,
    project_id: &str,
    query: &OperationHistoryQuery,
) -> AppResult<OperationHistoryPage> {
    project_operator(state, project_id).await?;
    let pools = project_database(state, project_id).await?;
    let (items, total) = OperationRepository::new(pools.workbench.clone())
        .list_history(
            domain,
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

pub async fn history_detail(
    state: &FormalAppState,
    domain: &str,
    project_id: &str,
    operation_id: &str,
) -> AppResult<OperationHistoryDetail> {
    project_operator(state, project_id).await?;
    let pools = project_database(state, project_id).await?;
    let (operation, targets) = OperationRepository::new(pools.workbench.clone())
        .history_detail(domain, operation_id)
        .await
        .map_err(map_formal_error)?;
    Ok(OperationHistoryDetail {
        operation: map_history(operation),
        targets: targets.into_iter().map(map_history_target).collect(),
    })
}
pub(crate) fn map_history(record: OperationHistoryRecord) -> OperationHistoryItem {
    OperationHistoryItem {
        id: record.id,
        domain_type: record.domain_type,
        operation_type: record.operation_type,
        operation_name: record.operation_name,
        operator_name: record.operator_name,
        instance_id: record.instance_id,
        state: record.state,
        target_count: record.target_count,
        success_count: record.success_count,
        failure_count: record.failure_count,
        cancelled_count: record.cancelled_count,
        artifact_name: record.artifact_name,
        artifact_version: record.artifact_version,
        started_at: record.started_at.to_string(),
        ended_at: record.ended_at.map(|value| value.to_string()),
        result_summary: record.result_summary,
        error_code: record.error_code,
        error_summary: record.error_summary,
    }
}

pub(crate) fn map_history_target(record: OperationHistoryTargetRecord) -> OperationHistoryTarget {
    OperationHistoryTarget {
        details: None,
        resource_type: record.resource_type,
        resource_key: record.resource_key,
        state: record.result_state,
        before_version: record.before_version,
        after_version: record.after_version,
        result_summary: record.result_summary,
        error_code: record.error_code,
        error_summary: record.error_summary,
        completed_at: record.completed_at.map(|value| value.to_string()),
    }
}
