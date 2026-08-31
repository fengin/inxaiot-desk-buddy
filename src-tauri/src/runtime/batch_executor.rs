use std::future::Future;
use std::sync::Arc;

use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetExecutionOutcome {
    Completed,
    Cancelled,
    Failed(String),
    Panicked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetExecutionResult {
    pub index: usize,
    pub outcome: TargetExecutionOutcome,
}

pub async fn execute_in_batches<T, F, Fut>(
    targets: Vec<T>,
    batch_size: usize,
    concurrency: usize,
    cancellation: CancellationToken,
    worker: F,
) -> AppResult<Vec<TargetExecutionResult>>
where
    T: Send + 'static,
    F: Fn(T, CancellationToken) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = AppResult<()>> + Send + 'static,
{
    if targets.is_empty() || batch_size == 0 || concurrency == 0 || concurrency > batch_size {
        return Err(AppError::InvalidConfig("批次执行参数无效".into()));
    }
    let worker = Arc::new(worker);
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut targets = targets.into_iter().enumerate().peekable();
    let mut results = Vec::new();
    while targets.peek().is_some() {
        if cancellation.is_cancelled() {
            results.extend(targets.map(|(index, _)| TargetExecutionResult {
                index,
                outcome: TargetExecutionOutcome::Cancelled,
            }));
            break;
        }
        let batch = targets.by_ref().take(batch_size).collect::<Vec<_>>();
        let mut handles = Vec::with_capacity(batch.len());
        for (index, target) in batch {
            let worker = worker.clone();
            let semaphore = semaphore.clone();
            let target_cancellation = cancellation.child_token();
            handles.push((
                index,
                tokio::spawn(async move {
                    if target_cancellation.is_cancelled() {
                        return TargetExecutionOutcome::Cancelled;
                    }
                    let permit = tokio::select! {
                        _ = target_cancellation.cancelled() => {
                            return TargetExecutionOutcome::Cancelled;
                        }
                        permit = semaphore.acquire_owned() => permit,
                    };
                    let Ok(_permit) = permit else {
                        return TargetExecutionOutcome::Cancelled;
                    };
                    match worker(target, target_cancellation).await {
                        Ok(()) => TargetExecutionOutcome::Completed,
                        Err(AppError::Cancelled) => TargetExecutionOutcome::Cancelled,
                        Err(error) => TargetExecutionOutcome::Failed(error.to_string()),
                    }
                }),
            ));
        }
        for (index, handle) in handles {
            let outcome = match handle.await {
                Ok(outcome) => outcome,
                Err(error) if error.is_panic() => TargetExecutionOutcome::Panicked,
                Err(_) => TargetExecutionOutcome::Cancelled,
            };
            results.push(TargetExecutionResult { index, outcome });
        }
    }
    results.sort_by_key(|result| result.index);
    Ok(results)
}
