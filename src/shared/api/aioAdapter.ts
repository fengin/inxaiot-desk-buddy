import type { ListAioNodesQuery } from "@/shared/api/aioNodes";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import type {
  AioImportSession,
  AioNodeDetail,
  AioNodeListPage,
  ImportSelection,
  InventoryApplyOutcome,
  InventoryPreview,
  InventoryValues
} from "@/shared/model/aio";

export interface AioAdapter {
  readonly real: boolean;
  listSpaces(projectId: string): Promise<ProjectSpaceNode[]>;
  updateNode(projectId: string, input: import("@/shared/model/aio").UpdateAioNodeInput): Promise<void>;
  listNodes(projectId: string, query: ListAioNodesQuery): Promise<AioNodeListPage>;
  getNodeDetail(projectId: string, mac: string): Promise<AioNodeDetail>;
  checkServices(projectId: string, mac: string): Promise<{ taskId: string }>;
  previewImport(projectId: string, filePath: string): Promise<InventoryPreview>;
  previewCreate(projectId: string, values: InventoryValues): Promise<InventoryPreview>;
  getLatestImport(projectId: string): Promise<AioImportSession | null>;
  updateImportSelection(projectId: string, sessionId: string, selections: ImportSelection[]): Promise<AioImportSession>;
  applyImport(projectId: string, sessionId: string): Promise<InventoryApplyOutcome>;
  discardImport(projectId: string, sessionId: string): Promise<void>;
}

let adapter: AioAdapter | undefined;

export function configureAioAdapter(next: AioAdapter) { adapter = next; }
export function useAioAdapter() {
  if (!adapter) throw new Error("AIO Adapter 尚未初始化");
  return adapter;
}
