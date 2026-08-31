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

export interface DeploymentPreflightReport {
  ready: boolean;
  checks: DeploymentPreflightCheck[];
  normalizedPlan: DeploymentPlanInput;
  profileVersion?: number;
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
