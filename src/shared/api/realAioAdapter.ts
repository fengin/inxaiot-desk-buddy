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
import { invoke } from "@tauri-apps/api/core";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";

export class RealAioAdapter implements AioAdapter {
  readonly real = true;
  listSpaces(projectId: string) { return invoke<ProjectSpaceNode[]>("list_project_spaces", { localProjectId: projectId }); }
  updateNode(projectId: string, input: Parameters<AioAdapter["updateNode"]>[1]) { return invoke<void>("update_aio_node", { localProjectId: projectId, input }); }
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
