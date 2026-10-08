<script setup lang="ts">
import {
  NAlert,
  NButton,
  NCheckbox,
  NDrawer,
  NDrawerContent,
  NInput,
  NModal,
  NPagination,
  NPopover,
  NSelect,
  NSpace,
  NSpin,
  NTag,
  useMessage
} from "naive-ui";
import {
  Box,
  Boxes,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  CircleHelp,
  Download,
  FileSpreadsheet,
  GitCompareArrows,
  Plus,
  RefreshCw,
  Search,
  ServerCog,
  Upload,
  Wifi,
  WifiOff,
  XCircle
} from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { commandErrorText } from "@/shared/api/errors";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { nodeServicePresentation, nodeServiceRows, projectVersionRows, serviceCheckCoverage, serviceCheckSourceLabel, serviceInspectionStageLabel, serviceRuntimeLabel } from "./serviceCheckPresentation";

import type {
  AioNodeListItem,
  ImportClassification,
  ReconciledImportItem
} from "@/shared/model/aio";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";
import { useActivityStore } from "@/stores/activity";
import AioNodeCreateDialog from "./AioNodeCreateDialog.vue";
import AioNodeEditDialog from "./AioNodeEditDialog.vue";
import AssetListFooter from "@/shared/components/AssetListFooter.vue";
import { useAioAdapter } from "@/shared/api/aioAdapter";
import { uniqueProjectSpacePaths, projectSpacePathsCsv } from "@/shared/model/projectSpace";

const aio = useAioNodesStore();
const projects = useProjectStore();
const activity = useActivityStore();
const message = useMessage();
const router = useRouter();
const dialogs = useSystemDialogAdapter();
const search = ref("");
const stateFilter = ref("all");
const viewMode = ref<"nodes" | "platform_issues">("nodes");
const issuePage = ref(1);
const selectedNode = ref<AioNodeListItem>();
const versionOpen = ref(false);
const importOpen = ref(false);
const createOpen = ref(false);
const editOpen = ref(false);
const selectedMacs = ref<string[]>([]);
const importStage = ref<"select" | "preview" | "done">("select");
let refreshTimer: number | undefined;

const filteredNodes = computed(() => aio.nodes);
const stats = computed(() => aio.stats);
const platformIssueCount = computed(() => aio.platformIssues.length);
const showingPlatformIssues = computed(() => viewMode.value === "platform_issues");
function canSelectNode(node: AioNodeListItem) { return /^[0-9a-f]{12}$/i.test(node.macNormalized) && node.managementState !== "conflict" && !node.conflicts.length; }
const selectablePage = computed(() => showingPlatformIssues.value ? [] : filteredNodes.value.filter(canSelectNode));
const allPageSelected = computed(() => selectablePage.value.length > 0 && selectablePage.value.every(node => selectedMacs.value.includes(node.macNormalized)));
const somePageSelected = computed(() => !allPageSelected.value && selectablePage.value.some(node => selectedMacs.value.includes(node.macNormalized)));
function selectNode(node: AioNodeListItem, checked: boolean) {
  if (!canSelectNode(node)) return;
  selectedMacs.value = checked ? [...new Set([...selectedMacs.value, node.macNormalized])] : selectedMacs.value.filter(mac => mac !== node.macNormalized);
}
function selectPage(checked: boolean) {
  const pageMacs = new Set(selectablePage.value.map(node => node.macNormalized));
  selectedMacs.value = checked ? [...new Set([...selectedMacs.value, ...pageMacs])] : selectedMacs.value.filter(mac => !pageMacs.has(mac));
}
async function openBatchOperations() {
  if (!selectedMacs.value.length || !projects.isReady || aio.loading) return;
  await router.push({ name: "aio-operations", query: { targets: [...selectedMacs.value], project: projects.activeProjectId } });
}
async function createdNode() {
  viewMode.value = "nodes"; search.value = ""; stateFilter.value = "all"; aio.page = 1;
  await refresh();
}
async function editedNode() {
  const projectId = projects.activeProjectId, mac = selectedNode.value?.mac;
  try {
    await refresh();
    if (mac && projects.activeProjectId === projectId) await aio.loadDetail(mac);
  } catch { message.warning("资料已保存，但列表刷新失败，请手动刷新核对"); }
}
const filteredPlatformIssues = computed(() => {
  const keyword = search.value.trim().toLocaleLowerCase();
  if (!keyword) return aio.platformIssues;
  return aio.platformIssues.filter((issue) =>
    [issue.name, issue.ip, issue.platformAioId, issue.rawMac]
      .some((value) => value.toLocaleLowerCase().includes(keyword))
  );
});
const pagedPlatformIssues = computed(() => {
  const start = (issuePage.value - 1) * aio.pageSize;
  return filteredPlatformIssues.value.slice(start, start + aio.pageSize);
});
const importSession = computed(() => aio.importPreview?.session);
const versionNodes = computed(() => aio.selectionNodes.length ? aio.selectionNodes : aio.nodes);

const versionRows = computed(() => projectVersionRows(versionNodes.value));
const detailServices = computed(() => aio.detail ? nodeServiceRows(aio.detail.node, aio.detail.versions) : []);
const detailInspection = computed(() => selectedNode.value ? aio.serviceInspection(selectedNode.value.mac) : undefined);
const detailInspectionStage = computed(() => {
  const taskId = detailInspection.value?.taskId;
  const task = activity.tasks.find((item) => item.id === taskId);
  return serviceInspectionStageLabel(activity.lastEvent?.localTaskId === taskId ? activity.lastEvent?.stage : task?.stage);
});

