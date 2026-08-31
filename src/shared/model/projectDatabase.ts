export type WorkbenchSchemaState =
  | "uninitialized"
  | "upgrade_required"
  | "ready"
  | "incompatible";

export interface WorkbenchSchemaStatus {
  state: WorkbenchSchemaState;
  currentVersion?: number;
  latestAvailableVersion: number;
  appliedMigrationCount: number;
  failedMigrationCount: number;
  missingTables: string[];
  forbiddenTables: string[];
  message: string;
}
