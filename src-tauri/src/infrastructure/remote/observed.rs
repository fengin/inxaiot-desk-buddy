use std::sync::Arc;

use crate::application::ports::remote_session::{
    HostKeyPolicy, RemoteAuth, RemoteConnection, RemoteConnector, RemoteTarget,
};
use crate::core::error::{AppError, AppResult};
use crate::infrastructure::local_sqlite::host_key_repository::{
    HostKeyObservation, HostKeyRepository,
};

type ChangeHandler = Arc<dyn Fn(&HostKeyObservation) -> AppResult<()> + Send + Sync>;

/// 产品连接策略：自动采集指纹、变化只通知；SSH用户认证仍由底层连接器执行。
#[derive(Clone)]
pub struct ObservedConnector<C> {
    inner: C,
    repository: HostKeyRepository,
    project_id: String,
    on_change: Option<ChangeHandler>,
}

impl<C: RemoteConnector> ObservedConnector<C> {
    pub fn new(inner: C, repository: HostKeyRepository, project_id: impl Into<String>) -> Self {
        Self {
            inner,
            repository,
            project_id: project_id.into(),
            on_change: None,
        }
    }

    pub fn with_change_handler(mut self, handler: ChangeHandler) -> Self {
        self.on_change = Some(handler);
        self
    }

    pub async fn connect_observed(
        &self,
        target: &RemoteTarget,
        auth: &RemoteAuth,
    ) -> AppResult<(C::Connection, HostKeyObservation)> {
        let session = self
            .inner
            .connect(target, auth, HostKeyPolicy::Capture)
            .await?;
        let observation = match self
            .repository
            .observe(&self.project_id, target, session.host_key())
            .await
        {
            Ok(observation) => observation,
            Err(error) => {
                let _ = session.disconnect().await;
                return Err(error);
            }
        };
        if observation.changed()
            && let Some(handler) = &self.on_change
            && let Err(error) = handler(&observation)
        {
            let _ = session.disconnect().await;
            return Err(error);
        }
        Ok((session, observation))
    }
}

impl<C: RemoteConnector> RemoteConnector for ObservedConnector<C> {
    type Connection = C::Connection;

    async fn connect(
        &self,
        target: &RemoteTarget,
        auth: &RemoteAuth,
        host_key_policy: HostKeyPolicy,
    ) -> AppResult<Self::Connection> {
        if host_key_policy != HostKeyPolicy::Capture {
            return Err(AppError::InvalidConfig(
                "自动指纹观测连接器只接受采集策略".into(),
            ));
        }
        self.connect_observed(target, auth)
            .await
            .map(|(session, _)| session)
    }
}
