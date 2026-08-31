import {
  applyInventoryImport,
  discardInventoryImport,
  getEdgeNodeDetail,
  getLatestInventoryImport,
  listEdgeNodes,
  previewInventoryImport,
  updateInventoryImportSelection
} from "@/shared/api/aioNodes";
import type { AioAdapter } from "@/shared/api/aioAdapter";

export class RealAioAdapter implements AioAdapter {
  readonly real = true;
  listNodes(projectId: string, query: Parameters<AioAdapter["listNodes"]>[1]) { return listEdgeNodes(projectId, query); }
  getNodeDetail(projectId: string, mac: string) { return getEdgeNodeDetail(projectId, mac); }
  previewImport(projectId: string, filePath: string) { return previewInventoryImport(projectId, filePath); }
  getLatestImport(projectId: string) { return getLatestInventoryImport(projectId); }
  updateImportSelection(projectId: string, sessionId: string, selections: Parameters<AioAdapter["updateImportSelection"]>[2]) {
    return updateInventoryImportSelection(projectId, sessionId, selections);
  }
  applyImport(projectId: string, sessionId: string) { return applyInventoryImport(projectId, sessionId); }
  discardImport(projectId: string, sessionId: string) { return discardInventoryImport(projectId, sessionId); }
}
