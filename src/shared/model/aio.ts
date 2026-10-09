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

/** 项目共享的成功部署版本记录；旧 observed 字段不作为本机服务观测。 */
export interface ServiceVersionRecord {
  macNormalized: string;
  serviceName: string;
  expectedImageName?: string;
  expectedVersion?: string;
  observedImageName?: string;
  observedVersion?: string;
  observedAt?: string;
}

export type ServiceObservationState = "normal" | "abnormal" | "version_mismatch" | "unknown";

export interface ServiceObservation {
  serviceName: string;
  state: ServiceObservationState;
  runtimeState: string;
  healthStatus?: string;
  expectedImage?: string;
  actualImage?: string;
  imageId?: string;
  message?: string;
  checkedAt: string;
  source: string;
}

export interface ServiceCheckReport {
  startedAt: string;
  checkedAt: string;
  source: string;
  scope: "all" | "service";
  serviceName?: string;
  expectedServices?: string[];
  services: ServiceObservation[];
  state: "succeeded" | "failed";
  error?: string;
}

/** 当前电脑保存的服务检查快照，按项目及 MAC 隔离。 */
export interface NodeServiceCheckSnapshot {
  expectedServices?: string[];
  services: ServiceObservation[];
  lastFullCheckAt?: string;
  lastAttempt?: ServiceCheckReport;
}

export type AioDeploymentState = "pending" | "deployed" | "attention" | "unconfirmed";

export interface AioNodeListItem {
  mac: string;
  macNormalized: string;
  name: string;
  ip: string;
  location: string;
  buildingId?: string;
  spacePath?: string;
  managementState: string;
  deploymentState: AioDeploymentState;
  deployLabel: string;
  platformState: "online" | "offline" | "unknown";
  platformUpdatedAt: string;
  serviceState: "healthy" | "warning" | "unreachable" | "unknown";
  serviceLabel: string;
  lastOperation: string;
  lastOperationAt?: string;
  platformId?: string;
  source: string;
  version: number;
  conflicts: FieldConflict[];
  versions: ServiceVersionRecord[];
  serviceCheck?: NodeServiceCheckSnapshot;
}

export interface AioNodeStats {
  total: number;
  online: number;
  offline: number;
  pending: number;
  deployed: number;
  attention: number;
  unconfirmed: number;
  conflicts: number;
}

export interface PlatformRecordIssue {
  platformAioId: string;
  name: string;
  ip: string;
  code: string;
  message: string;
  rawMac: string;
}

export interface AioNodeListPage {
  metadataWarning?: string | null;
  items: AioNodeListItem[];
  total: number;
  page: number;
  pageSize: number;
  stats: AioNodeStats;
  platformIssues: PlatformRecordIssue[];
  pagePlatformIssues: PlatformRecordIssue[];
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
  metadataWarning?: string | null;
  node: AioNodeListItem;
  platform?: PlatformNodeSnapshot;
  versions: ServiceVersionRecord[];
  lastOperation?: OperationRecordSummary;
  latestSshCheck?: LocalCheckRecord;
}

export interface InventoryValues {
  name: string;
  ip: string;
  mac: string;
  buildingId?: string;
  spacePath?: string;
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
    /** 本机导入会话编号，不是平台操作记录编号。 */
    operationId: string;
    appliedCount: number;
  };
  localSessionFinalized: boolean;
}

export interface UpdateAioNodeInput {
  mac: string;
  expectedVersion: number;
  platformBase?: PlatformNodeSnapshot | null;
  values: InventoryValues;
  forceTakeover?: boolean;
}
