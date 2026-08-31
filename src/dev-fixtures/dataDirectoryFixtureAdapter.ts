import type { DataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import type {
  DataDirectoryStatus,
  DataDirectorySwitchRequest
} from "@/shared/model/dataDirectory";

export class FixtureDataDirectoryAdapter implements DataDirectoryAdapter {
  private status: DataDirectoryStatus = {
    activeDirectory: "D:\\INX\\DeskBuddy-Fixture",
    defaultDirectory: "C:\\Users\\Public\\INX\\DeskBuddy-Fixture",
    restartRequired: false,
    firstSetup: true
  };

  async getStatus() {
    return structuredClone(this.status);
  }

  async scheduleSwitch(request: DataDirectorySwitchRequest) {
    this.status = {
      ...this.status,
      pendingDirectory: request.targetDirectory,
      pendingMode: request.mode,
      restartRequired: true,
      lastSwitchError: undefined
    };
    return this.getStatus();
  }

  async scheduleRollback() {
    if (!this.status.previousDirectory) throw new Error("Fixture没有上一数据目录");
    this.status = {
      ...this.status,
      pendingDirectory: this.status.previousDirectory,
      pendingMode: "use_existing",
      restartRequired: true
    };
    return this.getStatus();
  }
}
