use tauri::State;

use crate::application::diagnostics::{
    SystemDiagnostics, SystemDiagnosticsInput, build_system_diagnostics,
};
use crate::formal::app_state::FormalAppState;
use crate::infrastructure::agent_asset::{
    AGENT_COMPATIBILITY, AGENT_PROTOCOL_VERSION, AGENT_SHA256, AGENT_VERSION,
};

#[tauri::command]
pub fn get_system_diagnostics(state: State<'_, FormalAppState>) -> SystemDiagnostics {
    build_system_diagnostics(SystemDiagnosticsInput {
        application_version: env!("CARGO_PKG_VERSION").into(),
        local_schema_version: "2".into(),
        workbench_schema_version: "2".into(),
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
    })
}
