export type ProjectConnectionState = "ready" | "login_required" | "offline";
export type NodeManagementState = "managed" | "pending" | "platform_existing" | "conflict";
export type PlatformState = "online" | "offline" | "unknown";
export type ServiceState = "healthy" | "warning" | "unreachable" | "unknown";
export type OperationMode = "first_deploy" | "full_upgrade" | "service_upgrade";
export type TaskState = "queued" | "running" | "succeeded" | "partially_succeeded" | "failed";

export interface DemoProject {
  id: string;
  name: string;
  code: string;
  platformUrl: string;
  databaseHost: string;
  databaseName: string;
  connectionState: ProjectConnectionState;
  username?: string;
}

export interface ServiceVersion {
  service: string;
  image: string;
  expectedVersion: string;
  observedVersion: string;
  observedAt: string;
}

export interface EdgeNode {
  mac: string;
  name: string;
  ip: string;
  location: string;
  managementState: NodeManagementState;
  deployLabel: string;
  platformState: PlatformState;
  platformUpdatedAt: string;
  serviceState: ServiceState;
  serviceLabel: string;
  lastOperation: string;
  platformId?: string;
  versions: ServiceVersion[];
}

export interface ReleaseProfile {
  version: number;
  updatedBy: string;
  updatedAt: string;
  platformHost: string;
  platformApi: string;
  platformAuthKey: string;
  platformMqttHost: string;
  platformMqttPort: number;
  platformMqttUsername: string;
  platformMqttPassword: string;
  aioMqttUsername: string;
  aioMqttPassword: string;
  sshUsername: string;
  sshPassword: string;
  sshPrivateKey: string;
  envTemplate: string;
  composeTemplate: string;
}

export interface DemoTask {
  id: string;
  projectId: string;
  name: string;
  mode: OperationMode | "inventory_import";
  state: TaskState;
  stage: string;
  progress: number;
  targetCount: number;
  completedCount: number;
  updatedAt: string;
}

export interface DemoLogEntry {
  id: string;
  taskId: string;
  time: string;
  level: "INFO" | "WARN" | "ERROR";
  source: string;
  message: string;
}

export interface OperationHistoryItem {
  id: string;
  type: string;
  operator: string;
  targetSummary: string;
  artifact: string;
  result: "成功" | "部分成功" | "失败";
  finishedAt: string;
}