function managementTone(node: AioNodeListItem) {
  if (node.managementState === "managed") return "success";
  if (node.managementState === "conflict") return "error";
  if (node.managementState === "platform_existing") return "info";
  return "warning";
}

function classificationLabel(value: ImportClassification) {
  return {
    new_pending: "新增待实施",
    existing_unchanged: "已存在无变化",
    existing_changed: "已有字段变化",
    platform_existing: "平台已存在",
    conflict: "信息冲突",
    invalid: "格式错误"
  }[value];
}

function classificationTone(value: ImportClassification) {
  if (value === "new_pending") return "success";
  if (value === "platform_existing") return "info";
  if (value === "existing_changed" || value === "conflict") return "warning";
  if (value === "invalid") return "error";
  return "default";
}

function classificationAction(item: ReconciledImportItem) {
  if (item.classification === "new_pending") return "写入待实施资产";
  if (item.classification === "platform_existing") return "接管平台对象";
  if (item.classification === "existing_changed") return "更新最终资产";
  if (item.classification === "existing_unchanged") return "无需写入";
  return item.conflicts[0]?.message ?? item.errors[0] ?? "需要处理";
}

function lastOperationLabel(node: AioNodeListItem) {
  const completedAt = formatDisplayDateTime(node.lastOperationAt, "");
  const operation = node.lastOperation === "导入一体机清单" ? "导入" : node.lastOperation;
  return completedAt ? `${completedAt} · ${operation}` : operation;
}

function sourceLabel(value: string) {
  return {
    local: "本机待实施",
    deployment: "部署结果",
    import: "导入",
    merged: "平台接管",
    platform: "平台已有"
  }[value] ?? "未知来源";
}

async function refresh() {
  if (!projects.activeProjectId || !projects.allowsAccess("platform")) return;
  await aio.refresh(projects.activeProjectId, search.value, stateFilter.value);
}

function selectNodeFilter(value: string) {
  const wasShowingPlatformIssues = showingPlatformIssues.value;
  const stateChanged = stateFilter.value !== value;
  viewMode.value = "nodes";
  stateFilter.value = value;
  aio.page = 1;
  if (wasShowingPlatformIssues && !stateChanged) void refresh();
}

function selectPlatformIssues() {
  viewMode.value = "platform_issues";
  issuePage.value = 1;
}

function selectListFilter(value: string) {
  if (value === "platform_issues") {
    selectPlatformIssues();
    return;
  }
  selectNodeFilter(value);
}

function scheduleRefresh() {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => void refresh(), 180);
}

async function openNode(node: AioNodeListItem) {
  selectedNode.value = node;
  await aio.loadDetail(node.mac);
}

async function checkServices() {
  const node = selectedNode.value;
  if (!node || detailInspection.value) return;
  try {
    await aio.checkServices(node.mac);
  } catch (cause) {
    message.error(commandErrorText(cause, "提交服务检查失败"));
  }
}

async function openVersions() {
  if (!projects.activeProjectId) return;
  try {
    await aio.loadSelectionNodes(projects.activeProjectId);
    versionOpen.value = true;
  } catch (cause) {
    message.error(commandErrorText(cause, "读取项目版本分布失败"));
  }
}

async function openDeployment() {
  const mac = aio.detail?.node.mac ?? selectedNode.value?.mac;
  if (!mac) return;
  selectedNode.value = undefined;
  await router.push({ name: "aio-operations", query: { target: mac } });
}

async function openImport() {
  importOpen.value = true;
  importStage.value = "select";
  const existing = await aio.resumeImport();
  if (existing) importStage.value = "preview";
}

async function chooseInventoryFile() {
  try {
    let path = "C:/demo/inventory-2026-08.csv";
    if (aio.realBackend) {
      const selected = await dialogs.selectFile("选择一体机清单", [
        { name: "一体机清单", extensions: ["csv"] }
      ]);
      if (!selected) return;
      path = selected;
    }
    await aio.previewImport(path);
    importStage.value = "preview";
  } catch {
    if (aio.error) message.error(aio.error);
  }
}

function downloadCsv(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "text/csv;charset=utf-8" }));
  const link = document.createElement("a"); link.href = url; link.download = name; link.click();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}
function downloadInventoryTemplate() {
  downloadCsv("一体机导入模板.csv", "\uFEFF名称,IP,MAC,空间路径,位置,备注\r\n");
}
async function downloadSpacePaths() {
  const projectId = projects.activeProjectId;
  try {
    const spaces = await useAioAdapter().listSpaces(projectId);
    if (projects.activeProjectId !== projectId) return;
    downloadCsv("一体机-项目空间路径清单.csv", projectSpacePathsCsv(uniqueProjectSpacePaths(spaces)));
  } catch (cause) { message.error(commandErrorText(cause, "空间目录读取失败")); }
}

async function toggleImportItem(item: ReconciledImportItem, selected: boolean) {
  try {
    await aio.updateSelection({ rowNumber: item.rowNumber, selected });
  } catch {
    if (aio.error) message.error(aio.error);
  }
}

