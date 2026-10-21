import { computed, reactive, ref } from "vue";
import { defineStore } from "pinia";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { normalizeScreenMac, screenMergeCandidates } from "@/shared/model/screen";
import type { ScreenAction, ScreenSnapshot } from "@/shared/model/screen";
import { createScreenSpaceIndex } from "@/shared/model/screenSpace";
import { createScreenListProjection } from "@/shared/model/screenListProjection";

export const useSmartScreensStore = defineStore("smartScreens", () => {
  const projectId = ref("");
  const snapshot = ref<ScreenSnapshot>({ screens: [], spaces: [], tasks: [], ignoredPairs: [], platformAvailable: true });
  const loading = ref(false);
  const error = ref("");
  const selectedIds = ref<string[]>([]);
  const page = ref(1);
  const pageSize = ref(20);
  const operation = ref<ScreenAction>("install");
  const openedTaskId = ref("");
  const filters = reactive({ keyword: "", registration: "all", status: "all", size: "all", space: "", appVersion: "all" });
  let request = 0;
  let platformRefreshPending = false;
  let localRefreshQueued = false;
  let unsubscribe: (() => void) | undefined;

  const platformAvailable = computed(() => snapshot.value.platformAvailable);
  // 平台暂不可用时仍展示已注册屏缓存，注册关联不会退回本机未注册资产。
  const visibleScreens = computed(() => snapshot.value.screens);
  const spaceIndex = computed(() => createScreenSpaceIndex(snapshot.value.spaces));
  const candidates = computed(() => platformAvailable.value ? screenMergeCandidates(snapshot.value) : []);
  const candidateIds = computed(() => new Set(candidates.value.flatMap((c) => [c.local.id, c.platform.id])));
  // 与 canonical 屏记录分离，选择变化不重建资料投影或空间计数。
  const listProjections = computed(() => new Map(visibleScreens.value.map((screen) => [screen.id, createScreenListProjection(
    screen, snapshot.value.platformDrafts?.[screen.id], spaceIndex.value, snapshot.value.spacesAvailable !== false, platformAvailable.value
  )])));
  // 原始草稿及 revision 保留用于并发校验；提示仅表示相对最新平台值仍有差异。
  const draftIds = computed(() => new Set([...listProjections.value].filter(([, item]) => item.changedFields.size > 0).map(([id]) => id)));
  const listSpaceCounts = computed(() => spaceIndex.value.counts([...listProjections.value.values()].map((item) => item.spaceAssignment), snapshot.value.spacesAvailable !== false));
  const listSizeStats = computed(() => {
    const displayed = [...listProjections.value.values()];
    return { small: displayed.filter((item) => item.values.size === "4").length, large: displayed.filter((item) => item.values.size === "10").length };
  });
  const filtered = computed(() => {
    const keyword = filters.keyword.trim().toLocaleLowerCase();
    const macKeyword = normalizeScreenMac(keyword);
    return visibleScreens.value.filter((s) => {
      const displayed = listProjections.value.get(s.id)!;
      const searchMatch = !keyword || displayed.searchValues.some((value) => value.toLocaleLowerCase().includes(keyword))
        || (/^[0-9A-F]{4,}$/.test(macKeyword) && displayed.searchMacs.some((mac) => mac.includes(macKeyword)));
      return searchMatch && (filters.registration === "all" || (filters.registration === "duplicates" ? candidateIds.value.has(s.id) : s.source === filters.registration))
        && (filters.status === "all" || (s.source === "platform" && s.platformStatus === filters.status))
        && (filters.size === "all" || displayed.values.size === filters.size)
        && spaceIndex.value.matches(displayed.spaceAssignment, filters.space, snapshot.value.spacesAvailable !== false)
        && (filters.appVersion === "all" || (filters.appVersion === "unknown" ? !s.appVersion : s.appVersion === filters.appVersion));
    });
  });
  const paged = computed(() => filtered.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
  const stats = computed(() => {
    const all = visibleScreens.value;
    const platform = all.filter((s) => s.source === "platform");
    return { total: all.length, platform: platform.length, local: all.filter((s) => s.source === "local").length,
      online: platform.filter((s) => s.platformStatus === "online").length, offline: platform.filter((s) => s.platformStatus === "offline").length,
      unknown: platform.filter((s) => s.platformStatus === "unknown").length,
      duplicates: candidates.value.length, small: all.filter((s) => s.size === "4").length, large: all.filter((s) => s.size === "10").length };
  });
  const statusDifferences = computed(() => platformAvailable.value ? snapshot.value.screens.filter((s) => s.source === "platform" && s.ping !== null && s.ping !== s.platformStatus) : []);
  const selectedScreens = computed(() => visibleScreens.value.filter((s) => selectedIds.value.includes(s.id)));

  function resetFilters() { Object.assign(filters, { keyword: "", registration: "all", status: "all", size: "all", space: "", appVersion: "all" }); page.value = 1; selectedIds.value = []; }
  function filterChanged() { page.value = 1; selectedIds.value = []; }
  function toggleSelections(ids: readonly string[], checked: boolean) {
    if (!ids.length) return;
    const selected = new Set(selectedIds.value);
    for (const id of ids) { if (checked) selected.add(id); else selected.delete(id); }
    const next = [...selected];
    if (next.length !== selectedIds.value.length || next.some((id, index) => id !== selectedIds.value[index])) selectedIds.value = next;
  }
  function toggleSelection(id: string, checked: boolean) { toggleSelections([id], checked); }
  function selectPage(checked: boolean) { toggleSelections(paged.value.map(screen => screen.id), checked); }
  function selectAllMatched() { selectedIds.value = filtered.value.map((s) => s.id); }
  async function refresh(options?: { refreshPlatform?: boolean }) {
    if (!projectId.value) return;
    if (options?.refreshPlatform) platformRefreshPending = true;
    const revision = ++request;
    const expectedProject = projectId.value;
    loading.value = true;
    try {
      // 显式平台刷新尚未完成时，其他直接刷新请求也读取平台，避免本机缓存覆盖新状态。
      const next = await useScreenAdapter().load(expectedProject, { refreshPlatform: platformRefreshPending });
      if (revision !== request || projectId.value !== expectedProject) return;
      snapshot.value = next; error.value = "";
      selectedIds.value = [...new Set(selectedIds.value.flatMap((id) => {
        const screen = visibleScreens.value.find((s) => s.id === id || s.aliases.includes(id));
        return screen ? [screen.id] : [];
      }))];
      page.value = Math.min(page.value, Math.max(1, Math.ceil(filtered.value.length / pageSize.value)));
    } catch (cause) {
      if (revision === request) error.value = cause instanceof Error ? cause.message : "读取屏记录失败";
    } finally {
      if (revision === request) {
        loading.value = false; platformRefreshPending = false;
        if (localRefreshQueued) { localRefreshQueued = false; void refresh(); }
      }
    }
  }
  async function bindProject(id: string) {
    unsubscribe?.(); ++request; projectId.value = id; resetFilters(); openedTaskId.value = "";
    platformRefreshPending = false; localRefreshQueued = false;
    snapshot.value = { screens: [], spaces: [], tasks: [], ignoredPairs: [], platformAvailable: true };
    unsubscribe = useScreenAdapter().subscribe((changedProject) => {
      if (projectId.value !== changedProject) return;
      // 合并平台读取期间的任务通知，避免慢平台请求被每两秒的任务刷新不断替代。
      if (platformRefreshPending) localRefreshQueued = true;
      else void refresh();
    });
    await refresh({ refreshPlatform: true });
  }
  function stop() { unsubscribe?.(); unsubscribe = undefined; ++request; platformRefreshPending = false; localRefreshQueued = false; }

  return { projectId, snapshot, loading, error, selectedIds, selectedScreens, page, pageSize, operation, openedTaskId, filters,
    platformAvailable, visibleScreens, spaceIndex, candidates, candidateIds, draftIds, listProjections, listSpaceCounts, listSizeStats, filtered, paged, stats, statusDifferences,
    resetFilters, filterChanged, toggleSelection, toggleSelections, selectPage, selectAllMatched, refresh, bindProject, stop };
});
