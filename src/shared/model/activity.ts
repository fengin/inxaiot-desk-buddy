export type ActivityTaskState =
  | "draft"
  | "checking"
  | "check_failed"
  | "ready"
  | "queued"
  | "running"
  | "cancelling"
  | "cancelled"
  | "succeeded"
  | "partially_succeeded"
  | "failed"
  | "interrupted"
  | "finalizing_failed";

export interface ActivityTask {
  id: string;
  projectId: string;
  domainType: string;
  operationType: string;
  name: string;
  state: ActivityTaskState;
  stage: string;
  progress: number | null;
  targetCount: number;
  completedCount: number;
  updatedAt: string;
  cancellable: boolean;
}

export interface FinalizationRetryResult {
  task: ActivityTask;
  takeoverRequired: boolean;
  message: string;
}

export type ActivityLogLevel = "INFO" | "WARN" | "ERROR";

export interface ActivityLogEntry {
  id: string;
  taskId: string;
  sequence: number;
  timestamp: string;
  level: ActivityLogLevel;
  source: string;
  message: string;
}

export interface ActivityLogPage {
  items: ActivityLogEntry[];
  nextOffset: number;
  hasMore: boolean;
}

export interface TaskEventPayload {
  eventId: string;
  localTaskId: string;
  operationRecordId?: string | null;
  sequence: number;
  localProjectId: string;
  domainType: string;
  resourceType?: string | null;
  resourceKey?: string | null;
  stage: string;
  status: string;
  progressCurrent?: number | null;
  progressTotal?: number | null;
  level: "info" | "warn" | "error";
  messageCode: string;
  messageParams: Record<string, string>;
  message?: string | null;
  timestamp: string;
}

export interface CommandErrorDto {
  code: string;
  messageKey: string;
  params: Record<string, string>;
  traceId: string;
  fieldErrors?: Record<string, string>;
}
