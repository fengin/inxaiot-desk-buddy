use crate::application::aio_assets::project_operator;
use crate::application::ports::deployment_workflow::{
    DeploymentTaskQueryPort, OperationHistoryQueryPort,
};
use crate::application::project_context::{map_formal_error, project_database};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment_workflow::{
    DeploymentTaskStepView, DeploymentTaskTargetView, DeploymentTaskView, OperationHistoryDetail,
    OperationHistoryItem, OperationHistoryPage, OperationHistoryQuery, OperationHistoryTarget,
};
use crate::domain::common::task::{TargetState, TaskState, TaskTargetRecord};
use crate::formal::app_state::FormalAppState;
use crate::formal::operation_repository::{
    OperationHistoryRecord, OperationHistoryTargetRecord, OperationRepository,
};

pub struct Stage75BQueryAdapter<'a> {
    state: &'a FormalAppState,
}

impl<'a> Stage75BQueryAdapter<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
}

impl DeploymentTaskQueryPort for Stage75BQueryAdapter<'_> {
    async fn task(&self, project_id: &str, task_id: &str) -> AppResult<DeploymentTaskView> {
        let task = self.state.task_repository.get(task_id).await?;
        if task.local_project_id != project_id || task.domain_type != "aio" {
            return Err(AppError::NotFound(format!("当前项目没有任务：{task_id}")));
        }
        let targets = self.state.task_repository.targets(task_id).await?;
        let steps = self.state.task_repository.steps(task_id).await?;
        let target_views = targets
            .iter()
            .map(|target| DeploymentTaskTargetView {
                mac: target.resource_key.clone(),
                state: target.state.as_str().into(),
                stage: target.stage.clone(),
                progress: percent(target.progress_current, target.progress_total),
                message_code: target.message_code.clone(),
                message: target.message_params_json.clone(),
                updated_at: target.updated_at.clone(),
            })
            .collect::<Vec<_>>();
        let success_count = count_targets(&targets, |state| state == TargetState::Succeeded);
        let failure_count = count_targets(&targets, |state| {
            matches!(
                state,
                TargetState::Failed | TargetState::Interrupted | TargetState::Unknown
            )
        });
        let cancelled_count = count_targets(&targets, |state| state == TargetState::Cancelled);
        let completed_count = success_count + failure_count + cancelled_count;
        let progress = if target_views.is_empty() {
            0
        } else {
            target_views
                .iter()
                .map(|target| target.progress)
                .sum::<u32>()
                / u32::try_from(target_views.len()).unwrap_or(1)
        };
        let stage = target_views
            .iter()
            .find(|target| target.state == "running")
            .map(|target| target.stage.clone())
            .unwrap_or_else(|| task.state.as_str().into());
        Ok(DeploymentTaskView {
            id: task.id,
            project_id: task.local_project_id,
            operation_id: task.remote_operation_record_id,
            operation_type: task.operation_type,
            name: task.name,
            state: task.state.as_str().into(),
            stage,
            progress,
            target_count: u32::try_from(target_views.len()).unwrap_or(u32::MAX),
            completed_count,
            success_count,
            failure_count,
            cancelled_count,
            cancellable: matches!(task.state, TaskState::Queued | TaskState::Running),
            error_code: task.error_code,
            message: task.message,
            started_at: task.started_at,
            ended_at: task.ended_at,
            updated_at: task.updated_at,
            targets: target_views,
            steps: steps
                .into_iter()
                .map(|step| DeploymentTaskStepView {
                    mac: step.resource_key,
                    code: step.step_code,
                    state: step.state.as_str().into(),
                    error_code: step.error_code,
                    message: step.message,
                    started_at: step.started_at,
                    ended_at: step.ended_at,
                })
                .collect(),
        })
    }
}

impl OperationHistoryQueryPort for Stage75BQueryAdapter<'_> {
    async fn list_history(
        &self,
        project_id: &str,
        query: &OperationHistoryQuery,
    ) -> AppResult<OperationHistoryPage> {
        project_operator(self.state, project_id).await?;
        let pools = project_database(self.state, project_id).await?;
        let (items, total) = OperationRepository::new(pools.workbench.clone())
            .list_history(
                "aio",
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

    async fn history_detail(
        &self,
        project_id: &str,
        operation_id: &str,
    ) -> AppResult<OperationHistoryDetail> {
        project_operator(self.state, project_id).await?;
        let pools = project_database(self.state, project_id).await?;
        let (operation, targets) = OperationRepository::new(pools.workbench.clone())
            .history_detail("aio", operation_id)
            .await
            .map_err(map_formal_error)?;
        Ok(OperationHistoryDetail {
            operation: map_history(operation),
            targets: targets.into_iter().map(map_history_target).collect(),
        })
    }
}

fn count_targets(targets: &[TaskTargetRecord], predicate: impl Fn(TargetState) -> bool) -> u32 {
    u32::try_from(
        targets
            .iter()
            .filter(|target| predicate(target.state))
            .count(),
    )
    .unwrap_or(u32::MAX)
}

fn percent(current: u64, total: u64) -> u32 {
    if total == 0 {
        0
    } else {
        u32::try_from(current.saturating_mul(100) / total)
            .unwrap_or(100)
            .min(100)
    }
}

fn map_history(record: OperationHistoryRecord) -> OperationHistoryItem {
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

fn map_history_target(record: OperationHistoryTargetRecord) -> OperationHistoryTarget {
    OperationHistoryTarget {
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

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn task_progress_is_bounded() {
        assert_eq!(percent(3, 4), 75);
        assert_eq!(percent(8, 4), 100);
        assert_eq!(percent(0, 0), 0);
    }
}
