import { beforeEach, describe, expect, it, vi } from "vitest";

const { openMock } = vi.hoisted(() => ({ openMock: vi.fn() }));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock }));

import { RealSystemDialogAdapter } from "@/shared/api/realSystemDialogAdapter";

describe("RealSystemDialogAdapter Tauri Dialog契约", () => {
  const adapter = new RealSystemDialogAdapter();
  const filters = [{ name: "CSV 文件", extensions: ["csv"] }];

  beforeEach(() => {
    openMock.mockReset();
  });

  it("keeps file selection behind the shared adapter", async () => {
    openMock.mockResolvedValue("D:\\import\\inventory.csv");
    await expect(adapter.selectFile("选择一体机清单", filters)).resolves.toBe(
      "D:\\import\\inventory.csv"
    );
    expect(openMock).toHaveBeenLastCalledWith({
      directory: false,
      multiple: false,
      title: "选择一体机清单",
      filters
    });
  });
});
