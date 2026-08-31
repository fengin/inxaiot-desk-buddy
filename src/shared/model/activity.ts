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
  name: string;
  state: ActivityTaskState;
  stage: string;
  progress: number | null;
  targetCount: number;
  completedCount: number;
  updatedAt: string;
  cancellable: boolean;
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
  localTaskId: string;
  sequence: number;
}

export interface CommandErrorDto {
  code: string;
  messageKey: string;
  params: Record<string, string>;
  traceId: string;
}
