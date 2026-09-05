import { inspectServiceImage } from "@/shared/api/release";
import type { OperationsAdapter } from "@/shared/api/operationsAdapter";
import {
  getDeploymentTask,
  getOperationHistoryDetail,
  listOperationHistory,
  preflightDeployment,
  submitDeployment
} from "@/shared/api/deploymentWorkflow";

export class RealOperationsAdapter implements OperationsAdapter {
  readonly real = true;
  inspectImage(path: string, expectedImage?: string) {
    return inspectServiceImage(path, expectedImage);
  }
  preflight(
    projectId: string,
    preflightTaskId: string,
    plan: Parameters<OperationsAdapter["preflight"]>[2]
  ) {
    return preflightDeployment(projectId, preflightTaskId, plan);
  }
  submit(
    projectId: string,
    preflightTaskId: Parameters<OperationsAdapter["submit"]>[1],
    executionSnapshot: Parameters<OperationsAdapter["submit"]>[2]
  ) {
    return submitDeployment(projectId, preflightTaskId, executionSnapshot);
  }
  getTask(projectId: string, taskId: string) {
    return getDeploymentTask(projectId, taskId);
  }
  listHistory(projectId: string, query: Parameters<OperationsAdapter["listHistory"]>[1]) {
    return listOperationHistory(projectId, query);
  }
  getHistoryDetail(projectId: string, operationId: string) {
    return getOperationHistoryDetail(projectId, operationId);
  }
}
