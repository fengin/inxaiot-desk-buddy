import { invoke } from "@tauri-apps/api/core";

import type {
  AioImportSession,
  AioNodeDetail,
  AioNodeListPage,
  ImportSelection,
  InventoryApplyOutcome,
  InventoryPreview,
  InventoryValues
} from "@/shared/model/aio";

export interface ListAioNodesQuery {
  search?: string;
  state?: string;
  page: number;
  pageSize: number;
}

export function listEdgeNodes(localProjectId: string, query: ListAioNodesQuery) {
  return invoke<AioNodeListPage>("list_edge_nodes", { localProjectId, query });
}

export function getEdgeNodeDetail(localProjectId: string, mac: string) {
  return invoke<AioNodeDetail>("get_edge_node_detail", { localProjectId, mac });
}

export function checkEdgeNodeServices(localProjectId: string, mac: string) {
  return invoke<{ taskId: string }>("check_edge_node_services", { localProjectId, mac });
}

export function previewInventoryImport(localProjectId: string, filePath: string) {
  return invoke<InventoryPreview>("preview_inventory_import", { localProjectId, filePath });
}

export function previewAioNodeCreate(localProjectId: string, values: InventoryValues) {
  return invoke<InventoryPreview>("preview_aio_node_create", { localProjectId, values });
}

export function getLatestInventoryImport(localProjectId: string) {
  return invoke<AioImportSession | null>("get_latest_inventory_import", { localProjectId });
}

export function updateInventoryImportSelection(
  localProjectId: string,
  sessionId: string,
  selections: ImportSelection[]
) {
  return invoke<AioImportSession>("update_inventory_import_selection", {
    localProjectId,
    sessionId,
    selections
  });
}

export function applyInventoryImport(localProjectId: string, sessionId: string) {
  return invoke<InventoryApplyOutcome>("apply_inventory_import", {
    localProjectId,
    sessionId
  });
}

export function discardInventoryImport(localProjectId: string, sessionId: string) {
  return invoke<void>("discard_inventory_import", { localProjectId, sessionId });
}
