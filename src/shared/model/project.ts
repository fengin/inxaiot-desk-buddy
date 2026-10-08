import type { WorkbenchSchemaStatus } from "@/shared/model/projectDatabase";

export type ProjectConnectionState =
  | "disconnected"
  | "connecting"
  | "login_required"
  | "session_expired"
  | "schema_required"
  | "ready"
  | "connection_failed";

export type DatabaseConnectionState = "disconnected" | "connected" | "failed";
export type ProjectSessionState = "active" | "missing" | "expired";

export interface ProjectInput {
  name: string;
  platformUrl: string;
  dbHost: string;
  dbPort: number;
  dbUser: string;
  dbTlsEnabled: boolean;
  dbPassword?: string;
  businessDb: string;
  workbenchDb: string;
}

export interface ProjectSession {
  localProjectId: string;
  username?: string;
  state: ProjectSessionState;
  expiresAt?: string;
  updatedAt?: string;
}

export interface ProjectOverview {
  id: string;
  name: string;
  platformUrl: string;
  dbHost: string;
  dbPort: number;
  dbUser: string;
  dbTlsEnabled: boolean;
  businessDb: string;
  workbenchDb: string;
  lastOpenedAt?: string;
  connectionState: ProjectConnectionState;
  databaseState: DatabaseConnectionState;
  schemaState?: string;
  session?: ProjectSession;
  connectionEncrypted: boolean;
  statusMessage: string;
}

export interface ProjectConnectionTestRequest {
  existingProjectId?: string;
  project: ProjectInput;
}

export interface ProjectConnectionTestResult {
  successful: boolean;
  platformDatabaseConnected: boolean;
  workbenchDatabaseConnected: boolean;
  platformSchemaCompatible: boolean;
  workbenchSchemaState: string;
  workbenchSchemaMessage: string;
  mysqlVersion: string;
  connectionEncrypted: boolean;
  message: string;
}

export interface PlatformLoginChallenge {
  sessionUuid: string;
  captchaImageDataUrl?: string;
  requiresCaptcha: boolean;
  expiresAtEpochSeconds: number;
}

export interface PlatformLoginRequest {
  username: string;
  password: string;
  sessionUuid: string;
  imageCode: string;
}

export type HostKeyState = "unconfirmed" | "confirmed" | "changed";

export interface HostKeyObservation {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
  state: HostKeyState;
  expectedFingerprint?: string;
  acceptedAt?: string;
}

export interface HostKeyCaptureRequest {
  host: string;
  port?: number;
}

export interface ConfirmHostKeyRequest {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
  replaceChanged: boolean;
}

export type { WorkbenchSchemaStatus };

/** 本机项目没有远端连接，仍可管理本机资料和任务。 */
export function isLocalProject(project: Pick<ProjectInput, "platformUrl" | "dbHost" | "dbUser" | "businessDb">): boolean {
  return [project.platformUrl, project.dbHost, project.dbUser, project.businessDb].every((value) => !value.trim());
}

export type ProjectAccess = "local" | "platform" | "shared";

export function projectAllowsAccess(project: ProjectOverview | undefined, access: ProjectAccess): boolean {
  if (!project) return false;
  if (access === "local") return true;
  // databaseState 当前表示共享连接结果，不能据此屏蔽独立的平台读取。
  // 这里只判断是否可以发起读取，实际连接与会话由后端再次检查。
  if (access === "platform") return project.session?.state === "active" && !isLocalProject(project);
  return project.connectionState === "ready";
}
