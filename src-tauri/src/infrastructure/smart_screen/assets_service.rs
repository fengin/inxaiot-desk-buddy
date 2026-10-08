use super::platform;
use crate::application::ports::project_access::ProjectAccessPort;
use crate::application::ports::smart_screen::ScreenAssetsPort;
use crate::application::project_access::ProjectAccessRequirement;
use crate::core::error::{AppError, AppResult};
use crate::domain::smart_screen::model::*;
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::local_sqlite::screen_repository::ScreenRepository;
use crate::infrastructure::project_context::project_pools;
use crate::infrastructure::stage75_adapter::Stage75Adapter;

pub struct ScreenAssetsService<'a> {
    state: &'a FormalAppState,
}
impl<'a> ScreenAssetsService<'a> {
    pub fn new(state: &'a FormalAppState) -> Self {
        Self { state }
    }
    pub fn local(&self) -> ScreenRepository {
        ScreenRepository::new(self.state.local_store.pool().clone())
    }
    async fn local_access(&self, project: &str) -> AppResult<()> {
        Stage75Adapter::new(self.state)
            .require_project_access(project, ProjectAccessRequirement::Configured)
            .await
    }
    async fn refresh(&self, project: &str) -> AppResult<Vec<BusinessProject>> {
        Stage75Adapter::new(self.state)
            .require_project_access(project, ProjectAccessRequirement::PlatformRead)
            .await?;
        let local = self.local();
        let revision = local.project_connection_revision(project).await?;
        let pools = project_pools(self.state, project).await?;
        let source = platform::source_id(&pools.platform).await?;
        let available = platform::projects(&pools.platform).await?;
        if let Some((business, known_source)) = local.scope(project).await? {
            if known_source != source || !available.iter().any(|p| p.id == business) {
                local
                    .clear_unused_scope(project, &business, &known_source)
                    .await?;
            }
        }
        let scope = match local.scope(project).await? {
            Some(scope) => Some(scope),
            None if available.len() == 1 => Some((available[0].id.clone(), source.clone())),
            _ => None,
        };
        if let Some((business, known_source)) = scope {
            if known_source != source || !available.iter().any(|p| p.id == business) {
                return Err(AppError::Conflict(
                    "项目连接或业务范围在读取期间已改变，请刷新列表".into(),
                ));
            }
            let known = local.known_platform_ids(project, &business).await?;
            let (assets, spaces, deleted) = platform::read_with_known_ids(&pools.platform, &business, &known).await?;
            local
                .replace_snapshot_and_deleted(project, &business, &source, &revision, &assets, &spaces, &deleted)
                .await?;
        }
        Ok(available)
    }
}
impl ScreenAssetsPort for ScreenAssetsService<'_> {
    async fn snapshot(&self, project: &str, refresh: bool) -> AppResult<ScreenSnapshot> {
        self.local_access(project).await?;
        let configured = crate::formal::project_repository::LocalProjectRepository::new(
            self.state.local_store.pool().clone(),
            self.state.secret_store.clone(),
        )
        .get(project)
        .await
        .map_err(crate::infrastructure::project_context::map_formal_error)?;
        let local_only = crate::domain::common::project::local_project_only(
            &configured.platform_url,
            &configured.db_host,
            &configured.db_user,
            &configured.business_db,
        );
        let refreshed = if refresh && !local_only {
            Some(self.refresh(project).await)
        } else {
            None
        };
        let mut snapshot = self.local().snapshot(project).await?;
        snapshot.tasks = super::tasks::views(self.state, project).await?;
        snapshot.local_only = local_only;
        if let Some(result) = refreshed {
            match result {
                Ok(projects) => {
                    snapshot.available_projects = projects;
                    snapshot.platform_available = snapshot.business_project_id.is_some();
                    if !snapshot.platform_available {
                        snapshot.platform_message =
                            Some(if snapshot.available_projects.is_empty() {
                                "平台暂无有效业务项目和空间".into()
                            } else {
                                "请选择该本机项目对应的业务项目".into()
                            });
                    }
                }
                Err(error) => {
                    snapshot.platform_message = Some(error.to_string());
                }
            }
        }
        Ok(snapshot)
    }
    async fn select_business_project(&self, project: &str, business: &str) -> AppResult<()> {
        Stage75Adapter::new(self.state)
            .require_project_access(project, ProjectAccessRequirement::PlatformRead)
            .await?;
        let pools = project_pools(self.state, project).await?;
        if !platform::projects(&pools.platform)
            .await?
            .iter()
            .any(|p| p.id == business)
        {
            return Err(AppError::InvalidConfig("请选择平台中有效的业务项目".into()));
        }
        self.local()
            .set_scope(
                project,
                business,
                &platform::source_id(&pools.platform).await?,
            )
            .await
    }
    async fn save_local(
        &self,
        project: &str,
        fields: &ScreenFields,
        id: Option<&str>,
        revision: Option<u64>,
    ) -> AppResult<String> {
        self.local_access(project).await?;
        self.local().save_local(project, fields, id, revision).await
    }
    async fn import_local(&self, project: &str, fields: &[ScreenFields]) -> AppResult<Vec<String>> {
        self.local_access(project).await?;
        self.local().import_local(project, fields).await
    }
    async fn remove_local(&self, project: &str, id: &str) -> AppResult<()> {
        self.local_access(project).await?;
        self.local().remove_local(project, id).await
    }
    async fn save_draft(
        &self,
        project: &str,
        id: &str,
        fields: &ScreenFields,
        revision: u64,
        asset_revision: u64,
    ) -> AppResult<()> {
        self.local_access(project).await?;
        self.local()
            .save_draft_checked(project, id, fields, asset_revision, revision)
            .await
    }
    async fn discard_draft(&self, project: &str, id: &str, revision: u64) -> AppResult<()> {
        self.local_access(project).await?;
        self.local().discard_draft(project, id, revision).await
    }
}
