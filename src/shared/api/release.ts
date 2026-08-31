import { invoke } from "@tauri-apps/api/core";

import type {
  DeploymentExecutionSummary,
  DeploymentPlanInput,
  ReleaseValidation,
  ServiceImageInspection
} from "@/shared/model/release";

export function validateReleasePackage(path: string) {
  return invoke<ReleaseValidation>("validate_release_package", { path });
}

export function executeDeployment(
  localProjectId: string,
  plan: DeploymentPlanInput
) {
  return invoke<DeploymentExecutionSummary>("execute_deployment", {
    localProjectId,
    input: { plan }
  });
}

export function inspectServiceImage(path: string, expectedImage?: string) {
  return invoke<ServiceImageInspection>("inspect_service_image", {
    path,
    expectedImage
  });
}
