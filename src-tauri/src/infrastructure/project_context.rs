use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use crate::core::error::{AppError, AppResult};
use crate::core::secret::SecretValue;
use crate::domain::common::task::TaskState;
use crate::formal::app_state::FormalAppState;
use crate::formal::error::FormalError;
use crate::formal::operation_repository::OperationRepository;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::runtime_registry::ConnectionHealth;
use crate::formal::workbench_store::{WorkbenchSchemaStatus, WorkbenchStore};
use crate::infrastructure::database::{DatabaseTlsMode, DualMySqlPools, MySqlProjectConfig};

pub async fn project_database(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<Arc<DualMySqlPools>> {
    project_database_with_recovery(state, local_project_id, true).await
}

pub async fn project_database_for_finalization(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<Arc<DualMySqlPools>> {
    project_database_with_recovery(state, local_project_id, false).await
}

async fn project_database_with_recovery(
    state: &FormalAppState,
    local_project_id: &str,
    recover_abandoned: bool,
) -> AppResult<Arc<DualMySqlPools>> {
    let pools = project_pools(state, local_project_id).await?;
    let status = workbench_schema_status_with_pools(state, local_project_id, &pools).await?;
    let runtime = state
        .runtime_registry
        .open(local_project_id)
        .await
        .map_err(map_formal_error)?;
    if status.is_ready() {
        if recover_abandoned {
            let protected_operations = state
                .task_repository
                .list_active()
                .await?
                .into_iter()
                .filter(|task| {
                    task.local_project_id == local_project_id
                        && task.state == TaskState::FinalizingFailed
                })
                .filter_map(|task| task.remote_operation_record_id)
                .collect::<BTreeSet<_>>();
            recover_abandoned_operations(&pools, &protected_operations).await?;
        }
        runtime.set_health(ConnectionHealth::Ready).await;
        return Ok(pools);
    }
    runtime.set_health(ConnectionHealth::Degraded).await;
    Err(AppError::Conflict(format!(
        "{}；请在项目设置中显式初始化或升级",
        status.message
    )))
}

async fn recover_abandoned_operations(
    pools: &DualMySqlPools,
    protected_operations: &BTreeSet<String>,
) -> AppResult<()> {
    let repository = OperationRepository::new(pools.workbench.clone());
    let stale_after = Duration::from_secs(120);
    let candidates = repository
        .list_stale_candidates(stale_after, 100)
        .await
        .map_err(map_formal_error)?;
    for candidate in candidates {
        if protected_operations.contains(&candidate.id) {
            continue;
        }
        match repository
            .interrupt_stale(&candidate.id, candidate.version, stale_after)
            .await
        {
            Ok(record) => {
                tracing::warn!(
                    operation_id = %record.id,
                    target_count = record.target_count,
                    "recovered abandoned project operation"
                );
            }
            Err(FormalError::Conflict(_)) => {
                tracing::debug!(
                    operation_id = %candidate.id,
                    "abandoned operation changed during recovery"
                );
            }
            Err(error) => return Err(map_formal_error(error)),
        }
    }
    Ok(())
}

pub async fn workbench_schema_status(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<WorkbenchSchemaStatus> {
    let pools = project_pools(state, local_project_id).await?;
    let status = workbench_schema_status_with_pools(state, local_project_id, &pools).await?;
    let runtime = state
        .runtime_registry
        .open(local_project_id)
        .await
        .map_err(map_formal_error)?;
    runtime
        .set_health(if status.is_ready() {
            ConnectionHealth::Ready
        } else {
            ConnectionHealth::Degraded
        })
        .await;
    Ok(status)
}

pub async fn initialize_or_upgrade_workbench_schema(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<WorkbenchSchemaStatus> {
    let pools = project_pools(state, local_project_id).await?;
    let before = workbench_schema_status_with_pools(state, local_project_id, &pools).await?;
    if before.is_ready() {
        return Ok(before);
    }
    if !before.can_upgrade() {
        return Err(AppError::Conflict(format!(
            "{}；为避免覆盖人工结构，自动升级已阻止",
            before.message
        )));
    }
    WorkbenchStore::new(pools.workbench.clone())
        .migrate()
        .await
        .map_err(map_formal_error)?;
    let after = workbench_schema_status_with_pools(state, local_project_id, &pools).await?;
    let runtime = state
        .runtime_registry
        .open(local_project_id)
        .await
        .map_err(map_formal_error)?;
    if after.is_ready() {
        runtime.set_health(ConnectionHealth::Ready).await;
        Ok(after)
    } else {
        runtime.set_health(ConnectionHealth::Degraded).await;
        Err(AppError::Conflict(format!(
            "工作台数据库升级后仍未就绪：{}",
            after.message
        )))
    }
}

async fn project_pools(
    state: &FormalAppState,
    local_project_id: &str,
) -> AppResult<Arc<DualMySqlPools>> {
    if local_project_id.trim().is_empty() {
        return Err(AppError::InvalidConfig("本地项目 ID 不能为空".into()));
    }
    let runtime = state
        .runtime_registry
        .open(local_project_id)
        .await
        .map_err(map_formal_error)?;
    let result = runtime
        .database_or_try_init(|| async {
            let repository = LocalProjectRepository::new(
                state.local_store.pool().clone(),
                state.secret_store.clone(),
            );
            let secrets = repository
                .connection_secrets(local_project_id)
                .await
                .map_err(map_formal_error)?;
            let tls_mode = if secrets.project.db_tls_enabled {
                DatabaseTlsMode::Required
            } else {
                DatabaseTlsMode::Disabled
            };
            let config = MySqlProjectConfig {
                host: secrets.project.db_host,
                port: secrets.project.db_port,
                username: secrets.project.db_user,
                password: SecretValue::new(secrets.db_password),
                platform_schema: secrets.project.business_db,
                workbench_schema: secrets.project.workbench_db,
                connect_timeout: Duration::from_secs(10),
                tls_mode,
            };
            state
                .task_event_pipeline
                .register_secrets([config.password.expose().to_string()])?;
            let pools = DualMySqlPools::connect(&config).await?;
            let report = match pools.probe(&config).await {
                Ok(report) => report,
                Err(error) => {
                    pools.close().await;
                    return Err(error);
                }
            };
            if !report.missing_required_columns.is_empty() {
                pools.close().await;
                return Err(AppError::InvalidConfig(format!(
                    "平台一体机表缺少字段：{}",
                    report.missing_required_columns.join("、")
                )));
            }
            repository
                .touch_opened(local_project_id)
                .await
                .map_err(map_formal_error)?;
            Ok(pools)
        })
        .await;
    match result {
        Ok(pools) => Ok(pools),
        Err(error) => {
            runtime.set_health(ConnectionHealth::Degraded).await;
            Err(error)
        }
    }
}

async fn workbench_schema_status_with_pools(
    state: &FormalAppState,
    local_project_id: &str,
    pools: &DualMySqlPools,
) -> AppResult<WorkbenchSchemaStatus> {
    let repository =
        LocalProjectRepository::new(state.local_store.pool().clone(), state.secret_store.clone());
    let project = repository
        .get(local_project_id)
        .await
        .map_err(map_formal_error)?;
    WorkbenchStore::new(pools.workbench.clone())
        .schema_status(&project.workbench_db)
        .await
        .map_err(map_formal_error)
}

pub fn map_formal_error(error: FormalError) -> AppError {
    match error {
        FormalError::InvalidConfig(message) => AppError::InvalidConfig(message),
        FormalError::Conflict(message) => AppError::Conflict(message),
        FormalError::NotFound(message) => AppError::NotFound(message),
        FormalError::LocalDatabase(operation) => AppError::Database { operation },
        FormalError::SecretStore(operation) => AppError::Io { operation },
        FormalError::LocalIo(operation) => AppError::Io { operation },
    }
}
