import { createPinia, setActivePinia } from "pinia";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useDemoStore } from "@/stores/demo";

describe("demo store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    vi.useFakeTimers();
  });

  afterEach(() => vi.useRealTimers());

  it("keeps login sessions independent by project", () => {
    const store = useDemoStore();
    const target = store.projects.find((project) => project.connectionState === "login_required");
    expect(target).toBeDefined();
    store.loginProject(target!.id, "测试用户");
    expect(store.activeProjectId).toBe(target!.id);
    expect(target!.connectionState).toBe("ready");
    expect(store.projects[0]?.username).toBe("实施管理员");
  });

  it("applies only the final imported asset", () => {
    const store = useDemoStore();
    const before = store.nodes.length;
    store.applyMockImport();
    expect(store.nodes).toHaveLength(before + 1);
    expect(store.nodes[0]?.name).toBe("AIO-C栋-1F");
    store.applyMockImport();
    expect(store.nodes).toHaveLength(before + 1);
  });

  it("simulates a complete operation and records final results", async () => {
    const store = useDemoStore();
    const target = store.nodes[0]!;
    const taskId = store.startMockTask("service_upgrade", [target.mac], "device-edge 3.2.9");
    expect(store.tasks.find((task) => task.id === taskId)?.state).toBe("running");
    await vi.advanceTimersByTimeAsync(3800);
    expect(store.tasks.find((task) => task.id === taskId)?.state).toBe("succeeded");
    expect(store.history[0]?.artifact).toBe("device-edge 3.2.9");
    expect(store.nodes[0]?.deployLabel).toBe("已升级");
  });
});

