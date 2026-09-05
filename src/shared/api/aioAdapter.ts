import type { ListAioNodesQuery } from "@/shared/api/aioNodes";
import type {
  AioImportSession,
  AioNodeDetail,
  AioNodeListPage,
  ImportSelection,
  InventoryApplyOutcome,
  InventoryPreview
} from "@/shared/model/aio";

export interface AioAdapter {
  readonly real: boolean;
  listNodes(projectId: string, query: ListAioNodesQuery): Promise<AioNodeListPage>;
  getNodeDetail(projectId: string, mac: string): Promise<AioNodeDetail>;
  checkServices(projectId: string, mac: string): Promise<{ taskId: string }>;
  previewImport(projectId: string, filePath: string): Promise<InventoryPreview>;
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
