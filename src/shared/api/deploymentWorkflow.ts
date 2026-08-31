import { invoke } from "@tauri-apps/api/core";

import type {
  DeploymentPreflightReport,
  DeploymentTaskSubmission,
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage,
  OperationHistoryQuery
} from "@/shared/model/deploymentWorkflow";
import type { DeploymentPlanInput } from "@/shared/model/release";

export function preflightDeployment(
  localProjectId: string,
  input: DeploymentPlanInput
) {
  return invoke<DeploymentPreflightReport>("preflight_deployment", {
    localProjectId,
    input
  });
}

export function submitDeployment(
  localProjectId: string,
  input: DeploymentPlanInput
) {
  return invoke<DeploymentTaskSubmission>("submit_deployment", {
    localProjectId,
    input
  });
}

export function getDeploymentTask(localProjectId: string, taskId: string) {
  return invoke<DeploymentTaskView>("get_deployment_task", { localProjectId, taskId });
}

export function listOperationHistory(
  localProjectId: string,
  query: OperationHistoryQuery
) {
  return invoke<OperationHistoryPage>("list_operation_history", { localProjectId, query });
}

export function getOperationHistoryDetail(
  localProjectId: string,
  operationId: string
) {
  return invoke<OperationHistoryDetail>("get_operation_history_detail", {
    localProjectId,
    operationId
  });
}
