import { invoke } from "@tauri-apps/api/core";

import type { DataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import type {
  DataDirectoryStatus,
  DataDirectorySwitchRequest
} from "@/shared/model/dataDirectory";

export class RealDataDirectoryAdapter implements DataDirectoryAdapter {
  getStatus() {
    return invoke<DataDirectoryStatus>("get_data_directory_status");
  }

  scheduleSwitch(request: DataDirectorySwitchRequest) {
    return invoke<DataDirectoryStatus>("schedule_data_directory_switch", { request });
  }

  scheduleRollback() {
    return invoke<DataDirectoryStatus>("schedule_data_directory_rollback");
  }
}
