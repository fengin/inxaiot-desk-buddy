import type {
  DataDirectoryStatus,
  DataDirectorySwitchRequest
} from "@/shared/model/dataDirectory";

export interface DataDirectoryAdapter {
  getStatus(): Promise<DataDirectoryStatus>;
  scheduleSwitch(request: DataDirectorySwitchRequest): Promise<DataDirectoryStatus>;
  scheduleRollback(): Promise<DataDirectoryStatus>;
}

let adapter: DataDirectoryAdapter | undefined;

export function configureDataDirectoryAdapter(next: DataDirectoryAdapter) {
  adapter = next;
}

export function useDataDirectoryAdapter(): DataDirectoryAdapter {
  if (!adapter) throw new Error("数据目录 Adapter 尚未初始化");
  return adapter;
}
