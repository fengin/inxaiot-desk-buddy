import { invoke } from "@tauri-apps/api/core";
import type { OperationHistoryPage, OperationHistoryQuery, OperationHistoryDetail } from "@/shared/model/deploymentWorkflow";

export function listBusinessOperationHistory(localProjectId: string, domainType: string, query: OperationHistoryQuery) {
  return invoke<OperationHistoryPage>("list_business_operation_history", { localProjectId, domainType, query });
}
export function getBusinessOperationHistoryDetail(localProjectId: string, domainType: string, operationId: string) {
  return invoke<OperationHistoryDetail>("get_business_operation_history_detail", { localProjectId, domainType, operationId });
}

export interface ScreenOperationLock {
  resourceType: string;
  resourceKey: string;
  ownerInstanceId: string;
  ownerUser: string;
  fencingToken: number;
}
export function previewScreenLocks(localProjectId: string, operationId: string) {
  return invoke<ScreenOperationLock[]>("screen_lock_preview", {localProjectId, operationId});
}
export function releaseScreenLocks(localProjectId: string, operationId: string, expected: ScreenOperationLock[]) {
  return invoke<void>("screen_lock_release", {localProjectId, operationId, expected, confirmed: true});
}
