import { beforeEach, describe, expect, it, vi } from "vitest";

const { openMock, saveMock } = vi.hoisted(() => ({
  openMock: vi.fn(),
  saveMock: vi.fn()
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: openMock, save: saveMock }));

import { RealSystemDialogAdapter } from "@/shared/api/realSystemDialogAdapter";

describe("RealSystemDialogAdapter Tauri Dialog契约", () => {
  const adapter = new RealSystemDialogAdapter();
  const filters = [{ name: "INX 项目主密钥包", extensions: ["inxkey"] }];

  beforeEach(() => {
    openMock.mockReset();
    saveMock.mockReset();
  });

  it("keeps open and save dialogs behind the shared adapter", async () => {
    openMock.mockResolvedValue("D:\\secure\\project.inxkey");
    await expect(adapter.selectFile("导入项目主密钥", filters)).resolves.toBe(
      "D:\\secure\\project.inxkey"
    );
    expect(openMock).toHaveBeenLastCalledWith({
      directory: false,
      multiple: false,
      title: "导入项目主密钥",
      filters
    });

    saveMock.mockResolvedValue("D:\\secure\\project.inxkey");
    await expect(
      adapter.saveFile("导出项目主密钥", filters, "project.inxkey")
    ).resolves.toBe("D:\\secure\\project.inxkey");
    expect(saveMock).toHaveBeenLastCalledWith({
      title: "导出项目主密钥",
      filters,
      defaultPath: "project.inxkey"
    });
  });
});
