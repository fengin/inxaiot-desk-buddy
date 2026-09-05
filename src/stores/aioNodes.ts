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
import { useActivityStore } from "@/stores/activity";

const emptyStats = (): AioNodeStats => ({ total: 0, online: 0, offline: 0, pending: 0, conflicts: 0 });
const SELECTION_PAGE_SIZE = 100;
const MAX_SELECTION_NODES = 10_000;

export const useAioNodesStore = defineStore("aio-nodes", () => {
  const preferences = usePreferencesStore();
  const activity = useActivityStore();
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
  const selectionNodes = ref<AioNodeListItem[]>([]);
  const selectionLoading = ref(false);
  const importLoading = ref(false);
  const serviceInspections = ref<Record<string, { projectId: string; mac: string; taskId?: string }>>({});
  let listSearch = "";
  let listState = "all";
  let listRequest = 0;
  let detailRequest = 0;
  let importRequest = 0;
  let selectionRequest = 0;

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
      selectionRequest += 1;
      selectionNodes.value = [];
    }
    projectId.value = nextProjectId;
    listSearch = search;
    listState = state;
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

  async function loadSelectionNodes(nextProjectId = projectId.value) {
    if (!nextProjectId) return [];
    if (projectId.value !== nextProjectId) {
      projectId.value = nextProjectId;
      selectionNodes.value = [];
    }
    const request = ++selectionRequest;
    selectionLoading.value = true;
    try {
      const adapter = useAioAdapter();
      const first = await adapter.listNodes(nextProjectId, {
        state: "all",
        page: 1,
        pageSize: SELECTION_PAGE_SIZE
      });
      if (first.total > MAX_SELECTION_NODES) {
        throw new Error(`项目一体机数量 ${first.total} 超过选择上限 ${MAX_SELECTION_NODES}`);
      }
      const all = [...first.items];
      const pageCount = Math.ceil(first.total / SELECTION_PAGE_SIZE);
      for (let nextPage = 2; nextPage <= pageCount; nextPage += 1) {
        const pageResult = await adapter.listNodes(nextProjectId, {
          state: "all",
          page: nextPage,
          pageSize: SELECTION_PAGE_SIZE
        });
        if (request !== selectionRequest || projectId.value !== nextProjectId) return [];
        if (pageResult.items.length === 0) {
          throw new Error("一体机选择集分页在读取完成前出现空页");
        }
        all.push(...pageResult.items);
      }
      const unique = [...new Map(all.map((node) => [node.macNormalized, node])).values()];
      if (unique.length !== first.total) {
        throw new Error(`一体机选择集数量不一致：期望 ${first.total}，实际 ${unique.length}`);
      }
      if (request === selectionRequest && projectId.value === nextProjectId) {
        selectionNodes.value = unique;
      }
      return unique;
    } catch (cause) {
      if (request === selectionRequest && projectId.value === nextProjectId) {
        selectionNodes.value = [];
        error.value = commandErrorText(cause, "读取完整一体机选择集失败");
      }
      throw cause;
    } finally {
      if (request === selectionRequest) selectionLoading.value = false;
    }
  }

  async function loadDetail(mac: string) {
    const request = ++detailRequest;
    const expectedProjectId = projectId.value;
    detailLoading.value = true;
    if (detail.value?.node.mac !== mac) detail.value = undefined;
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

  function inspectionKey(mac: string, expectedProjectId = projectId.value) {
    return `${expectedProjectId}:${mac.replace(/[:-]/g, "").toUpperCase()}`;
  }

  function serviceInspection(mac: string) {
    return serviceInspections.value[inspectionKey(mac)];
  }

  async function finishServiceInspection(key: string) {
    const inspection = serviceInspections.value[key];
    if (!inspection) return;
    delete serviceInspections.value[key];
    if (projectId.value !== inspection.projectId) return;
    const currentDetailMac = detail.value?.node.mac;
    const currentDetailRequest = detailRequest;
    await refresh(inspection.projectId, listSearch, listState);
    if (projectId.value === inspection.projectId && currentDetailMac
      && detailRequest === currentDetailRequest && detail.value?.node.mac === currentDetailMac) {
      await loadDetail(currentDetailMac);
    }
    if (selectionNodes.value.length && projectId.value === inspection.projectId) {
      await loadSelectionNodes(inspection.projectId).catch(() => undefined);
    }
  }

  watch(() => activity.tasks.map((task) => `${task.id}:${task.state}`).join("|"), () => {
    for (const [key, inspection] of Object.entries(serviceInspections.value)) {
      const task = activity.tasks.find((item) => item.id === inspection.taskId && item.projectId === inspection.projectId);
      if (task && ["succeeded", "partially_succeeded", "failed", "cancelled", "interrupted"].includes(task.state)) {
        void finishServiceInspection(key);
      }
    }
  });

  async function checkServices(mac: string) {
    const expectedProjectId = projectId.value;
    if (!expectedProjectId) throw new Error("请先选择项目");
    const key = inspectionKey(mac, expectedProjectId);
    if (serviceInspections.value[key]) return;
    serviceInspections.value[key] = { projectId: expectedProjectId, mac };
    try {
      await activity.start(expectedProjectId);
      const result = await useAioAdapter().checkServices(expectedProjectId, mac);
      serviceInspections.value[key] = { projectId: expectedProjectId, mac, taskId: result.taskId };
      if (projectId.value === expectedProjectId) {
        await activity.refreshTasks(expectedProjectId);
        await activity.selectTask(result.taskId);
        activity.openPanel("logs");
      }
      return result;
    } catch (cause) {
      delete serviceInspections.value[key];
      if (projectId.value === expectedProjectId) error.value = commandErrorText(cause, "提交服务检查失败");
      throw cause;
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
    selectionNodes, selectionLoading, serviceInspections, serviceInspection,
    refresh, loadSelectionNodes, loadDetail, checkServices, resumeImport, previewImport, updateSelection, applyImport, discardImport
  };
});
