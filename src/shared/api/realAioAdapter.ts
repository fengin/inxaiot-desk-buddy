import {
  applyInventoryImport,
  checkEdgeNodeServices,
  discardInventoryImport,
  getEdgeNodeDetail,
  getLatestInventoryImport,
  listEdgeNodes,
  previewInventoryImport,
  previewAioNodeCreate,
  updateInventoryImportSelection
} from "@/shared/api/aioNodes";
import type { AioAdapter } from "@/shared/api/aioAdapter";

export class RealAioAdapter implements AioAdapter {
  readonly real = true;
  listNodes(projectId: string, query: Parameters<AioAdapter["listNodes"]>[1]) { return listEdgeNodes(projectId, query); }
  getNodeDetail(projectId: string, mac: string) { return getEdgeNodeDetail(projectId, mac); }
  checkServices(projectId: string, mac: string) { return checkEdgeNodeServices(projectId, mac); }
  previewImport(projectId: string, filePath: string) { return previewInventoryImport(projectId, filePath); }
  previewCreate(projectId: string, values: Parameters<AioAdapter["previewCreate"]>[1]) { return previewAioNodeCreate(projectId, values); }
  getLatestImport(projectId: string) { return getLatestInventoryImport(projectId); }
  updateImportSelection(projectId: string, sessionId: string, selections: Parameters<AioAdapter["updateImportSelection"]>[2]) {
    return updateInventoryImportSelection(projectId, sessionId, selections);
  }
  applyImport(projectId: string, sessionId: string) { return applyInventoryImport(projectId, sessionId); }
  discardImport(projectId: string, sessionId: string) { return discardInventoryImport(projectId, sessionId); }
}
