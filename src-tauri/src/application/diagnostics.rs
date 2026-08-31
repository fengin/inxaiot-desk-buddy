use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SystemDiagnostics {
    pub application_version: String,
    pub local_schema_version: String,
    pub workbench_schema_version: String,
    pub agent_version: String,
    pub agent_protocol_version: String,
    pub agent_sha256: String,
    pub agent_compatibility: Vec<String>,
    pub operating_system: String,
    pub architecture: String,
    pub data_directory: String,
    pub application_logs_directory: String,
    pub task_logs_directory: String,
    pub task_artifacts_directory: String,
}

#[derive(Clone, Debug)]
pub struct SystemDiagnosticsInput {
    pub application_version: String,
    pub local_schema_version: String,
    pub workbench_schema_version: String,
    pub agent_version: String,
    pub agent_protocol_version: String,
    pub agent_sha256: String,
    pub agent_compatibility: Vec<String>,
    pub operating_system: String,
    pub architecture: String,
    pub data_directory: String,
    pub application_logs_directory: String,
    pub task_logs_directory: String,
    pub task_artifacts_directory: String,
}

pub fn build_system_diagnostics(input: SystemDiagnosticsInput) -> SystemDiagnostics {
    SystemDiagnostics {
        application_version: input.application_version,
        local_schema_version: input.local_schema_version,
        workbench_schema_version: input.workbench_schema_version,
        agent_version: input.agent_version,
        agent_protocol_version: input.agent_protocol_version,
        agent_sha256: input.agent_sha256,
        agent_compatibility: input.agent_compatibility,
        operating_system: input.operating_system,
        architecture: input.architecture,
        data_directory: input.data_directory,
        application_logs_directory: input.application_logs_directory,
        task_logs_directory: input.task_logs_directory,
        task_artifacts_directory: input.task_artifacts_directory,
    }
}
