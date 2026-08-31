import type { OperationMode } from "@/shared/model/demo";
import type {
  DeploymentPreflightReport,
  DeploymentTaskSubmission,
  DeploymentPlanInput,
  ReleaseValidation,
  ServiceImageInspection
} from "@/shared/model/release";
import type {
  DeploymentTaskView,
  OperationHistoryDetail,
  OperationHistoryPage,
  OperationHistoryQuery
} from "@/shared/model/deploymentWorkflow";

export interface ArtifactInspection {
  releaseValidation?: ReleaseValidation;
  imageInspection?: ServiceImageInspection;
}

export interface OperationsAdapter {
  readonly real: boolean;
  inspectArtifact(mode: OperationMode, path: string): Promise<ArtifactInspection>;
  preflight(projectId: string, plan: DeploymentPlanInput): Promise<DeploymentPreflightReport>;
  submit(projectId: string, plan: DeploymentPlanInput): Promise<DeploymentTaskSubmission>;
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
