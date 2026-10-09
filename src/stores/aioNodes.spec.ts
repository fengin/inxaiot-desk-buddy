import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { useAioNodesStore } from "@/stores/aioNodes";
import { usePreferencesStore } from "@/stores/preferences";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { useActivityStore } from "@/stores/activity";

const projectId = "project-shenzhen-bay";

describe("aio nodes store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
    configureAioAdapter(new FixtureAioAdapter());
    configureActivityAdapter(new FixtureActivityAdapter());
  });

  afterEach(() => {
    useActivityStore().dispose();
    vi.useRealTimers();
  });

  it("普通刷新只读记录，显式服务检查防重复且终态刷新列表与详情", async () => {
    vi.useFakeTimers();
    const adapter = new FixtureAioAdapter();
    const check = vi.spyOn(adapter, "checkServices");
    const list = vi.spyOn(adapter, "listNodes");
    const detail = vi.spyOn(adapter, "getNodeDetail");
    configureAioAdapter(adapter);
    const store = useAioNodesStore();
    const activity = useActivityStore();
    await store.refresh(projectId, "AIO-1F");
    const target = store.nodes[0]!;
    await store.loadDetail(target.mac);
    expect(check).not.toHaveBeenCalled();
    const submitted = store.checkServices(target.mac);
    await store.checkServices(target.mac);
    const result = await submitted;
    expect(check).toHaveBeenCalledTimes(1);
    expect(activity.selectedTaskId).toBe(result?.taskId);
    expect(activity.panelTab).toBe("logs");
    expect(activity.panelOpen).toBe(true);
    expect(store.serviceInspection(target.mac)).toBeDefined();
    await vi.advanceTimersByTimeAsync(600);
    expect(store.serviceInspection(target.mac)).toBeUndefined();
    expect(list).toHaveBeenCalledTimes(2);
    expect(list.mock.lastCall?.[1].search).toBe("AIO-1F");
    expect(detail).toHaveBeenCalledTimes(2);
    expect(store.detail?.node.serviceCheck?.services).toHaveLength(target.versions.length);
    expect(store.detail?.node.serviceCheck?.lastFullCheckAt).toBeTruthy();
  });

  it("检查提交失败后释放按钮并保留之前的实测记录", async () => {
    const adapter = new FixtureAioAdapter();
    vi.spyOn(adapter, "checkServices").mockRejectedValue(new Error("检查任务正在执行"));
    configureAioAdapter(adapter);
    const store = useAioNodesStore();
    await store.refresh(projectId);
    const target = store.nodes[0]!;
    await store.loadDetail(target.mac);
    const detail = store.detail;
    await expect(store.checkServices(target.mac)).rejects.toThrow("检查任务正在执行");
    expect(store.serviceInspection(target.mac)).toBeUndefined();
    expect(store.detail).toBe(detail);
    expect(store.error).toContain("检查任务正在执行");
  });

  it("切换项目后旧检查完成不会刷新新项目详情", async () => {
    vi.useFakeTimers();
    const adapter = new FixtureAioAdapter();
    const list = vi.spyOn(adapter, "listNodes");
    configureAioAdapter(adapter);
    const store = useAioNodesStore();
    await store.refresh(projectId);
    const target = store.nodes[0]!;
    await store.checkServices(target.mac);
    await store.refresh("another-project");
    await store.loadDetail(target.mac);
    const callsBeforeCompletion = list.mock.calls.length;
    await vi.advanceTimersByTimeAsync(600);
    expect(list).toHaveBeenCalledTimes(callsBeforeCompletion);
    expect(store.detail?.node.serviceCheck).toBeUndefined();
  });

  it("keeps browser fixtures behind the same real node DTO", async () => {
    const store = useAioNodesStore();
    await store.refresh(projectId);
    expect(store.realBackend).toBe(false);
    expect(store.nodes.length).toBe(8);
    expect(store.stats.total).toBe(8);
    expect(store.nodes[0]?.macNormalized).not.toContain(":");

    await store.refresh(projectId, "不存在节点");
    expect(store.nodes).toHaveLength(0);
    expect(store.stats.total).toBe(8);
  });

  it("已部署筛选合并两种平台注册记录，待处理与待实施独立统计", async () => {
    const store = useAioNodesStore();
    await store.refresh(projectId, "", "deployed");
    expect(store.nodes).toHaveLength(6);
    expect(store.nodes.every(node => node.deploymentState === "deployed" && node.deployLabel === "已部署")).toBe(true);
    expect(store.nodes.some(node => node.managementState === "managed")).toBe(true);
    expect(store.nodes.some(node => node.managementState === "platform_existing")).toBe(true);
    expect(store.stats).toMatchObject({ total: 8, deployed: 6, pending: 1, attention: 1, unconfirmed: 0 });
    await store.refresh(projectId, "", "attention");
    expect(store.nodes.map(node => node.deployLabel)).toEqual(["待处理"]);
    expect(store.nodes[0]?.conflicts[0]?.message).toContain("名称和 IP");
    await store.refresh(projectId, "", "pending");
    expect(store.nodes.map(node => node.deployLabel)).toEqual(["待实施"]);
    await store.refresh(projectId, "", "unconfirmed");
    expect(store.nodes).toHaveLength(0);
  });

  it("待处理当前页与平台异常全集分别保存，切换筛选不保留上一页异常", async () => {
    const adapter = new FixtureAioAdapter();
    const original = adapter.listNodes.bind(adapter);
    const issues = Array.from({ length: 30 }, (_, index) => ({ platformAioId: String(index), name: `异常${index}`, ip: "", rawMac: "", code: "MISSING_MAC", message: "MAC 地址缺失" }));
    vi.spyOn(adapter, "listNodes").mockImplementation(async (id, query) => ({
      ...await original(id, query), platformIssues: issues,
      pagePlatformIssues: query.state === "attention" ? issues.slice(20, 30) : []
    }));
    configureAioAdapter(adapter);
    const store = useAioNodesStore();
    await store.refresh(projectId, "", "attention");
    expect(store.platformIssues).toHaveLength(30);
    expect(store.pagePlatformIssues.map(issue => issue.platformAioId)).toEqual(issues.slice(20).map(issue => issue.platformAioId));
    await store.previewImport("C:/demo/inventory.csv");
    expect(store.pagePlatformIssues).toHaveLength(10);
    await store.refresh(projectId, "", "deployed");
    expect(store.platformIssues).toHaveLength(30);
    expect(store.pagePlatformIssues).toHaveLength(0);
  });

  it("previews selections and applies only selected final assets in browser mode", async () => {
    const store = useAioNodesStore();
    await store.refresh(projectId);
    await store.previewImport("C:/demo/inventory.csv");
    expect(store.importPreview?.session.counts.selected).toBe(1);
    expect(store.importPreview?.session.items.find(row => row.platformAioId)?.selected).toBe(false);
    await expect(store.updateSelection({ rowNumber: 4, selected: true })).rejects.toThrow("已注册");
    await store.updateSelection({ rowNumber: 2, selected: false });
    expect(store.importPreview?.session.counts.selected).toBe(0);
    await store.updateSelection({ rowNumber: 2, selected: true });
    const outcome = await store.applyImport();
    expect(outcome.result.appliedCount).toBe(1);
    expect(store.importPreview?.session.state).toBe("applied");
  });

  it("follows the shared 20/50/100 page-size preference", async () => {
    const preferences = usePreferencesStore();
    const store = useAioNodesStore();
    expect(store.pageSize).toBe(preferences.pageSize);
    store.page = 3;
    preferences.pageSize = 100;
    await nextTick();
    expect(store.pageSize).toBe(100);
    expect(store.page).toBe(1);
  });

  it("loads a complete 250-node selection set across 100-item pages", async () => {
    const adapter = new FixtureAioAdapter();
    const template = (await adapter.listNodes(projectId, { page: 1, pageSize: 100 })).items[0]!;
    const all = Array.from({ length: 250 }, (_, index) => {
      const macNormalized = index.toString(16).padStart(12, "0").toUpperCase();
      return {
        ...structuredClone(template),
        mac: macNormalized.match(/.{2}/g)!.join(":"),
        macNormalized,
        name: `selection-node-${index}`
      };
    });
    const list = vi.spyOn(adapter, "listNodes").mockImplementation(async (_id, query) => {
      const start = (query.page - 1) * query.pageSize;
      return {
        items: structuredClone(all.slice(start, start + query.pageSize)),
        total: all.length,
        page: query.page,
        pageSize: query.pageSize,
        stats: { total: 250, online: 250, offline: 0, pending: 0, deployed: 250, attention: 0, unconfirmed: 0, conflicts: 0 },
        platformIssues: [],
        pagePlatformIssues: [],
        refreshedAt: new Date().toISOString()
      };
    });
    configureAioAdapter(adapter);
    const store = useAioNodesStore();
    const selected = await store.loadSelectionNodes(projectId);
    expect(selected).toHaveLength(250);
    expect(store.selectionNodes).toHaveLength(250);
    expect(list).toHaveBeenCalledTimes(3);
    expect(list.mock.calls.map(([, query]) => query.page)).toEqual([1, 2, 3]);
  });
});
