use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::{AbortHandle, JoinHandle};
use tokio_util::sync::CancellationToken;

use crate::core::error::{AppError, AppResult};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    Completed,
    Cancelled,
    Failed(String),
    Panicked,
    Aborted,
}

struct JobEntry {
    cancellation: CancellationToken,
    handle: Option<JoinHandle<AppResult<()>>>,
    abort_handle: Option<AbortHandle>,
}

#[derive(Clone, Default)]
pub struct JobSupervisor {
    jobs: Arc<RwLock<HashMap<String, JobEntry>>>,
}

impl JobSupervisor {
    pub async fn register(&self, task_id: &str) -> AppResult<CancellationToken> {
        validate_task_id(task_id)?;
        let mut jobs = self.jobs.write().await;
        if jobs.contains_key(task_id) {
            return Err(AppError::Conflict(format!("任务已注册：{task_id}")));
        }
        let cancellation = CancellationToken::new();
        jobs.insert(
            task_id.into(),
            JobEntry {
                cancellation: cancellation.clone(),
                handle: None,
                abort_handle: None,
            },
        );
        Ok(cancellation)
    }

    pub async fn spawn<F, Fut>(&self, task_id: &str, factory: F) -> AppResult<CancellationToken>
    where
        F: FnOnce(CancellationToken) -> Fut + Send + 'static,
        Fut: Future<Output = AppResult<()>> + Send + 'static,
    {
        let cancellation = self.register(task_id).await?;
        let future_cancellation = cancellation.clone();
        let handle = tokio::spawn(factory(future_cancellation));
        let abort_handle = handle.abort_handle();
        let mut jobs = self.jobs.write().await;
        let Some(entry) = jobs.get_mut(task_id) else {
            handle.abort();
            return Err(AppError::NotFound(format!("任务不存在：{task_id}")));
        };
        entry.handle = Some(handle);
        entry.abort_handle = Some(abort_handle);
        Ok(cancellation)
    }

    pub async fn cancel(&self, task_id: &str) -> AppResult<()> {
        let jobs = self.jobs.read().await;
        let entry = jobs
            .get(task_id)
            .ok_or_else(|| AppError::NotFound(format!("任务不存在：{task_id}")))?;
        entry.cancellation.cancel();
        Ok(())
    }

    pub async fn cancel_all(&self) {
        for entry in self.jobs.read().await.values() {
            entry.cancellation.cancel();
        }
    }

    pub async fn contains(&self, task_id: &str) -> bool {
        self.jobs.read().await.contains_key(task_id)
    }

    pub async fn wait_until_finished(&self, task_id: &str) -> AppResult<()> {
        loop {
            let finished = {
                let jobs = self.jobs.read().await;
                let entry = jobs
                    .get(task_id)
                    .ok_or_else(|| AppError::NotFound(format!("任务不存在：{task_id}")))?;
                entry
                    .handle
                    .as_ref()
                    .ok_or_else(|| AppError::Conflict(format!("任务没有执行句柄：{task_id}")))?
                    .is_finished()
            };
            if finished {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    pub async fn join(&self, task_id: &str) -> AppResult<JobOutcome> {
        let (handle, cancellation) = {
            let mut jobs = self.jobs.write().await;
            let entry = jobs
                .get_mut(task_id)
                .ok_or_else(|| AppError::NotFound(format!("任务不存在：{task_id}")))?;
            let handle = entry
                .handle
                .take()
                .ok_or_else(|| AppError::Conflict(format!("任务没有可等待句柄：{task_id}")))?;
            (handle, entry.cancellation.clone())
        };
        let joined = handle.await;
        self.jobs.write().await.remove(task_id);
        Ok(match joined {
            Ok(Ok(())) if cancellation.is_cancelled() => JobOutcome::Cancelled,
            Ok(Ok(())) => JobOutcome::Completed,
            Ok(Err(AppError::Cancelled)) => JobOutcome::Cancelled,
            Ok(Err(error)) => JobOutcome::Failed(error.to_string()),
            Err(error) if error.is_panic() => JobOutcome::Panicked,
            Err(error) if error.is_cancelled() => JobOutcome::Aborted,
            Err(_) => JobOutcome::Failed("任务线程异常结束".into()),
        })
    }

    pub async fn reap_finished(&self) -> Vec<(String, JobOutcome)> {
        let finished = self
            .jobs
            .read()
            .await
            .iter()
            .filter(|(_, entry)| entry.handle.as_ref().is_some_and(JoinHandle::is_finished))
            .map(|(task_id, _)| task_id.clone())
            .collect::<Vec<_>>();
        let mut outcomes = Vec::with_capacity(finished.len());
        for task_id in finished {
            if let Ok(outcome) = self.join(&task_id).await {
                outcomes.push((task_id, outcome));
            }
        }
        outcomes
    }

    pub async fn shutdown(&self, timeout: Duration) -> Vec<(String, JobOutcome)> {
        self.cancel_all().await;
        let deadline = tokio::time::Instant::now() + timeout;
        let task_ids = self.jobs.read().await.keys().cloned().collect::<Vec<_>>();
        let mut outcomes = Vec::with_capacity(task_ids.len());
        for task_id in task_ids {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero()
                && let Some(abort_handle) = self
                    .jobs
                    .read()
                    .await
                    .get(&task_id)
                    .and_then(|entry| entry.abort_handle.clone())
            {
                abort_handle.abort();
            }
            match tokio::time::timeout(remaining, self.join(&task_id)).await {
                Ok(Ok(outcome)) => outcomes.push((task_id, outcome)),
                Ok(Err(_)) => {
                    if self.jobs.write().await.remove(&task_id).is_some() {
                        outcomes.push((task_id, JobOutcome::Cancelled));
                    }
                }
                Err(_) => {
                    if let Some(abort_handle) = self
                        .jobs
                        .read()
                        .await
                        .get(&task_id)
                        .and_then(|entry| entry.abort_handle.clone())
                    {
                        abort_handle.abort();
                    }
                    self.jobs.write().await.remove(&task_id);
                    outcomes.push((task_id, JobOutcome::Aborted));
                }
            }
        }
        outcomes
    }

    pub async fn finish(&self, task_id: &str) -> bool {
        self.jobs.write().await.remove(task_id).is_some()
    }

    pub async fn active_count(&self) -> usize {
        self.jobs.read().await.len()
    }
}

fn validate_task_id(task_id: &str) -> AppResult<()> {
    if task_id.trim().is_empty() {
        Err(AppError::InvalidConfig("任务ID不能为空".into()))
    } else {
        Ok(())
    }
}
