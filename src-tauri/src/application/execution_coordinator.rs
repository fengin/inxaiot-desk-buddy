use tokio_util::sync::CancellationToken;

use crate::core::error::AppResult;

#[allow(async_fn_in_trait)]
pub trait ExecutionLifecyclePort: Send + Sync {
    type Handle: Send;
    type Summary: Clone + Send;
    type Heartbeat: Send;

    async fn start(&self) -> AppResult<Self::Handle>;

    fn start_heartbeat(
        &self,
        handle: &Self::Handle,
        cancellation: CancellationToken,
    ) -> Self::Heartbeat;

    async fn execute(
        &self,
        handle: &Self::Handle,
        cancellation: CancellationToken,
    ) -> AppResult<Self::Summary>;

    async fn stop_heartbeat(
        &self,
        heartbeat: Self::Heartbeat,
        handle: &mut Self::Handle,
    ) -> AppResult<()>;

    async fn finalize(&self, handle: &Self::Handle, summary: &Self::Summary) -> AppResult<()>;

    async fn mark_interrupted(
        &self,
        handle: &Self::Handle,
        error_code: &str,
        message: &str,
    ) -> AppResult<()>;

    async fn mark_finalizing_failed(&self, handle: &Self::Handle, message: &str) -> AppResult<()>;
}

#[derive(Clone, Default)]
pub struct ExecutionCoordinator;

impl ExecutionCoordinator {
    pub async fn run<L>(
        &self,
        lifecycle: &L,
        cancellation: CancellationToken,
    ) -> AppResult<L::Summary>
    where
        L: ExecutionLifecyclePort,
    {
        let mut handle = lifecycle.start().await?;
        let heartbeat = lifecycle.start_heartbeat(&handle, cancellation.clone());
        let summary = match lifecycle.execute(&handle, cancellation).await {
            Ok(summary) => summary,
            Err(error) => {
                let _ = lifecycle.stop_heartbeat(heartbeat, &mut handle).await;
                let _ = lifecycle
                    .mark_interrupted(&handle, "EXECUTION_FAILED", &error.to_string())
                    .await;
                return Err(error);
            }
        };
        if let Err(error) = lifecycle.stop_heartbeat(heartbeat, &mut handle).await {
            let _ = lifecycle
                .mark_interrupted(&handle, "HEARTBEAT_LOST", &error.to_string())
                .await;
            return Err(error);
        }
        if let Err(error) = lifecycle.finalize(&handle, &summary).await {
            let _ = lifecycle
                .mark_finalizing_failed(&handle, &error.to_string())
                .await;
            return Err(error);
        }
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Mutex;

    use super::{ExecutionCoordinator, ExecutionLifecyclePort};
    use crate::core::error::{AppError, AppResult};

    struct RecordingLifecycle {
        events: Arc<Mutex<Vec<&'static str>>>,
        execution_error: bool,
        finalize_error: bool,
    }

    impl ExecutionLifecyclePort for RecordingLifecycle {
        type Handle = u64;
        type Summary = u64;
        type Heartbeat = ();

        async fn start(&self) -> AppResult<Self::Handle> {
            self.events.lock().await.push("start");
            Ok(1)
        }

        fn start_heartbeat(
            &self,
            _handle: &Self::Handle,
            _cancellation: tokio_util::sync::CancellationToken,
        ) -> Self::Heartbeat {
        }

        async fn execute(
            &self,
            _handle: &Self::Handle,
            _cancellation: tokio_util::sync::CancellationToken,
        ) -> AppResult<Self::Summary> {
            self.events.lock().await.push("execute");
            if self.execution_error {
                Err(AppError::Conflict("execute failed".into()))
            } else {
                Ok(2)
            }
        }

        async fn stop_heartbeat(
            &self,
            _heartbeat: Self::Heartbeat,
            _handle: &mut Self::Handle,
        ) -> AppResult<()> {
            self.events.lock().await.push("stop_heartbeat");
            Ok(())
        }

        async fn finalize(
            &self,
            _handle: &Self::Handle,
            _summary: &Self::Summary,
        ) -> AppResult<()> {
            self.events.lock().await.push("finalize");
            if self.finalize_error {
                Err(AppError::Conflict("finalize failed".into()))
            } else {
                Ok(())
            }
        }

        async fn mark_interrupted(
            &self,
            _handle: &Self::Handle,
            _error_code: &str,
            _message: &str,
        ) -> AppResult<()> {
            self.events.lock().await.push("interrupted");
            Ok(())
        }

        async fn mark_finalizing_failed(
            &self,
            _handle: &Self::Handle,
            _message: &str,
        ) -> AppResult<()> {
            self.events.lock().await.push("finalizing_failed");
            Ok(())
        }
    }

    #[tokio::test]
    async fn coordinator_owns_order_and_failure_convergence() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let success = RecordingLifecycle {
            events: events.clone(),
            execution_error: false,
            finalize_error: false,
        };
        assert_eq!(
            ExecutionCoordinator
                .run(&success, Default::default())
                .await
                .expect("success"),
            2
        );
        assert_eq!(
            *events.lock().await,
            vec!["start", "execute", "stop_heartbeat", "finalize"]
        );

        events.lock().await.clear();
        let failed = RecordingLifecycle {
            events: events.clone(),
            execution_error: true,
            finalize_error: false,
        };
        assert!(
            ExecutionCoordinator
                .run(&failed, Default::default())
                .await
                .is_err()
        );
        assert_eq!(
            *events.lock().await,
            vec!["start", "execute", "stop_heartbeat", "interrupted"]
        );

        events.lock().await.clear();
        let finalizing_failed = RecordingLifecycle {
            events: events.clone(),
            execution_error: false,
            finalize_error: true,
        };
        assert!(
            ExecutionCoordinator
                .run(&finalizing_failed, Default::default())
                .await
                .is_err()
        );
        assert_eq!(
            *events.lock().await,
            vec![
                "start",
                "execute",
                "stop_heartbeat",
                "finalize",
                "finalizing_failed"
            ]
        );
    }
}
