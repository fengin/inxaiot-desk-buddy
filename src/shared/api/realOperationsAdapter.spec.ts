import { beforeEach, describe, expect, it, vi } from "vitest";

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

import { RealOperationsAdapter } from "@/shared/api/realOperationsAdapter";

describe("RealOperationsAdapter Tauri Command 契约", () => {
  const adapter = new RealOperationsAdapter();

  beforeEach(() => invokeMock.mockReset().mockResolvedValue({}));

  it("maps task and shared operation history queries to stable command names", async () => {
    const plan = {
      mode: "full_upgrade" as const,
      targetMacs: ["001122334455"],
      artifactPath: "C:/release",
      artifactName: "Release",
      artifactVersion: "1",
      batchSize: 1,
      concurrency: 1
    };
    await adapter.preflight("project-1", plan);
    expect(invokeMock).toHaveBeenLastCalledWith("preflight_deployment", {
      localProjectId: "project-1",
      input: plan
    });
    await adapter.submit("project-1", plan);
    expect(invokeMock).toHaveBeenLastCalledWith("submit_deployment", {
      localProjectId: "project-1",
      input: plan
    });

    await adapter.getTask("project-1", "task-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_deployment_task", {
      localProjectId: "project-1",
      taskId: "task-1"
    });

    const query = { page: 1, pageSize: 20 };
    await adapter.listHistory("project-1", query);
    expect(invokeMock).toHaveBeenLastCalledWith("list_operation_history", {
      localProjectId: "project-1",
      query
    });

    await adapter.getHistoryDetail("project-1", "operation-1");
    expect(invokeMock).toHaveBeenLastCalledWith("get_operation_history_detail", {
      localProjectId: "project-1",
      operationId: "operation-1"
    });
  });
});
