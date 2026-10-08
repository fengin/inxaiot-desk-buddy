import type { DeploymentPlanInput } from "@/shared/model/release";

export type PreflightStatus = "passed" | "warning" | "failed";

export interface PreflightRemediation {
  action: string;
  label: string;
  route?: string;
  target?: string;
}

export interface DeploymentPreflightCheck {
  code: string;
  label: string;
  status: PreflightStatus;
  blocking: boolean;
  targetMac?: string;
  message: string;
  remediation?: PreflightRemediation;
}

export interface DeploymentTargetSnapshot {
  node: {
    macNormalized: string;
    name: string;
    ip: string;
    buildingId?: string;
    regionId?: string;
    addrAlias?: string;
    floor?: string;
    location?: string;
    remark?: string;
    platformAioId?: string;
    managementState: string;
    source: string;
    lastOperationId?: string;
    version: number;
  };
  sshHost: string;
  sshPort: number;
  hostKeyAlgorithm: string;
  hostKeyFingerprint: string;
  hostKeyAcceptedAt: string;
}

export interface DeploymentExecutionSnapshot {
  schemaVersion: number;
  localProjectId: string;
  checkedAt: string;
  profileVersion: number;
  artifactFingerprint: string;
  plan: DeploymentPlanInput;
  targets: DeploymentTargetSnapshot[];
}

export interface DeploymentPreflightReport {
  ready: boolean;
  checks: DeploymentPreflightCheck[];
  normalizedPlan: DeploymentPlanInput;
  profileVersion?: number;
  executionSnapshot?: DeploymentExecutionSnapshot | null;
  checkedAt: string;
}

export interface DeploymentTaskSubmission {
  taskId: string;
  state: string;
  submittedAt: string;
}

export interface DeploymentTaskTargetView {
  mac: string;
  state: string;
  stage: string;
  progress: number;
  messageCode?: string;
  message?: string;
  updatedAt: string;
}

export interface DeploymentTaskStepView {
  mac?: string;
  code: string;
  state: string;
  errorCode?: string;
  message?: string;
  startedAt?: string;
  endedAt?: string;
}

export interface DeploymentTaskView {
  id: string;
  projectId: string;
  operationId?: string;
  operationType: string;
  name: string;
  state: string;
  stage: string;
  progress: number;
  targetCount: number;
  completedCount: number;
  successCount: number;
  failureCount: number;
  cancelledCount: number;
  cancellable: boolean;
  errorCode?: string;
  message?: string;
  startedAt?: string;
  endedAt?: string;
  updatedAt: string;
  targets: DeploymentTaskTargetView[];
  steps: DeploymentTaskStepView[];
}

export interface OperationHistoryQuery {
  page: number;
  pageSize: number;
  operationType?: string;
  state?: string;
}

export interface OperationHistoryItem {
  id: string;
  domainType: string;
  operationType: string;
  operationName: string;
  operatorName: string;
  instanceId: string;
  state: string;
  targetCount: number;
  successCount: number;
  failureCount: number;
  cancelledCount: number;
  artifactName?: string;
  artifactVersion?: string;
  startedAt: string;
  endedAt?: string;
  resultSummary?: string;
  errorCode?: string;
  errorSummary?: string;
}

export interface OperationHistoryTarget {
  details?: { targetName?: string; targetIp?: string; device?: string; business?: string; shared?: string; observedAt?: string; observedIp?: string; appVersionCode?: number; retryOfOperationId?: string; package?: {name?:string;version?:string;versionCode?:number;sha256?:string}; configuration?: {fields?:string[];save?:string;restart?:string;readback?:string;changes?:{field:string;before:string|boolean|null;after:string|boolean|null}[]} };
  resourceType: string;
  resourceKey: string;
  state: string;
  beforeVersion?: string;
  afterVersion?: string;
  resultSummary?: string;
  errorCode?: string;
  errorSummary?: string;
  completedAt?: string;
}

export interface OperationHistoryPage {
  items: OperationHistoryItem[];
  total: number;
  page: number;
  pageSize: number;
}

export interface OperationHistoryDetail {
  operation: OperationHistoryItem;
  targets: OperationHistoryTarget[];
}
