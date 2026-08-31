import {
  inspectServiceImage,
  validateReleasePackage
} from "@/shared/api/release";
import type { OperationsAdapter } from "@/shared/api/operationsAdapter";
import type { OperationMode } from "@/shared/model/demo";
import {
  getDeploymentTask,
  getOperationHistoryDetail,
  listOperationHistory,
  preflightDeployment,
  submitDeployment
} from "@/shared/api/deploymentWorkflow";

export class RealOperationsAdapter implements OperationsAdapter {
  readonly real = true;
  async inspectArtifact(mode: OperationMode, path: string) {
    return mode === "service_upgrade"
      ? { imageInspection: await inspectServiceImage(path) }
      : { releaseValidation: await validateReleasePackage(path) };
  }
  preflight(projectId: string, plan: Parameters<OperationsAdapter["preflight"]>[1]) {
    return preflightDeployment(projectId, plan);
  }
  submit(projectId: string, plan: Parameters<OperationsAdapter["submit"]>[1]) {
    return submitDeployment(projectId, plan);
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
