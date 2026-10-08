use super::{platform, shared_results::ScreenSharedResults};
use crate::application::{
    ports::project_access::ProjectAccessPort, project_access::ProjectAccessRequirement,
};
use crate::core::{
    error::{AppError, AppResult},
};
use crate::formal::{app_state::FormalAppState, project_repository::LocalProjectRepository};
use crate::infrastructure::{
    local_sqlite::screen_repository::ScreenRepository,
    project_context::{map_formal_error, project_database_for_finalization, project_operator},
    stage75_adapter::Stage75Adapter,
};
use sqlx::MySqlPool;

pub struct ScreenWriteContext {
    pub business: String,
    pub source: String,
    pub read: MySqlPool,
    pub write: MySqlPool,
    pub shared: MySqlPool,
    pub shared_schema: String,
    pub operator: String,
}
impl ScreenWriteContext {
    pub fn validate_plan(
        &self,
        plan: &crate::domain::smart_screen::operation::ScreenPlan,
    ) -> AppResult<()> {
        if plan.business_project_id.as_deref() != Some(self.business.as_str())
            || plan.data_source_id.as_deref() != Some(self.source.as_str())
        {
            return Err(AppError::Conflict(
                "业务项目或平台数据源已变化，不能继续原写操作".into(),
            ));
        }
        Ok(())
    }
}

pub async fn open(state: &FormalAppState, project: &str) -> AppResult<ScreenWriteContext> {
    Stage75Adapter::new(state)
        .require_project_access(project, ProjectAccessRequirement::ActiveSession)
        .await?;
    let local = ScreenRepository::new(state.local_store.pool().clone());
    let (business, known_source) = local
        .scope(project)
        .await?
        .ok_or_else(|| AppError::Conflict("请先读取并确认平台业务项目".into()))?;
    let pools = project_database_for_finalization(state, project).await?;
    let source = platform::source_id(&pools.platform).await?;
    if source != known_source {
        return Err(AppError::Conflict("平台数据源与本机资料不一致".into()));
    }
    let repository =
        LocalProjectRepository::new(state.local_store.pool().clone(), state.secret_store.clone());
    let record = repository.get(project).await.map_err(map_formal_error)?;
    let write = crate::infrastructure::project_context::project_platform_write_pool(state, project).await?;
    if platform::source_id(&write).await? != source {
        return Err(AppError::Conflict("平台读写连接的数据源不同".into()));
    }
    ScreenSharedResults::new(pools.workbench.clone())
        .bind_source(&source, &record.business_db)
        .await?;
    Ok(ScreenWriteContext {
        business,
        source,
        read: pools.platform.clone(),
        write,
        shared: pools.workbench.clone(),
        shared_schema: record.workbench_db,
        operator: project_operator(state, project).await?,
    })
}
