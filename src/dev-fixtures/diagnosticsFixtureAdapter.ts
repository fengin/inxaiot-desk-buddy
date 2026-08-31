import type { DiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";

export class FixtureDiagnosticsAdapter implements DiagnosticsAdapter {
  async getSystemDiagnostics() {
    return {
      applicationVersion: "fixture",
      sourceCommit: "fixture-only",
      localSchemaVersion: "fixture",
      workbenchSchemaVersion: "fixture",
      agentVersion: "fixture",
      agentProtocolVersion: "fixture",
      agentSha256: "fixture-only",
      agentCompatibility: ["浏览器Fixture，不代表Tauri真实运行环境"],
      operatingSystem: "browser",
      architecture: "fixture",
      dataDirectory: "D:\\INX\\DeskBuddy-Fixture",
      applicationLogsDirectory: "fixture://logs",
      taskLogsDirectory: "fixture://task-logs",
      taskArtifactsDirectory: "fixture://task-artifacts"
    };
  }
}
