import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useAioAdapter } from "@/shared/api/aioAdapter";
import type {
  AioNodeDetail,
  AioNodeListItem,
  AioNodeStats,
  ImportSelection,
  InventoryApplyOutcome,
  InventoryPreview
} from "@/shared/model/aio";
import { usePreferencesStore } from "@/stores/preferences";

const emptyStats = (): AioNodeStats => ({ total: 0, online: 0, offline: 0, pending: 0, conflicts: 0 });

export const useAioNodesStore = defineStore("aio-nodes", () => {
  const preferences = usePreferencesStore();
  const realBackend = useAioAdapter().real;
  const projectId = ref("");
  const nodes = ref<AioNodeListItem[]>([]);
  const stats = ref<AioNodeStats>(emptyStats());
  const total = ref(0);
  const page = ref(1);
  const pageSize = computed(() => preferences.pageSize);
  const refreshedAt = ref("");
  const latestImportSessionId = ref("");
  const loading = ref(false);
  const error = ref("");
  const detail = ref<AioNodeDetail>();
  const detailLoading = ref(false);
  const importPreview = ref<InventoryPreview>();
  const importLoading = ref(false);

  watch(pageSize, () => { page.value = 1; });

  async function refresh(nextProjectId = projectId.value, search = "", state = "all") {
    if (!nextProjectId) return;
    projectId.value = nextProjectId;
    loading.value = true;
    error.value = "";
    try {
      const result = await useAioAdapter().listNodes(nextProjectId, {
        search: search.trim() || undefined,
        state,
        page: page.value,
        pageSize: pageSize.value
      });
      nodes.value = result.items;
      stats.value = result.stats;
      total.value = result.total;
      refreshedAt.value = result.refreshedAt;
      latestImportSessionId.value = result.latestImportSessionId ?? "";
    } catch (cause) {
      error.value = commandErrorText(cause, "一体机数据读取失败");
      nodes.value = [];
      stats.value = emptyStats();
      total.value = 0;
    } finally {
      loading.value = false;
    }
  }

  async function loadDetail(mac: string) {
    detailLoading.value = true;
    try {
      detail.value = await useAioAdapter().getNodeDetail(projectId.value, mac);
    } catch (cause) {
      error.value = commandErrorText(cause, "一体机详情读取失败");
      detail.value = undefined;
    } finally {
      detailLoading.value = false;
    }
  }

  async function resumeImport() {
    importLoading.value = true;
    try {
      const session = await useAioAdapter().getLatestImport(projectId.value);
      importPreview.value = session ? { session, platformIssues: [] } : undefined;
      return importPreview.value;
    } finally {
      importLoading.value = false;
    }
  }

  async function previewImport(filePath: string) {
    importLoading.value = true;
    try {
      importPreview.value = await useAioAdapter().previewImport(projectId.value, filePath);
      return importPreview.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "导入预览失败");
      throw cause;
    } finally {
      importLoading.value = false;
    }
  }

  async function updateSelection(selection: ImportSelection) {
    const session = importPreview.value?.session;
    if (!session) return;
    try {
      importPreview.value = {
        ...importPreview.value!,
        session: await useAioAdapter().updateImportSelection(projectId.value, session.id, [selection])
      };
    } catch (cause) {
      error.value = commandErrorText(cause, "更新导入选择失败");
      throw cause;
    }
  }

  async function applyImport(): Promise<InventoryApplyOutcome> {
    const session = importPreview.value?.session;
    if (!session) throw new Error("没有可应用的导入预览");
    importLoading.value = true;
    try {
      const outcome = await useAioAdapter().applyImport(projectId.value, session.id);
      importPreview.value = { ...importPreview.value!, session: { ...session, state: "applied" } };
      await refresh(projectId.value);
      return outcome;
    } catch (cause) {
      error.value = commandErrorText(cause, "应用导入失败");
      throw cause;
    } finally {
      importLoading.value = false;
    }
  }

  async function discardImport() {
    const session = importPreview.value?.session;
    if (!session) return;
    try {
      await useAioAdapter().discardImport(projectId.value, session.id);
      importPreview.value = undefined;
      latestImportSessionId.value = "";
    } catch (cause) {
      error.value = commandErrorText(cause, "放弃导入失败");
      throw cause;
    }
  }

  return {
    realBackend, nodes, stats, total, page, pageSize, refreshedAt, latestImportSessionId,
    loading, error, detail, detailLoading, importPreview, importLoading,
    refresh, loadDetail, resumeImport, previewImport, updateSelection, applyImport, discardImport
  };
});
