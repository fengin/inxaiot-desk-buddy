import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { useAioNodesStore } from "@/stores/aioNodes";
import { usePreferencesStore } from "@/stores/preferences";

const projectId = "project-shenzhen-bay";

describe("aio nodes store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    Reflect.deleteProperty(window, "__TAURI_INTERNALS__");
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

  it("previews selections and applies only selected final assets in browser mode", async () => {
    const store = useAioNodesStore();
    await store.refresh(projectId);
    await store.previewImport("C:/demo/inventory.csv");
    expect(store.importPreview?.session.counts.selected).toBe(2);
    await store.updateSelection({ rowNumber: 2, selected: false });
    expect(store.importPreview?.session.counts.selected).toBe(1);
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
        stats: { total: 250, online: 250, offline: 0, pending: 0, conflicts: 0 },
        platformIssues: [],
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
