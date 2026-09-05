import { invoke } from "@tauri-apps/api/core";

import type { ServiceImageInspection } from "@/shared/model/release";

export function inspectServiceImage(path: string, expectedImage?: string) {
  return invoke<ServiceImageInspection>("inspect_service_image", {
    path,
    expectedImage
  });
}
