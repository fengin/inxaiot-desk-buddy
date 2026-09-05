import { describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));
import { RealAioAdapter } from "@/shared/api/realAioAdapter";

describe("一体机服务检查 IPC 契约", () => {
  it("以项目和 MAC 提交后台检查任务，读取详情仍使用只读命令", async () => {
    const adapter = new RealAioAdapter();
    invokeMock.mockResolvedValue({ taskId: "inspection-1" });
    expect(await adapter.checkServices("project-1", "001122334455")).toEqual({ taskId: "inspection-1" });
    expect(invokeMock).toHaveBeenLastCalledWith("check_edge_node_services", { localProjectId: "project-1", mac: "001122334455" });
    await adapter.getNodeDetail("project-1", "001122334455");
    expect(invokeMock).toHaveBeenLastCalledWith("get_edge_node_detail", { localProjectId: "project-1", mac: "001122334455" });
  });
});
