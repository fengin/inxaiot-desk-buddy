import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { RealDataDirectoryAdapter } from "@/shared/api/realDataDirectoryAdapter";

describe("RealDataDirectoryAdapter Tauri Command 契约", () => {
  const adapter = new RealDataDirectoryAdapter();

  beforeEach(() => invokeMock.mockReset().mockResolvedValue({}));

  it("maps status, switch and rollback to stable command names", async () => {
    await adapter.getStatus();
    expect(invokeMock).toHaveBeenLastCalledWith("get_data_directory_status");
    const request = { targetDirectory: "D:\\INX\\DeskBuddy", mode: "migrate" as const };
    await adapter.scheduleSwitch(request);
    expect(invokeMock).toHaveBeenLastCalledWith("schedule_data_directory_switch", { request });
    await adapter.scheduleRollback();
    expect(invokeMock).toHaveBeenLastCalledWith("schedule_data_directory_rollback");
  });
});
