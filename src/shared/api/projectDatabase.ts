import { invoke } from "@tauri-apps/api/core";

import type { WorkbenchSchemaStatus } from "@/shared/model/projectDatabase";

export function getWorkbenchSchemaStatus(localProjectId: string) {
  return invoke<WorkbenchSchemaStatus>("get_workbench_schema_status", {
    localProjectId
  });
}

export function initializeOrUpgradeWorkbenchSchema(localProjectId: string) {
  return invoke<WorkbenchSchemaStatus>("initialize_or_upgrade_workbench_schema", {
    localProjectId
  });
}
