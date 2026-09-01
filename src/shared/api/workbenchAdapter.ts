import type {
  ConfirmHostKeyRequest,
  HostKeyCaptureRequest,
  HostKeyObservation,
  PlatformLoginChallenge,
  PlatformLoginRequest,
  ProjectConnectionTestRequest,
  ProjectConnectionTestResult,
  ProjectInput,
  ProjectOverview,
  ProjectSession,
  WorkbenchSchemaStatus
} from "@/shared/model/project";
import type {
  ReleaseMasterKeyOperationResult,
  ReleaseMasterKeyTransferRequest,
  ReleaseProfileDraft,
  ReleaseProfileValidation,
  ReleaseProfileView
} from "@/shared/model/releaseProfile";

export interface WorkbenchAdapter {
  listProjects(): Promise<ProjectOverview[]>;
  createProject(input: ProjectInput): Promise<ProjectOverview>;
  updateProject(projectId: string, input: ProjectInput): Promise<ProjectOverview>;
  deleteProject(projectId: string): Promise<void>;
  testProjectConnection(request: ProjectConnectionTestRequest): Promise<ProjectConnectionTestResult>;
  switchProject(projectId: string): Promise<ProjectOverview>;
  createLoginChallenge(projectId: string): Promise<PlatformLoginChallenge>;
  loginProject(projectId: string, request: PlatformLoginRequest): Promise<ProjectSession>;
  getProjectSession(projectId: string): Promise<ProjectSession>;
  checkProjectSession(projectId: string): Promise<ProjectSession>;
  logoutProject(projectId: string): Promise<ProjectSession>;
  getWorkbenchSchemaStatus(projectId: string): Promise<WorkbenchSchemaStatus>;
  initializeOrUpgradeWorkbenchSchema(projectId: string): Promise<WorkbenchSchemaStatus>;
  getReleaseProfile(projectId: string): Promise<ReleaseProfileView | null>;
  validateReleaseProfile(draft: ReleaseProfileDraft): Promise<ReleaseProfileValidation>;
  saveReleaseProfile(projectId: string, draft: ReleaseProfileDraft): Promise<ReleaseProfileView>;
  exportReleaseMasterKey(projectId: string, request: ReleaseMasterKeyTransferRequest): Promise<ReleaseMasterKeyOperationResult>;
  importReleaseMasterKey(projectId: string, request: ReleaseMasterKeyTransferRequest): Promise<ReleaseMasterKeyOperationResult>;
  listHostKeys(projectId: string): Promise<HostKeyObservation[]>;
  captureHostKey(projectId: string, request: HostKeyCaptureRequest): Promise<HostKeyObservation>;
  confirmHostKey(projectId: string, request: ConfirmHostKeyRequest): Promise<HostKeyObservation>;
}

let adapter: WorkbenchAdapter | undefined;

export function configureWorkbenchAdapter(next: WorkbenchAdapter) {
  adapter = next;
}

export function useWorkbenchAdapter(): WorkbenchAdapter {
  if (!adapter) throw new Error("工作台 Adapter 尚未初始化");
  return adapter;
}
