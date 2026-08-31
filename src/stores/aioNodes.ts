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
  InventoryPreview,
  PlatformRecordIssue
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
  const platformIssues = ref<PlatformRecordIssue[]>([]);
  const importLoading = ref(false);
  let listRequest = 0;
  let detailRequest = 0;
  let importRequest = 0;

  watch(pageSize, () => { page.value = 1; });

  async function refresh(nextProjectId = projectId.value, search = "", state = "all") {
    if (!nextProjectId) return;
    const request = ++listRequest;
    if (projectId.value !== nextProjectId) {
      detailRequest += 1;
      importRequest += 1;
      nodes.value = [];
      stats.value = emptyStats();
      total.value = 0;
      detail.value = undefined;
      importPreview.value = undefined;
      latestImportSessionId.value = "";
      platformIssues.value = [];
    }
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
      if (request === listRequest && projectId.value === nextProjectId) {
        nodes.value = result.items;
        stats.value = result.stats;
        total.value = result.total;
        refreshedAt.value = result.refreshedAt;
        latestImportSessionId.value = result.latestImportSessionId ?? "";
        platformIssues.value = result.platformIssues;
      }
    } catch (cause) {
      if (request === listRequest) {
        error.value = commandErrorText(cause, "一体机数据读取失败");
        nodes.value = [];
        stats.value = emptyStats();
        total.value = 0;
        platformIssues.value = [];
      }
    } finally {
      if (request === listRequest) loading.value = false;
    }
  }

  async function loadDetail(mac: string) {
    const request = ++detailRequest;
    const expectedProjectId = projectId.value;
    detailLoading.value = true;
    try {
      const result = await useAioAdapter().getNodeDetail(expectedProjectId, mac);
      if (request === detailRequest && projectId.value === expectedProjectId) {
        detail.value = result;
      }
      return result;
    } catch (cause) {
      if (request === detailRequest) {
        error.value = commandErrorText(cause, "一体机详情读取失败");
        detail.value = undefined;
      }
    } finally {
      if (request === detailRequest) detailLoading.value = false;
    }
  }

  async function resumeImport() {
    const request = ++importRequest;
    const expectedProjectId = projectId.value;
    importLoading.value = true;
    try {
      const session = await useAioAdapter().getLatestImport(expectedProjectId);
      const preview = session ? { session, platformIssues: [] } : undefined;
      if (request === importRequest && projectId.value === expectedProjectId) {
        importPreview.value = preview;
      }
      return preview;
    } finally {
      if (request === importRequest) importLoading.value = false;
    }
  }

  async function previewImport(filePath: string) {
    const request = ++importRequest;
    const expectedProjectId = projectId.value;
    importLoading.value = true;
    try {
      const preview = await useAioAdapter().previewImport(expectedProjectId, filePath);
      if (request === importRequest && projectId.value === expectedProjectId) {
        importPreview.value = preview;
      }
      return preview;
    } catch (cause) {
      if (request === importRequest) {
        error.value = commandErrorText(cause, "导入预览失败");
      }
      throw cause;
    } finally {
      if (request === importRequest) importLoading.value = false;
    }
  }

  async function updateSelection(selection: ImportSelection) {
    const session = importPreview.value?.session;
    if (!session) return;
    const request = ++importRequest;
    const expectedProjectId = projectId.value;
    try {
      const updated = await useAioAdapter().updateImportSelection(
        expectedProjectId,
        session.id,
        [selection]
      );
      if (request === importRequest && projectId.value === expectedProjectId && importPreview.value) {
        importPreview.value = { ...importPreview.value, session: updated };
      }
    } catch (cause) {
      error.value = commandErrorText(cause, "更新导入选择失败");
      throw cause;
    }
  }

  async function applyImport(): Promise<InventoryApplyOutcome> {
    const session = importPreview.value?.session;
    if (!session) throw new Error("没有可应用的导入预览");
    const request = ++importRequest;
    const expectedProjectId = projectId.value;
    importLoading.value = true;
    try {
      const outcome = await useAioAdapter().applyImport(expectedProjectId, session.id);
      if (request === importRequest && projectId.value === expectedProjectId && importPreview.value) {
        importPreview.value = { ...importPreview.value, session: { ...session, state: "applied" } };
        await refresh(expectedProjectId);
      }
      return outcome;
    } catch (cause) {
      error.value = commandErrorText(cause, "应用导入失败");
      throw cause;
    } finally {
      if (request === importRequest) importLoading.value = false;
    }
  }

  async function discardImport() {
    const session = importPreview.value?.session;
    if (!session) return;
    const request = ++importRequest;
    const expectedProjectId = projectId.value;
    try {
      await useAioAdapter().discardImport(expectedProjectId, session.id);
      if (request === importRequest && projectId.value === expectedProjectId) {
        importPreview.value = undefined;
        latestImportSessionId.value = "";
      }
    } catch (cause) {
      error.value = commandErrorText(cause, "放弃导入失败");
      throw cause;
    }
  }

  return {
    realBackend, nodes, stats, total, page, pageSize, refreshedAt, latestImportSessionId,
    loading, error, detail, detailLoading, importPreview, importLoading, platformIssues,
    refresh, loadDetail, resumeImport, previewImport, updateSelection, applyImport, discardImport
  };
});
