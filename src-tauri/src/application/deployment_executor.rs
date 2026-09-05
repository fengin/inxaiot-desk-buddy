use std::future::Future;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};
use crate::domain::aio::deployment::{DeploymentPlan, DeploymentStep};
use crate::runtime::batch_executor::{TargetExecutionOutcome, execute_in_batches};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeploymentTargetState {
    Succeeded,
    Failed,
    Cancelled,
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentTargetOutcome {
    pub mac: String,
    pub state: DeploymentTargetState,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentExecutionSummary {
    pub targets: Vec<DeploymentTargetOutcome>,
    pub success_count: u32,
    pub failure_count: u32,
    pub cancelled_count: u32,
}

pub async fn execute_deployment_plan<F, Fut>(
    plan: DeploymentPlan,
    cancellation: CancellationToken,
    execute_step: F,
) -> AppResult<DeploymentExecutionSummary>
where
    F: Fn(String, DeploymentStep, CancellationToken) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let steps = Arc::new(plan.steps.clone());
    let worker = Arc::new(execute_step);
    execute_deployment_targets(plan, cancellation, move |mac, token| {
        let steps = steps.clone();
        let worker = worker.clone();
        async move {
            for step in steps.iter().cloned() {
                if token.is_cancelled() {
                    return Err(AppError::Cancelled);
                }
                worker(mac.clone(), step, token.child_token()).await?;
            }
            Ok(())
        }
    })
    .await
}

pub async fn execute_deployment_targets<F, Fut>(
    plan: DeploymentPlan,
    cancellation: CancellationToken,
    worker: F,
) -> AppResult<DeploymentExecutionSummary>
where
    F: Fn(String, CancellationToken) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    let macs = plan.target_macs.clone();
    let results = execute_in_batches(
        macs.clone(),
        usize::try_from(plan.batch_size).unwrap_or(usize::MAX),
        usize::try_from(plan.concurrency).unwrap_or(usize::MAX),
        cancellation,
        worker,
    )
    .await?;
    let targets = results
        .into_iter()
        .map(|result| -> AppResult<DeploymentTargetOutcome> {
            let mac = macs
                .get(result.index)
                .cloned()
                .ok_or_else(|| AppError::Conflict("批处理结果索引超出部署目标范围".into()))?;
            Ok(match result.outcome {
                TargetExecutionOutcome::Completed => DeploymentTargetOutcome {
                    mac,
                    state: DeploymentTargetState::Succeeded,
                    error: None,
                },
                TargetExecutionOutcome::Cancelled => DeploymentTargetOutcome {
                    mac,
                    state: DeploymentTargetState::Cancelled,
                    error: None,
                },
                TargetExecutionOutcome::Failed(error) => DeploymentTargetOutcome {
                    mac,
                    state: DeploymentTargetState::Failed,
                    error: Some(error),
                },
                TargetExecutionOutcome::Panicked => DeploymentTargetOutcome {
                    mac,
                    state: DeploymentTargetState::Panicked,
                    error: Some("节点执行器panic".into()),
                },
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(DeploymentExecutionSummary {
        success_count: count(&targets, DeploymentTargetState::Succeeded),
        failure_count: count(&targets, DeploymentTargetState::Failed)
            + count(&targets, DeploymentTargetState::Panicked),
        cancelled_count: count(&targets, DeploymentTargetState::Cancelled),
        targets,
    })
}

fn count(targets: &[DeploymentTargetOutcome], state: DeploymentTargetState) -> u32 {
    u32::try_from(
        targets
            .iter()
            .filter(|target| target.state == state)
            .count(),
    )
    .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;
    use tokio_util::sync::CancellationToken;

    use crate::domain::aio::deployment::{DeploymentMode, DeploymentPlan, DeploymentPlanInput};

    use super::{DeploymentTargetState, execute_deployment_plan};

    fn plan() -> DeploymentPlan {
        DeploymentPlan::build(DeploymentPlanInput {
            mode: DeploymentMode::FullUpgrade,
            target_macs: vec!["A".into(), "B".into()],
            image_files: vec![crate::domain::aio::deployment::DeploymentImageInput {
                service_name: "device-edge".into(),
                file_path: "C:/device-edge.tar".into(),
                image_tag: "device-edge:1".into(),
            }],
            artifact_path: "C:/release".into(),
            artifact_name: "Release".into(),
            artifact_version: "1".into(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: std::collections::BTreeMap::new(),
            batch_size: 2,
            concurrency: 2,
        })
        .expect("plan")
    }

    #[tokio::test]
    async fn executes_steps_in_order_and_keeps_target_failure_isolated() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let output = execute_deployment_plan(plan(), CancellationToken::new(), {
            let events = events.clone();
            move |mac, step, _token| {
                let events = events.clone();
                async move {
                    events.lock().await.push(format!("{mac}:{}", step.code));
                    if mac == "B" && step.code == "backup" {
                        return Err(crate::core::error::AppError::Conflict(
                            "backup failed".into(),
                        ));
                    }
                    Ok(())
                }
            }
        })
        .await
        .expect("execute");
        assert_eq!(output.success_count, 1);
        assert_eq!(output.failure_count, 1);
        assert_eq!(output.targets[1].state, DeploymentTargetState::Failed);
        assert!(!events.lock().await.iter().any(|event| event == "B:upload"));
    }
}
