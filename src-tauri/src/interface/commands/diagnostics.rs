use tauri::State;

use crate::application::diagnostics::{
    SystemDiagnostics, SystemDiagnosticsInput, build_system_diagnostics,
};
use crate::formal::app_state::FormalAppState;
use crate::formal::project_repository::LocalProjectRepository;
use crate::formal::workbench_store::latest_workbench_schema_version;
use crate::infrastructure::agent_asset::{
    AGENT_COMPATIBILITY, AGENT_PROTOCOL_VERSION, AGENT_SHA256, AGENT_VERSION,
};
use crate::interface::error::CommandErrorDto;

#[tauri::command]
pub async fn get_system_diagnostics(
    state: State<'_, FormalAppState>,
) -> Result<SystemDiagnostics, CommandErrorDto> {
    let local_schema_version = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(version) FROM _sqlx_migrations WHERE success = 1",
    )
    .fetch_one(state.local_store.pool())
    .await
    .map_err(|error| {
        CommandErrorDto::from(crate::core::error::AppError::database(
            "读取本地Schema版本",
            &error,
        ))
    })?
    .unwrap_or_default()
    .to_string();
    let pending_secret_cleanup_count =
        LocalProjectRepository::new(state.local_store.pool().clone(), state.secret_store.clone())
            .pending_secret_cleanup_count()
            .await
            .map_err(|error| CommandErrorDto::from(crate::core::error::AppError::from(error)))?;
    Ok(build_system_diagnostics(SystemDiagnosticsInput {
        application_version: env!("CARGO_PKG_VERSION").into(),
        source_commit: env!("INX_BUILD_GIT_COMMIT").into(),
        local_schema_version,
        workbench_schema_version: latest_workbench_schema_version().to_string(),
        agent_version: AGENT_VERSION.into(),
        agent_protocol_version: AGENT_PROTOCOL_VERSION.into(),
        agent_sha256: AGENT_SHA256.into(),
        agent_compatibility: AGENT_COMPATIBILITY
            .iter()
            .map(|value| (*value).into())
            .collect(),
        operating_system: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        data_directory: state.paths.data_dir.to_string_lossy().into_owned(),
        application_logs_directory: state.paths.logs_dir.to_string_lossy().into_owned(),
        task_logs_directory: state.paths.task_logs_dir.to_string_lossy().into_owned(),
        task_artifacts_directory: state
            .paths
            .task_artifacts_dir
            .to_string_lossy()
            .into_owned(),
        pending_secret_cleanup_count,
    }))
}
