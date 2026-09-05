use crate::application::ports::deployment_workflow::{
    DeploymentTaskQueryPort, OperationHistoryQueryPort,
};
use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment_workflow::{
    DeploymentTaskStepView, DeploymentTaskTargetView, DeploymentTaskView, OperationHistoryDetail,
    OperationHistoryItem, OperationHistoryPage, OperationHistoryQuery, OperationHistoryTarget,
};
use crate::domain::common::task::{
    StepState, TargetState, TaskState, TaskStepRecord, TaskTargetRecord,
};
use crate::formal::app_state::FormalAppState;
use crate::formal::operation_repository::{
    OperationHistoryRecord, OperationHistoryTargetRecord, OperationRepository,
};
use crate::infrastructure::aio_assets_service::project_operator;
use crate::infrastructure::project_context::{map_formal_error, project_database};

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
        let visible_targets = targets
            .iter()
            .filter(|target| {
                task.operation_type != "deployment_preflight"
                    || target.resource_type != "preflight_internal"
            })
            .collect::<Vec<_>>();
        let target_views = visible_targets
            .iter()
            .map(|target| DeploymentTaskTargetView {
                mac: target.resource_key.clone(),
                state: target.state.as_str().into(),
                stage: target.stage.clone(),
                progress: percent(target.progress_current, target.progress_total),
                message_code: target.message_code.clone(),
                message: target_result_message(target, &steps),
                updated_at: target.updated_at.clone(),
            })
            .collect::<Vec<_>>();
        let success_count =
            count_targets(&visible_targets, |state| state == TargetState::Succeeded);
        let failure_count = count_targets(&visible_targets, |state| {
            matches!(
                state,
                TargetState::Failed | TargetState::Interrupted | TargetState::Unknown
            )
        });
        let cancelled_count =
            count_targets(&visible_targets, |state| state == TargetState::Cancelled);
        let completed_count = success_count + failure_count + cancelled_count;
        let progress_current = targets
            .iter()
            .map(|target| target.progress_current)
            .sum::<u64>();
        let progress_total = targets
            .iter()
            .map(|target| target.progress_total)
            .sum::<u64>();
        let progress = if progress_total == 0 {
            0
        } else {
            percent(progress_current, progress_total)
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

fn count_targets(targets: &[&TaskTargetRecord], predicate: impl Fn(TargetState) -> bool) -> u32 {
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

fn target_result_message(target: &TaskTargetRecord, steps: &[TaskStepRecord]) -> Option<String> {
    if let Some(message) = target
        .message_params_json
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return Some(message.into());
    }
    if !matches!(
        target.state,
        TargetState::Failed | TargetState::Interrupted | TargetState::Unknown
    ) {
        return None;
    }
    steps
        .iter()
        .rev()
        .find(|step| {
            step.resource_type.as_deref() == Some(target.resource_type.as_str())
                && step.resource_key.as_deref() == Some(target.resource_key.as_str())
                && matches!(step.state, StepState::Failed | StepState::Interrupted)
        })
        .and_then(|step| step.message.clone())
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
    use crate::domain::common::task::{StepState, TargetState, TaskStepRecord, TaskTargetRecord};

    use super::{percent, target_result_message};

    #[test]
    fn task_progress_is_bounded() {
        assert_eq!(percent(3, 4), 75);
        assert_eq!(percent(8, 4), 100);
        assert_eq!(percent(0, 0), 0);
    }

    #[test]
    fn failed_target_uses_its_latest_failed_step_message() {
        let target = TaskTargetRecord {
            local_task_id: "task".into(),
            resource_type: "aio".into(),
            resource_key: "000C290B71F4".into(),
            state: TargetState::Failed,
            stage: "failed".into(),
            progress_current: 100,
            progress_total: 100,
            fencing_token: None,
            message_code: None,
            message_params_json: None,
            updated_at: "1".into(),
        };
        let steps = vec![TaskStepRecord {
            id: "step".into(),
            local_task_id: "task".into(),
            resource_type: Some("aio".into()),
            resource_key: Some("000C290B71F4".into()),
            step_code: "backup".into(),
            state: StepState::Failed,
            error_code: Some("DEPLOYMENT_FAILED".into()),
            message: Some("当前一体机不存在可升级的已部署版本，请先执行首次部署".into()),
            started_at: Some("1".into()),
            ended_at: Some("2".into()),
            updated_at: "2".into(),
        }];

        assert_eq!(
            target_result_message(&target, &steps).as_deref(),
            Some("当前一体机不存在可升级的已部署版本，请先执行首次部署")
        );
    }
}