async function applyImport() {
  try {
    const outcome = await aio.applyImport();
    importStage.value = "done";
    const suffix = outcome.localSessionFinalized
      ? ""
      : "；最终资产已提交，但本地预览封存失败，重新打开前请刷新";
    message.success("已将 " + outcome.result.appliedCount + " 台一体机写入项目最终资产" + suffix);
  } catch {
    if (aio.error) message.error(aio.error);
  }
}

async function discardImport() {
  try {
    await aio.discardImport();
    importOpen.value = false;
    importStage.value = "select";
  } catch {
    if (aio.error) message.error(aio.error);
  }
}

function closeImport() {
  importOpen.value = false;
  window.setTimeout(() => {
    importStage.value = "select";
  }, 180);
}

watch([search, stateFilter], () => {
  if (showingPlatformIssues.value) {
    issuePage.value = 1;
    return;
  }
  aio.page = 1;
  scheduleRefresh();
});
watch(() => projects.activeProject?.connectionState, () => { void refresh(); });
watch(
  () => aio.page,
  () => {
    if (!showingPlatformIssues.value) void refresh();
  }
);
watch(
  () => projects.activeProjectId,
  () => {
    viewMode.value = "nodes";
    selectedMacs.value = []; createOpen.value = false; editOpen.value = false;
    selectedNode.value = undefined;
    versionOpen.value = false;
    aio.page = 1;
    void refresh();
  }
);
watch(() => aio.nodes, (nodes) => {
  const blocked = new Set(nodes.filter(node => !canSelectNode(node)).map(node => node.macNormalized));
  selectedMacs.value = selectedMacs.value.filter(mac => !blocked.has(mac));
});
onMounted(async () => {
  await projects.initialize();
  await refresh();
});
onBeforeUnmount(() => {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
});
</script>

