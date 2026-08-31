export type ImportClassification =
  | "new_pending"
  | "existing_unchanged"
  | "existing_changed"
  | "platform_existing"
  | "conflict"
  | "invalid";

export interface FieldConflict {
  code: string;
  field: string;
  message: string;
  importValue?: string;
  workbenchValue?: string;
  platformValue?: string;
}

export interface ServiceVersionRecord {
  macNormalized: string;
  serviceName: string;
  expectedImageName?: string;
  expectedVersion?: string;
  observedImageName?: string;
  observedVersion?: string;
  observedAt?: string;
}

export interface AioNodeListItem {
  mac: string;
  macNormalized: string;
  name: string;
  ip: string;
  location: string;
  managementState: string;
  deployLabel: string;
  platformState: "online" | "offline" | "unknown";
  platformUpdatedAt: string;
  serviceState: "healthy" | "warning" | "unreachable" | "unknown";
  serviceLabel: string;
  lastOperation: string;
  platformId?: string;
  source: string;
  version: number;
  conflicts: FieldConflict[];
  versions: ServiceVersionRecord[];
}

export interface AioNodeStats {
  total: number;
  online: number;
  offline: number;
  pending: number;
  conflicts: number;
}

export interface PlatformRecordIssue {
  platformAioId: string;
  code: string;
  message: string;
  rawMac: string;
}

export interface AioNodeListPage {
  items: AioNodeListItem[];
  total: number;
  page: number;
  pageSize: number;
  stats: AioNodeStats;
  platformIssues: PlatformRecordIssue[];
  latestImportSessionId?: string;
  refreshedAt: string;
}

export interface PlatformNodeSnapshot {
  id: string;
  name: string;
  ip: string;
  macRaw: string;
  macNormalized: string;
  buildingId?: string;
  addrAlias?: string;
  status?: number;
  lastBeatTime?: number;
  lastSyncTime?: string;
}

export interface OperationRecordSummary {
  id: string;
  operationType: string;
  operationName: string;
  state: string;
  operatorName: string;
  endedAt?: string;
  resultSummary?: string;
}

export interface LocalCheckRecord {
  stepCode: string;
  state: string;
  message?: string;
  updatedAt: string;
}

export interface AioNodeDetail {
  node: AioNodeListItem;
  platform?: PlatformNodeSnapshot;
  versions: ServiceVersionRecord[];
  lastOperation?: OperationRecordSummary;
  latestSshCheck?: LocalCheckRecord;
  latestServiceCheck?: LocalCheckRecord;
}

export interface InventoryValues {
  name: string;
  ip: string;
  mac: string;
  buildingId?: string;
  regionId?: string;
  addrAlias?: string;
  floor?: string;
  location?: string;
  remark?: string;
}

export interface ReconciledImportItem {
  rowNumber: number;
  values: InventoryValues;
  macNormalized?: string;
  displayMac?: string;
  classification: ImportClassification;
  selected: boolean;
  errors: string[];
  conflicts: FieldConflict[];
  workbenchVersion?: number;
  platformAioId?: string;
  platformFingerprint?: string;
}

export interface ImportCounts {
  total: number;
  newPending: number;
  existingUnchanged: number;
  existingChanged: number;
  platformExisting: number;
  conflicts: number;
  invalid: number;
  selected: number;
}

export interface AioImportSession {
  id: string;
  localProjectId: string;
  fileName: string;
  filePath: string;
  state: string;
  counts: ImportCounts;
  createdAt: string;
  updatedAt: string;
  items: ReconciledImportItem[];
}

export interface InventoryPreview {
  session: AioImportSession;
  platformIssues: PlatformRecordIssue[];
}

export interface ImportSelection {
  rowNumber: number;
  selected: boolean;
}

export interface InventoryApplyOutcome {
  result: {
    operationId: string;
    appliedCount: number;
  };
  localSessionFinalized: boolean;
}
