import { invoke } from "@tauri-apps/api/core";

import type { WorkbenchAdapter } from "@/shared/api/workbenchAdapter";
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

export class RealWorkbenchAdapter implements WorkbenchAdapter {
  listProjects() { return invoke<ProjectOverview[]>("list_local_projects"); }
  createProject(input: ProjectInput) { return invoke<ProjectOverview>("create_local_project", { input }); }
  updateProject(projectId: string, input: ProjectInput) {
    return invoke<ProjectOverview>("update_local_project", { projectId, input });
  }
  deleteProject(projectId: string) { return invoke<void>("delete_local_project", { projectId }); }
  testProjectConnection(request: ProjectConnectionTestRequest) {
    return invoke<ProjectConnectionTestResult>("test_project_connection", { request });
  }
  switchProject(projectId: string) { return invoke<ProjectOverview>("switch_project", { projectId }); }
  createLoginChallenge(projectId: string) {
    return invoke<PlatformLoginChallenge>("create_project_login_challenge", { projectId });
  }
  loginProject(projectId: string, request: PlatformLoginRequest) {
    return invoke<ProjectSession>("login_project", { projectId, request });
  }
  getProjectSession(projectId: string) {
    return invoke<ProjectSession>("get_project_session", { projectId });
  }
  checkProjectSession(projectId: string) {
    return invoke<ProjectSession>("check_project_session", { projectId });
  }
  logoutProject(projectId: string) { return invoke<ProjectSession>("logout_project", { projectId }); }
  getWorkbenchSchemaStatus(projectId: string) {
    return invoke<WorkbenchSchemaStatus>("get_workbench_schema_status", { localProjectId: projectId });
  }
  initializeOrUpgradeWorkbenchSchema(projectId: string) {
    return invoke<WorkbenchSchemaStatus>("initialize_or_upgrade_workbench_schema", { localProjectId: projectId });
  }
  getReleaseProfile(projectId: string) {
    return invoke<ReleaseProfileView | null>("get_release_profile", { projectId });
  }
  validateReleaseProfile(draft: ReleaseProfileDraft) {
    return invoke<ReleaseProfileValidation>("validate_release_profile", { draft });
  }
  saveReleaseProfile(projectId: string, draft: ReleaseProfileDraft) {
    return invoke<ReleaseProfileView>("save_release_profile", { projectId, draft });
  }
  exportReleaseMasterKey(projectId: string, request: ReleaseMasterKeyTransferRequest) {
    return invoke<ReleaseMasterKeyOperationResult>("export_release_master_key", { projectId, request });
  }
  importReleaseMasterKey(projectId: string, request: ReleaseMasterKeyTransferRequest) {
    return invoke<ReleaseMasterKeyOperationResult>("import_release_master_key", { projectId, request });
  }
  rotateReleaseMasterKey(projectId: string) {
    return invoke<ReleaseMasterKeyOperationResult>("rotate_release_master_key", { projectId });
  }
  listHostKeys(projectId: string) {
    return invoke<HostKeyObservation[]>("list_host_keys", { projectId });
  }
  captureHostKey(projectId: string, request: HostKeyCaptureRequest) {
    return invoke<HostKeyObservation>("capture_host_key", { projectId, request });
  }
  confirmHostKey(projectId: string, request: ConfirmHostKeyRequest) {
    return invoke<HostKeyObservation>("confirm_host_key", { projectId, request });
  }
}