<template>
  <section class="workspace-page node-list-page" data-testid="aio-node-list">
    <header class="page-header">
      <div><h1>一体机列表</h1></div>
      <div class="page-actions">
        <n-button size="small" secondary :loading="aio.selectionLoading" @click="openVersions">
          <template #icon><GitCompareArrows /></template>
          查看镜像和版本
        </n-button>
        <n-button size="small" type="primary" @click="openImport">
          <template #icon><Upload /></template>
          导入清单
        </n-button>
        <n-button size="small" type="primary" :disabled="!projects.allowsAccess('platform')" data-testid="aio-create-node" @click="createOpen = true">
          <template #icon><Plus /></template>新增一体机
        </n-button>
      </div>
    </header>

    <n-alert v-if="aio.error" type="error" :bordered="false" closable @close="aio.error = ''">
      {{ aio.error }}
    </n-alert>
    <n-alert v-else-if="aio.metadataWarning" type="warning" :bordered="false">{{ aio.metadataWarning }}</n-alert>
    <n-alert
      v-else-if="aio.latestImportSessionId"
      type="info"
      :bordered="false"
      class="pending-import-alert"
    >
      当前项目有未处理的导入预览。
      <n-button text type="primary" size="tiny" @click="openImport">继续处理</n-button>
    </n-alert>

    <div class="summary-strip">
      <button class="summary-item" :class="{ active: !showingPlatformIssues && stateFilter === 'all' }" type="button" @click="selectNodeFilter('all')">
        <span class="summary-label summary-label--all"><Boxes :size="14" />全部一体机</span>
        <span class="summary-value"><strong>{{ stats.total }}</strong><small>当前项目资产视图</small></span>
      </button>
      <button class="summary-item" :class="{ active: !showingPlatformIssues && stateFilter === 'online' }" type="button" @click="selectNodeFilter('online')">
        <span class="summary-label summary-label--online"><Wifi :size="14" />平台在线</span>
        <span class="summary-value"><strong>{{ stats.online }}</strong><small>以平台状态时间为准</small></span>
      </button>
      <button class="summary-item" :class="{ active: !showingPlatformIssues && stateFilter === 'offline' }" type="button" @click="selectNodeFilter('offline')">
        <span class="summary-label summary-label--offline"><WifiOff :size="14" />平台离线</span>
        <span class="summary-value"><strong>{{ stats.offline }}</strong><small>执行前仍会检查SSH</small></span>
      </button>
      <button class="summary-item" :class="{ active: !showingPlatformIssues && stateFilter === 'pending' }" type="button" @click="selectNodeFilter('pending')">
        <span class="summary-label summary-label--pending"><Box :size="14" />待实施</span>
        <span class="summary-value"><strong>{{ stats.pending }}</strong><small>已确认但尚未部署</small></span>
      </button>
      <button class="summary-item warning" :class="{ active: !showingPlatformIssues && stateFilter === 'conflict' }" type="button" @click="selectNodeFilter('conflict')">
        <span class="summary-label summary-label--conflict"><CircleAlert :size="14" />信息冲突</span>
        <span class="summary-value"><strong>{{ stats.conflicts }}</strong><small>处理后才能执行</small></span>
      </button>
      <n-popover v-if="platformIssueCount" trigger="hover" placement="bottom-end" :width="420">
        <template #trigger>
          <button
            class="summary-item warning platform-issue-summary"
            :class="{ active: showingPlatformIssues }"
            type="button"
            data-testid="platform-issue-summary"
            @click="selectPlatformIssues"
          >
            <span class="summary-label summary-label--conflict"><CircleAlert :size="14" />平台待处理</span>
            <span class="summary-value"><strong>{{ platformIssueCount }}</strong><small>MAC 信息待补充</small></span>
          </button>
        </template>
        <section class="platform-issue-popover" data-testid="platform-issue-popover">
          <strong>请在平台补充正确的 MAC 地址</strong>
          <p>以下记录暂不参与资产匹配；修正后刷新列表即可自动纳入。</p>
          <ul>
            <li v-for="issue in aio.platformIssues" :key="issue.platformAioId">
              <strong>{{ issue.name || '未命名一体机' }}</strong>
              <span>IP：{{ issue.ip || '未填写' }} · 平台编号：{{ issue.platformAioId }}</span>
              <span>当前 MAC：{{ issue.rawMac || '未填写' }}（{{ issue.message }}）</span>
            </li>
          </ul>
        </section>
      </n-popover>
      <button
        v-else
        class="summary-item platform-issue-summary"
        :class="{ active: showingPlatformIssues }"
        type="button"
        data-testid="platform-issue-summary"
        @click="selectPlatformIssues"
      >
        <span class="summary-label summary-label--conflict"><CircleAlert :size="14" />平台待处理</span>
        <span class="summary-value"><strong>0</strong><small>暂无待处理记录</small></span>
      </button>
    </div>

    <section class="data-panel">
      <header class="data-toolbar">
        <div class="toolbar-spacer"></div>
        <n-input v-model:value="search" size="small" clearable placeholder="搜索名称、IP、MAC或位置" class="search-input">
          <template #prefix><Search :size="15" /></template>
        </n-input>
        <n-select
          :value="showingPlatformIssues ? 'platform_issues' : stateFilter"
          size="small"
          class="state-select"
          @update:value="selectListFilter"
          :options="[
            { label: '全部状态', value: 'all' },
            { label: '已管理', value: 'managed' },
            { label: '待实施', value: 'pending' },
            { label: '平台已存在', value: 'platform_existing' },
            { label: '信息冲突', value: 'conflict' },
            { label: '平台待处理', value: 'platform_issues' }
          ]"
        />
        <n-button size="small" quaternary title="刷新列表" :loading="aio.loading" @click="refresh">
          <template #icon><RefreshCw /></template>
        </n-button>
      </header>

      <div class="node-table-header">
        <table class="workbench-table node-table">
          <colgroup>
            <col v-if="!showingPlatformIssues" class="node-col-select" />
            <col class="node-col-name" /><col class="node-col-ip" /><col class="node-col-mac" />
            <col class="node-col-location" /><col class="node-col-deploy" /><col class="node-col-platform" />
            <col class="node-col-service" /><col class="node-col-action" />
          </colgroup>
          <thead><tr><th v-if="!showingPlatformIssues" class="node-select-cell"><n-checkbox aria-label="选择当前页一体机" :checked="allPageSelected" :indeterminate="somePageSelected" :disabled="aio.loading || !selectablePage.length" @update:checked="selectPage" /></th><th>名称</th><th>IP</th><th>MAC地址</th><th>位置</th><th>部署状态</th><th>平台状态</th><th>最近服务检查</th><th></th></tr></thead>
        </table>
      </div>
      <div class="table-scroll">
        <n-spin :show="aio.loading">
          <table class="workbench-table node-table">
            <colgroup>
              <col v-if="!showingPlatformIssues" class="node-col-select" />
              <col class="node-col-name" /><col class="node-col-ip" /><col class="node-col-mac" />
              <col class="node-col-location" /><col class="node-col-deploy" /><col class="node-col-platform" />
              <col class="node-col-service" /><col class="node-col-action" />
            </colgroup>
            <tbody>
              <template v-if="showingPlatformIssues">
                <tr v-for="issue in pagedPlatformIssues" :key="issue.platformAioId" class="platform-issue-row">
                  <td><strong>{{ issue.name || '未命名一体机' }}</strong><small>平台编号：{{ issue.platformAioId }}</small></td>
                  <td class="mono">{{ issue.ip || '—' }}</td>
                  <td class="mono muted-cell">{{ issue.rawMac || '—' }}</td>
                  <td>—</td>
                  <td><n-tag size="small" :bordered="false" type="warning">待处理</n-tag></td>
                  <td><n-tag size="small" :bordered="false" type="warning">MAC 需修正</n-tag></td>
                  <td><n-tag size="small" :bordered="false">未纳入资产</n-tag></td>
                  <td>—</td>
                </tr>
              </template>
              <template v-else>
                <tr
                  v-for="node in filteredNodes"
                  :key="node.macNormalized"
                  :data-node-mac="node.macNormalized"
                  :class="{ 'node-selected': selectedMacs.includes(node.macNormalized) }"
                  tabindex="0"
                  @click="openNode(node)"
                  @keydown.enter="openNode(node)"
                >
                  <td class="node-select-cell" @click.stop @keydown.stop><n-checkbox :aria-label="`选择${node.name}`" :checked="selectedMacs.includes(node.macNormalized)" :disabled="aio.loading || !canSelectNode(node)" :title="!canSelectNode(node) ? '请先处理资料冲突或补齐 MAC' : `选择${node.name}`" @update:checked="selectNode(node, $event)" /></td>
                  <td><strong>{{ node.name }}</strong><small>{{ lastOperationLabel(node) }}</small></td>
                  <td class="mono">{{ node.ip }}</td>
                  <td class="mono muted-cell">{{ node.mac }}</td>
                  <td class="node-space-cell" :title="[node.spacePath, node.location].filter(Boolean).join(' · ')"><span>{{ node.spacePath || (node.buildingId ? '原空间待核实' : '未选择空间') }}</span><small v-if="node.location">{{ node.location }}</small></td>
                  <td><n-tag size="small" :bordered="false" :type="managementTone(node)">{{ node.deployLabel }}</n-tag></td>
                  <td>
                    <span class="state-with-time" :class="node.platformState">
                      <i></i>
                      <span>
                        {{ node.platformState === "online" ? "在线" : node.platformState === "offline" ? "离线" : "未知" }}
                        <small>{{ formatDisplayDateTime(node.platformUpdatedAt) }}</small>
                      </span>
                    </span>
                  </td>
                  <td class="service-check-cell"><n-tag size="small" :bordered="false" :type="nodeServicePresentation(node).tone">{{ nodeServicePresentation(node).label }}</n-tag><small>{{ node.serviceCheck?.lastFullCheckAt ? formatDisplayDateTime(node.serviceCheck.lastFullCheckAt) : '整机未检查' }}</small></td>
                  <td><ChevronRight :size="16" class="row-chevron" /></td>
                </tr>
              </template>
            </tbody>
          </table>
          <div v-if="!aio.loading && !(showingPlatformIssues ? filteredPlatformIssues.length : filteredNodes.length)" class="empty-inline">
            <XCircle :size="34" />
            <strong>{{ showingPlatformIssues ? '没有匹配的待处理记录' : '没有匹配的一体机' }}</strong>
            <span>{{ showingPlatformIssues ? '调整搜索条件或刷新列表后再试。' : '调整搜索条件、状态筛选或项目连接后再试。' }}</span>
          </div>
        </n-spin>
      </div>
      <asset-list-footer :selected-count="showingPlatformIssues ? undefined : selectedMacs.length" :disabled="aio.loading || !projects.isReady" :disabled-reason="aio.loading ? '列表正在刷新，请稍候' : '请先连接并登录当前项目'" batch-test-id="aio-batch-operations" @batch="openBatchOperations">
        <template #summary>{{ showingPlatformIssues ? `每页 ${aio.pageSize} 条 · 共 ${filteredPlatformIssues.length} 条平台待处理记录` : `每页 ${aio.pageSize} 条 · 共 ${aio.total} 条` }}</template>
        <template #pagination>
          <n-pagination v-if="showingPlatformIssues && filteredPlatformIssues.length > aio.pageSize" v-model:page="issuePage" :page-size="aio.pageSize" :item-count="filteredPlatformIssues.length" :page-slot="5" size="small" />
          <n-pagination v-else-if="!showingPlatformIssues && aio.total > aio.pageSize" v-model:page="aio.page" :page-size="aio.pageSize" :item-count="aio.total" :page-slot="5" size="small" />
        </template>
        <template #meta>数据更新时间：{{ formatDisplayDateTime(aio.refreshedAt) }}</template>
      </asset-list-footer>
    </section>

    <aio-node-create-dialog v-model:show="createOpen" :project-id="projects.activeProjectId" @created="createdNode" />
    <aio-node-edit-dialog v-model:show="editOpen" :project-id="projects.activeProjectId" :detail="aio.detail" @saved="editedNode" />

    <n-drawer :show="Boolean(selectedNode)" width="min(var(--inx-detail-drawer-width), 94vw)" class="device-detail-drawer aio-detail-drawer" placement="right" @update:show="!$event && (selectedNode = undefined)">
      <n-drawer-content title="一体机详情" closable>
        <n-spin :show="aio.detailLoading">
          <template v-if="aio.detail">
            <n-alert v-if="aio.detail.metadataWarning" type="warning" :bordered="false">{{ aio.detail.metadataWarning }}</n-alert>
            <div class="drawer-identity">
              <span class="feature-icon info"><ServerCog :size="22" /></span>
              <div><strong :title="aio.detail.node.name">{{ aio.detail.node.name }}</strong><span class="mono" :title="`${aio.detail.node.ip} · ${aio.detail.node.mac}`">{{ aio.detail.node.ip }} · {{ aio.detail.node.mac }}</span></div>
            </div>
            <div>
              <h3 class="device-detail-section-title">资产关系</h3>
              <dl class="device-detail-facts">
                <div><dt>工作台状态</dt><dd :title="aio.detail.node.deployLabel">{{ aio.detail.node.deployLabel }}</dd></div>
                <div><dt>平台对象ID</dt><dd class="mono" :title="String(aio.detail.node.platformId ?? '尚未关联')">{{ aio.detail.node.platformId ?? "尚未关联" }}</dd></div>
                <div><dt>资产来源</dt><dd :title="sourceLabel(aio.detail.node.source)">{{ sourceLabel(aio.detail.node.source) }}</dd></div>
                <div><dt>资料保存</dt><dd>{{ aio.detail.platform ? '平台业务库' : '当前电脑' }}</dd></div>
                <div><dt>空间位置</dt><dd :title="aio.detail.node.spacePath">{{ aio.detail.node.spacePath || (aio.detail.node.buildingId ? '原空间待核实' : '未选择') }}</dd></div>
                <div><dt>具体位置</dt><dd :title="aio.detail.node.location">{{ aio.detail.node.location || '未填写' }}</dd></div>
                <div><dt>最近操作</dt><dd :title="lastOperationLabel(aio.detail.node)">{{ lastOperationLabel(aio.detail.node) }}</dd></div>
              </dl>
            </div>
            <div>
              <div class="service-section-heading"><h3 class="device-detail-section-title">服务与版本检查</h3><n-button size="tiny" secondary :loading="Boolean(detailInspection)" :disabled="!projects.isReady || !aio.detail.node.ip || Boolean(detailInspection)" data-testid="check-node-services" @click="checkServices">{{ detailInspection ? '检查中' : '检查服务' }}</n-button></div>
              <p class="service-check-meta">已部署版本为项目共享记录，服务检查仅保存到本机。</p>
              <div class="service-check-summary"><p class="service-check-meta">本机最近整机检查：{{ formatDisplayDateTime(aio.detail.node.serviceCheck?.lastFullCheckAt, '尚未检查') }}</p><p v-if="detailServices.length" class="service-check-meta">{{ serviceCheckCoverage(aio.detail.node).checked }}/{{ serviceCheckCoverage(aio.detail.node).total }} 项服务有本机实测结果</p></div>
              <p v-if="detailInspection" class="service-check-meta" data-testid="service-inspection-stage">{{ detailInspectionStage }} · 可在底部查看任务日志</p>
              <p v-if="aio.detail.node.serviceCheck?.lastAttempt?.scope === 'service'" class="service-check-meta">最近仅检查 {{ aio.detail.node.serviceCheck.lastAttempt.serviceName }}，其他服务保留各自检查时间。</p>
              <div v-if="aio.detail.node.serviceCheck?.lastAttempt?.state === 'failed'" class="inline-notice warning service-check-failure" data-testid="service-check-failure"><CircleAlert :size="16" /><div><strong>最近一次检查失败 · {{ formatDisplayDateTime(aio.detail.node.serviceCheck.lastAttempt.checkedAt) }}</strong><span>{{ aio.detail.node.serviceCheck.lastAttempt.error || '未能完成服务检查，请查看任务日志。' }}</span></div></div>
              <div v-if="detailServices.length" class="service-observation-list">
                <div v-for="service in detailServices" :key="service.serviceName" class="service-observation" data-testid="node-service-observation">
                  <header>
                    <strong :title="service.serviceName">{{ service.serviceName }}</strong>
                    <span class="service-observation-status" :class="service.tone">
                      <component :is="service.tone === 'success' ? CheckCircle2 : service.tone === 'error' ? XCircle : service.tone === 'warning' ? CircleAlert : CircleHelp" :size="13" aria-hidden="true" />
                      {{ service.label }}
                    </span>
                  </header>
                  <dl class="detail-grid">
                    <dt>镜像版本</dt><dd class="mono" :title="service.observation?.actualImage ? '最近检查的实际镜像' : '已部署记录，实际镜像尚未采集'"><span :title="service.observation?.actualImage || service.deployedImage">{{ service.observation?.actualImage || service.deployedImage }}</span></dd>
                    <template v-if="service.observation?.actualImage && service.deployedImage !== '未记录' && service.observation.actualImage !== service.deployedImage"><dt>已部署版本</dt><dd class="mono" :title="service.deployedImage">{{ service.deployedImage }}</dd></template>
                    <template v-if="service.observation?.expectedImage && service.observation.expectedImage !== (service.observation.actualImage || service.deployedImage) && service.observation.expectedImage !== service.deployedImage"><dt>生效配置镜像</dt><dd class="mono" :title="service.observation.expectedImage">{{ service.observation.expectedImage }}</dd></template>
                    <dt>运行状态</dt><dd :title="serviceRuntimeLabel(service.observation)">{{ serviceRuntimeLabel(service.observation) }}</dd>
                    <dt>检查时间</dt><dd :title="formatDisplayDateTime(service.observation?.checkedAt, '尚未检查')">{{ formatDisplayDateTime(service.observation?.checkedAt, '尚未检查') }}</dd>
                    <dt>检查来源</dt><dd :title="serviceCheckSourceLabel(service.observation?.source)">{{ serviceCheckSourceLabel(service.observation?.source) }}</dd>
                    <template v-if="service.observation?.message"><dt>说明</dt><dd class="service-observation-message">{{ service.observation.message }}</dd></template>
                  </dl>
                </div>
              </div>
              <div v-else class="empty-compact">无检查记录，点击“检查服务”读取一体机状态。</div>
            </div>
            <div v-if="aio.detail.node.conflicts.length" class="inline-notice warning">
              <CircleAlert :size="17" />
              <div>
                <strong>存在 {{ aio.detail.node.conflicts.length }} 项资产冲突</strong>
                <span>{{ aio.detail.node.conflicts.map((conflict) => conflict.message).join("；") }}</span>
              </div>
            </div>
          </template>
        </n-spin>
        <template #footer>
          <n-space justify="end">
            <n-button size="small" @click="selectedNode = undefined">关闭</n-button>
            <n-button size="small" :disabled="aio.detailLoading || !aio.detail" data-testid="aio-edit-node" @click="editOpen = true">编辑资料</n-button>
            <n-button size="small" type="primary" :disabled="Boolean(aio.detail?.node.conflicts.length)" @click="openDeployment">进入部署升级</n-button>
          </n-space>
        </template>
      </n-drawer-content>
    </n-drawer>

    <n-modal v-model:show="versionOpen" preset="card" title="镜像和版本" class="version-modal" :bordered="false">
      <div class="version-summary">
        <span><strong>{{ versionRows.length }}</strong> 个服务</span>
        <span><strong>{{ versionNodes.filter((node) => node.versions.some((version) => (node.serviceCheck?.expectedServices === undefined || node.serviceCheck.expectedServices.includes(version.serviceName)) && (version.expectedImageName || version.expectedVersion))).length }}</strong> 台有部署记录</span>
        <span><strong>{{ versionNodes.filter((node) => serviceCheckCoverage(node).checked > 0).length }}</strong> 台有本机实测记录</span>
      </div>
      <table class="workbench-table compact">
        <thead><tr><th>服务</th><th>已部署版本分布</th><th>部署记录</th><th>本机实测记录</th></tr></thead>
        <tbody>
          <tr v-for="row in versionRows" :key="row.service">
            <td><strong>{{ row.service }}</strong></td>
            <td><span v-for="([version, count]) in row.versions" :key="version" class="version-token">{{ version }} <small>×{{ count }}</small></span></td>
            <td>{{ row.recorded }} 台<span v-if="row.unrecorded" class="version-unrecorded"> · {{ row.unrecorded }} 台未记录</span></td>
            <td><span>{{ row.checked }} 台已采集</span><n-tag v-if="row.deviations" size="small" :bordered="false" type="warning">{{ row.deviations }} 台版本偏差</n-tag><small v-if="row.recorded + row.unrecorded > row.checked" class="version-unrecorded"> · {{ row.recorded + row.unrecorded - row.checked }} 台未检查</small></td>
          </tr>
        </tbody>
      </table>
      <div v-if="!versionRows.length" class="empty-compact">当前项目尚无记录版本</div>
      <template #footer><n-space justify="end"><n-button size="small" @click="versionOpen = false">关闭</n-button><n-button size="small" secondary :loading="aio.selectionLoading" @click="openVersions"><template #icon><RefreshCw /></template>刷新项目记录</n-button></n-space></template>
    </n-modal>

    <n-modal :show="importOpen" preset="card" title="导入一体机清单" class="import-modal" :bordered="false" @update:show="!$event && closeImport()">
      <div class="mini-stepper">
        <span :class="{ active: importStage === 'select' }"><b>1</b>选择文件</span><i></i>
        <span :class="{ active: importStage === 'preview' }"><b>2</b>校验预览</span><i></i>
        <span :class="{ active: importStage === 'done' }"><b>3</b>应用结果</span>
      </div>
      <n-spin :show="aio.importLoading">
        <div v-if="importStage === 'select'" class="upload-zone" @click="chooseInventoryFile">
          <FileSpreadsheet :size="36" />
          <strong>选择 CSV 清单</strong>
          <span>必填：名称、IP、MAC；空间路径可选，格式为“项目/楼幢/楼层/区域”；位置可填写具体安装位置。</span>
          <n-button size="small" type="primary" @click.stop="chooseInventoryFile"><template #icon><Upload /></template>选择文件</n-button>
          <n-space><n-button size="small" @click.stop="downloadInventoryTemplate">下载导入模板</n-button><n-button size="small" @click.stop="downloadSpacePaths">下载项目空间路径清单</n-button></n-space>
        </div>
        <div v-else-if="importStage === 'preview' && importSession" class="import-preview">
          <div class="import-file">
            <FileSpreadsheet :size="20" />
            <span><strong>{{ importSession.fileName }}</strong><small>{{ importSession.counts.total }} 行 · 本地预览，不上传原始文件</small></span>
            <n-tag size="small" :type="importSession.counts.invalid ? 'warning' : 'success'" :bordered="false">{{ importSession.counts.invalid ? "存在格式错误" : "格式已解析" }}</n-tag>
          </div>
          <div class="classification-grid">
            <span class="success"><strong>{{ importSession.counts.newPending }}</strong><em>新增待实施</em></span>
            <span><strong>{{ importSession.counts.existingUnchanged }}</strong><em>已存在无变化</em></span>
            <span class="info"><strong>{{ importSession.counts.platformExisting }}</strong><em>平台已存在</em></span>
            <span class="warning"><strong>{{ importSession.counts.conflicts }}</strong><em>信息冲突</em></span>
          </div>
          <n-alert v-if="importSession.counts.existingUnchanged" type="warning" :bordered="false">
            已存在 {{ importSession.counts.existingUnchanged }} 条重复的 MAC 一体机，导入将忽略这些记录。
          </n-alert>
          <div class="import-preview-table-header">
            <table class="workbench-table compact import-preview-table__table">
              <colgroup><col class="import-col-apply" /><col class="import-col-result" /><col class="import-col-name" /><col class="import-col-ip" /><col class="import-col-mac" /><col class="import-col-space" /><col class="import-col-action" /></colgroup>
              <thead><tr><th>应用</th><th>结果</th><th>名称</th><th>IP</th><th>MAC</th><th>空间位置</th><th>处理</th></tr></thead>
            </table>
          </div>
          <div class="import-preview-table">
            <table class="workbench-table compact import-preview-table__table">
              <colgroup><col class="import-col-apply" /><col class="import-col-result" /><col class="import-col-name" /><col class="import-col-ip" /><col class="import-col-mac" /><col class="import-col-space" /><col class="import-col-action" /></colgroup>
              <tbody>
                <tr v-for="item in importSession.items" :key="item.rowNumber">
                  <td>
                    <n-checkbox
                      :checked="item.selected"
                      :disabled="!['new_pending', 'existing_changed', 'platform_existing'].includes(item.classification)"
                      @update:checked="(checked) => toggleImportItem(item, checked)"
                    />
                  </td>
                  <td><n-tag size="small" :type="classificationTone(item.classification)" :bordered="false">{{ classificationLabel(item.classification) }}</n-tag></td>
                  <td>{{ item.values.name || "—" }}</td>
                  <td class="mono">{{ item.values.ip || "—" }}</td>
                  <td class="mono">{{ item.displayMac ?? item.values.mac ?? "—" }}</td>
                  <td :title="item.values.spacePath"><span>{{ item.values.spacePath || "未选择" }}</span><small v-if="item.values.addrAlias">{{ item.values.addrAlias }}</small></td>
                  <td class="import-action-copy">{{ classificationAction(item) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <div class="inline-notice info">
            <CheckCircle2 :size="17" />
            <div><strong>确认后只保存在当前电脑</strong><span>未注册一体机留在本机待实施清单，部署后才注册到平台；已注册一体机请在详情中编辑。</span></div>
          </div>
        </div>
        <div v-else class="result-state">
          <span class="result-icon success"><CheckCircle2 :size="32" /></span>
          <strong>已完成导入</strong>
          <p>已导入一体机 {{ importSession?.counts.selected ?? 0 }} 台</p>
        </div>
      </n-spin>
      <template #footer>
        <n-space justify="end">
          <n-button v-if="importStage === 'preview'" size="small" secondary @click="discardImport">放弃本次导入</n-button>
          <n-button size="small" @click="closeImport">{{ importStage === "done" ? "完成" : "关闭" }}</n-button>
          <n-button
            v-if="importStage === 'preview'"
            size="small"
            type="primary"
            :loading="aio.importLoading"
            :disabled="!importSession?.counts.selected"
            @click="applyImport"
          >
            <template #icon><Download /></template>
            确认应用 {{ importSession?.counts.selected ?? 0 }} 台
          </n-button>
        </n-space>
      </template>
    </n-modal>
  </section>
</template>

<style scoped>
.node-space-cell > span, .node-space-cell > small { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.import-preview-table__table .import-col-apply { width: 6%; }
.import-preview-table__table .import-col-result { width: 12%; }
.import-preview-table__table .import-col-name { width: 15%; }
.import-preview-table__table .import-col-ip { width: 15%; }
.import-preview-table__table .import-col-mac { width: 18%; }
.import-preview-table__table .import-col-space { width: 20%; }
.import-preview-table__table .import-col-action { width: 14%; }
.service-section-heading,
.service-observation > header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.service-section-heading { margin: 14px 0 8px; }
.service-section-heading h3 { margin: 0; }
.service-check-summary { display: flex; flex-wrap: wrap; justify-content: space-between; gap: 0 12px; }
.service-check-meta { margin: 4px 0; color: var(--inx-color-text-secondary); font-size: 12px; line-height: 1.5; }
.node-table td > small { margin-top: 1px; }
.node-table .node-col-select { width: 34px; }
.node-table .node-col-select ~ .node-col-location { width: auto; }
.node-table .node-select-cell { padding-left: 10px; padding-right: 6px; }
.node-table .node-selected td { background: var(--inx-color-selected); }
.service-check-cell > small { display: block; margin-top: 2px; white-space: nowrap; }
.service-observation-list { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 8px; margin-top: 8px; }
.service-observation { min-width: 0; padding: 10px; border: 1px solid var(--inx-color-border); border-radius: 6px; background: var(--inx-color-surface-subtle); }
.service-observation > header strong { min-width: 0; font-size: 13px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.service-observation-status { display: inline-flex; align-items: center; gap: 4px; min-height: 22px; flex: none; color: var(--inx-color-text-secondary); font-size: 12px; white-space: nowrap; cursor: default; }
.service-observation-status svg { flex: none; color: var(--inx-color-text-tertiary); }
.service-observation-status.success svg { color: var(--inx-color-operation); }
.service-observation-status.warning svg { color: var(--inx-color-warning); }
.service-observation-status.error svg { color: var(--inx-color-danger); }
.service-observation .detail-grid { grid-template-columns: 72px minmax(0, 1fr); gap: 4px 8px; margin-top: 5px; font-size: 12px; line-height: 1.4; }
.service-observation dt { white-space: nowrap; }
.service-observation dd { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.service-observation .service-observation-message { white-space: normal; overflow-wrap: anywhere; }
.aio-detail-drawer .drawer-identity strong { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.service-check-failure { margin-top: 10px; }
.version-unrecorded { color: var(--inx-color-text-secondary); font-size: 12px; }
</style>
