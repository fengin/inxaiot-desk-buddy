import { invoke } from "@tauri-apps/api/core";

import type {
  ReleaseValidation,
  ServiceImageInspection
} from "@/shared/model/release";

export function validateReleasePackage(path: string) {
  return invoke<ReleaseValidation>("validate_release_package", { path });
}

export function inspectServiceImage(path: string, expectedImage?: string) {
  return invoke<ServiceImageInspection>("inspect_service_image", {
    path,
    expectedImage
  });
}
