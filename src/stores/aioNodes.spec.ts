import { createPinia, setActivePinia } from "pinia";
import { nextTick } from "vue";
import { beforeEach, describe, expect, it } from "vitest";

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
});
