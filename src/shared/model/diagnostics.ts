export interface SystemDiagnostics {
  applicationVersion: string;
  sourceCommit: string;
  localSchemaVersion: string;
  workbenchSchemaVersion: string;
  agentVersion: string;
  agentProtocolVersion: string;
  agentSha256: string;
  agentCompatibility: string[];
  operatingSystem: string;
  architecture: string;
  dataDirectory: string;
  applicationLogsDirectory: string;
  taskLogsDirectory: string;
  taskArtifactsDirectory: string;
}
