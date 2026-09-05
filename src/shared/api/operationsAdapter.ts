import type {
  DeploymentPlanInput,
  ServiceImageInspection
} from "@/shared/model/release";
import type {
  DeploymentExecutionSnapshot,
  DeploymentPreflightReport,
  DeploymentTaskSubmission,
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage,
  OperationHistoryQuery
} from "@/shared/model/deploymentWorkflow";

export interface OperationsAdapter {
  readonly real: boolean;
  inspectImage(path: string, expectedImage?: string): Promise<ServiceImageInspection>;
  preflight(
    projectId: string,
    preflightTaskId: string,
    plan: DeploymentPlanInput
  ): Promise<DeploymentPreflightReport>;
  submit(
    projectId: string,
    preflightTaskId: string,
    executionSnapshot: DeploymentExecutionSnapshot
  ): Promise<DeploymentTaskSubmission>;
  getTask(projectId: string, taskId: string): Promise<DeploymentTaskView>;
  listHistory(projectId: string, query: OperationHistoryQuery): Promise<OperationHistoryPage>;
  getHistoryDetail(projectId: string, operationId: string): Promise<OperationHistoryDetail>;
}

let adapter: OperationsAdapter | undefined;
export function configureOperationsAdapter(next: OperationsAdapter) { adapter = next; }
export function useOperationsAdapter() {
  if (!adapter) throw new Error("Operations Adapter 尚未初始化");
  return adapter;
}
