import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ScreenSnapshot } from "@/shared/model/screen";
import { useSmartScreensStore } from "@/stores/smartScreens";

const adapter = vi.hoisted(() => ({ load: vi.fn(), subscribe: vi.fn() }));
vi.mock("@/shared/api/screenAdapter", () => ({ useScreenAdapter: () => adapter }));

const listeners = new Set<(projectId: string) => void>();
function notify(projectId: string) { for (const listener of listeners) listener(projectId); }
function snapshot(platformAvailable: boolean, platformMessage?: string): ScreenSnapshot {
  return { screens: [], spaces: [], tasks: [], ignoredPairs: [], platformAvailable, platformMessage };
}
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((accept, fail) => { resolve = accept; reject = fail; });
  return { promise, resolve, reject };
}

describe("智能屏平台刷新与任务通知", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    listeners.clear();
    adapter.load.mockReset();
    adapter.subscribe.mockReset().mockImplementation((listener: (projectId: string) => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    });
  });

  it("平台读取较慢时合并多条任务通知，完成后只补一次本机读取", async () => {
    const platform = deferred<ScreenSnapshot>();
    const local = deferred<ScreenSnapshot>();
    adapter.load.mockResolvedValueOnce(snapshot(false, "需要登录"))
      .mockReturnValueOnce(platform.promise)
      .mockReturnValueOnce(local.promise);
    const store = useSmartScreensStore();
    await store.bindProject("project-a");

    const refreshing = store.refresh({ refreshPlatform: true });
    notify("project-a");
    notify("project-a");
    notify("project-a");
    expect(adapter.load).toHaveBeenCalledTimes(2);
    expect(store.loading).toBe(true);

    platform.resolve(snapshot(true));
    await refreshing;
    expect(store.platformAvailable).toBe(true);
    expect(store.snapshot.platformMessage).toBeUndefined();
    expect(adapter.load.mock.calls).toEqual([
      ["project-a", { refreshPlatform: true }],
      ["project-a", { refreshPlatform: true }],
      ["project-a", { refreshPlatform: false }]
    ]);
    expect(store.loading).toBe(true);

    local.resolve(snapshot(true));
    await vi.waitFor(() => expect(store.loading).toBe(false));
    expect(adapter.load).toHaveBeenCalledTimes(3);
  });

  it("较早的未登录读取迟到，不覆盖登录后成功读取的平台结果", async () => {
    const old = deferred<ScreenSnapshot>();
    adapter.load.mockReturnValueOnce(old.promise).mockResolvedValueOnce(snapshot(true));
    const store = useSmartScreensStore();
    const binding = store.bindProject("project-a");

    await store.refresh({ refreshPlatform: true });
    old.resolve(snapshot(false, "平台会话缺失"));
    await binding;
    expect(store.platformAvailable).toBe(true);
    expect(store.snapshot.platformMessage).toBeUndefined();
    expect(store.error).toBe("");
    expect(store.loading).toBe(false);
  });

  it("切换项目后丢弃旧项目排队的通知和迟到结果，不替换新项目内容", async () => {
    const old = deferred<ScreenSnapshot>();
    adapter.load.mockReturnValueOnce(old.promise)
      .mockResolvedValueOnce(snapshot(true, "项目B资料"));
    const store = useSmartScreensStore();
    const binding = store.bindProject("project-a");
    notify("project-a");
    notify("project-a");

    await store.bindProject("project-b");
    notify("project-a");
    old.resolve(snapshot(false, "项目A需要登录"));
    await binding;
    expect(store.projectId).toBe("project-b");
    expect(store.snapshot.platformMessage).toBe("项目B资料");
    expect(store.platformAvailable).toBe(true);
    expect(store.loading).toBe(false);
    expect(listeners.size).toBe(1);
    expect(adapter.load.mock.calls).toEqual([
      ["project-a", { refreshPlatform: true }],
      ["project-b", { refreshPlatform: true }]
    ]);
  });

  it("旧项目的请求失败不污染新项目的错误状态，也不发出排队读取", async () => {
    const old = deferred<ScreenSnapshot>();
    adapter.load.mockReturnValueOnce(old.promise).mockResolvedValueOnce(snapshot(true));
    const store = useSmartScreensStore();
    const binding = store.bindProject("project-a");
    notify("project-a");

    await store.bindProject("project-b");
    old.reject(new Error("项目A连接失败"));
    await binding;
    expect(store.projectId).toBe("project-b");
    expect(store.platformAvailable).toBe(true);
    expect(store.error).toBe("");
    expect(adapter.load).toHaveBeenCalledTimes(2);
  });

  it("页面停止订阅后，未完成的平台读取和排队任务通知都不继续更新", async () => {
    const pending = deferred<ScreenSnapshot>();
    adapter.load.mockReturnValueOnce(pending.promise);
    const store = useSmartScreensStore();
    const binding = store.bindProject("project-a");
    notify("project-a");
    store.stop();
    notify("project-a");

    pending.resolve(snapshot(false, "页面关闭后的旧结果"));
    await binding;
    expect(listeners.size).toBe(0);
    expect(store.snapshot.platformMessage).toBeUndefined();
    expect(adapter.load).toHaveBeenCalledTimes(1);
  });
});
