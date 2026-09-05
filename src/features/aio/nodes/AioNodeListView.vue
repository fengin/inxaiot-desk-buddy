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
  Download,
  FileSpreadsheet,
  GitCompareArrows,
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
const importStage = ref<"select" | "preview" | "done">("select");
let refreshTimer: number | undefined;

const filteredNodes = computed(() => aio.nodes);
const stats = computed(() => aio.stats);
const platformIssueCount = computed(() => aio.platformIssues.length);
const showingPlatformIssues = computed(() => viewMode.value === "platform_issues");
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
    import: "导入",
    merged: "平台接管",
    platform: "平台已有"
  }[value] ?? "未知来源";
}

async function refresh() {
  if (!projects.activeProjectId || !projects.isReady) return;
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
    selectedNode.value = undefined;
    versionOpen.value = false;
    aio.page = 1;
    void refresh();
  }
);
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
      </div>
    </header>

    <n-alert v-if="aio.error" type="error" :bordered="false" closable @close="aio.error = ''">
      {{ aio.error }}
    </n-alert>
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
            <col class="node-col-name" /><col class="node-col-ip" /><col class="node-col-mac" />
            <col class="node-col-location" /><col class="node-col-deploy" /><col class="node-col-platform" />
            <col class="node-col-service" /><col class="node-col-action" />
          </colgroup>
          <thead><tr><th>名称</th><th>IP</th><th>MAC地址</th><th>位置</th><th>部署状态</th><th>平台状态</th><th>最近服务检查</th><th></th></tr></thead>
        </table>
      </div>
      <div class="table-scroll">
        <n-spin :show="aio.loading">
          <table class="workbench-table node-table">
            <colgroup>
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
                  tabindex="0"
                  @click="openNode(node)"
                  @keydown.enter="openNode(node)"
                >
                  <td><strong>{{ node.name }}</strong><small>{{ lastOperationLabel(node) }}</small></td>
                  <td class="mono">{{ node.ip }}</td>
                  <td class="mono muted-cell">{{ node.mac }}</td>
                  <td>{{ node.location }}</td>
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
      <footer class="table-footer">
        <span>{{ showingPlatformIssues ? `每页 ${aio.pageSize} 条 · 共 ${filteredPlatformIssues.length} 条平台待处理记录` : `每页 ${aio.pageSize} 条 · 共 ${aio.total} 条` }}</span>
        <n-pagination v-if="showingPlatformIssues && filteredPlatformIssues.length > aio.pageSize" v-model:page="issuePage" :page-size="aio.pageSize" :item-count="filteredPlatformIssues.length" size="small" />
        <n-pagination v-else-if="aio.total > aio.pageSize" v-model:page="aio.page" :page-size="aio.pageSize" :item-count="aio.total" size="small" />
        <span>数据更新时间：{{ formatDisplayDateTime(aio.refreshedAt) }}</span>
      </footer>
    </section>

    <n-drawer :show="Boolean(selectedNode)" :width="440" placement="right" @update:show="!$event && (selectedNode = undefined)">
      <n-drawer-content title="一体机详情" closable>
        <n-spin :show="aio.detailLoading">
          <template v-if="aio.detail">
            <div class="drawer-identity">
              <span class="feature-icon info"><ServerCog :size="22" /></span>
              <div><strong>{{ aio.detail.node.name }}</strong><span class="mono">{{ aio.detail.node.ip }} · {{ aio.detail.node.mac }}</span></div>
            </div>
            <div class="detail-section">
              <h3>资产关系</h3>
              <dl class="detail-grid">
                <dt>工作台状态</dt><dd>{{ aio.detail.node.deployLabel }}</dd>
                <dt>平台对象ID</dt><dd class="mono">{{ aio.detail.node.platformId ?? "尚未关联" }}</dd>
                <dt>资产来源</dt><dd>{{ sourceLabel(aio.detail.node.source) }}</dd>
                <dt>记录版本</dt><dd>{{ aio.detail.node.version || "平台只读" }}</dd>
                <dt>位置</dt><dd>{{ aio.detail.node.location }}</dd>
                <dt>最近操作</dt><dd>{{ lastOperationLabel(aio.detail.node) }}</dd>
              </dl>
            </div>
            <div class="detail-section">
              <div class="service-section-heading"><h3>服务与版本检查</h3><n-button size="tiny" secondary :loading="Boolean(detailInspection)" :disabled="!projects.isReady || !aio.detail.node.ip || Boolean(detailInspection)" data-testid="check-node-services" @click="checkServices">{{ detailInspection ? '检查中' : '检查服务' }}</n-button></div>
              <p class="service-check-meta">已部署版本为项目共享记录，服务检查仅保存到本机。</p>
              <p class="service-check-meta">本机最近整机检查：{{ formatDisplayDateTime(aio.detail.node.serviceCheck?.lastFullCheckAt, '尚未检查') }}</p>
              <p v-if="detailServices.length" class="service-check-meta">{{ serviceCheckCoverage(aio.detail.node).checked }}/{{ serviceCheckCoverage(aio.detail.node).total }} 项服务有本机实测结果</p>
              <p v-if="detailInspection" class="service-check-meta" data-testid="service-inspection-stage">{{ detailInspectionStage }} · 可在底部查看任务日志</p>
              <p v-if="aio.detail.node.serviceCheck?.lastAttempt?.scope === 'service'" class="service-check-meta">最近仅检查 {{ aio.detail.node.serviceCheck.lastAttempt.serviceName }}，其他服务保留各自检查时间。</p>
              <div v-if="aio.detail.node.serviceCheck?.lastAttempt?.state === 'failed'" class="inline-notice warning service-check-failure" data-testid="service-check-failure"><CircleAlert :size="16" /><div><strong>最近一次检查失败 · {{ formatDisplayDateTime(aio.detail.node.serviceCheck.lastAttempt.checkedAt) }}</strong><span>{{ aio.detail.node.serviceCheck.lastAttempt.error || '未能完成服务检查，请查看任务日志。' }}</span></div></div>
              <div v-if="detailServices.length" class="service-observation-list">
                <div v-for="service in detailServices" :key="service.serviceName" class="service-observation" data-testid="node-service-observation">
                  <header><strong>{{ service.serviceName }}</strong><n-tag size="small" :bordered="false" :type="service.tone">{{ service.label }}</n-tag></header>
                  <dl class="detail-grid">
                    <dt>镜像版本</dt><dd class="mono" :title="service.observation?.actualImage ? '最近检查的实际镜像' : '已部署记录，实际镜像尚未采集'">{{ service.observation?.actualImage || service.deployedImage }}</dd>
                    <template v-if="service.observation?.actualImage && service.deployedImage !== '未记录' && service.observation.actualImage !== service.deployedImage"><dt>已部署版本</dt><dd class="mono">{{ service.deployedImage }}</dd></template>
                    <template v-if="service.observation?.expectedImage && service.observation.expectedImage !== (service.observation.actualImage || service.deployedImage) && service.observation.expectedImage !== service.deployedImage"><dt>生效配置镜像</dt><dd class="mono">{{ service.observation.expectedImage }}</dd></template>
                    <dt>运行状态</dt><dd>{{ serviceRuntimeLabel(service.observation) }}</dd>
                    <dt>检查时间</dt><dd>{{ formatDisplayDateTime(service.observation?.checkedAt, '尚未检查') }}</dd>
                    <dt>检查来源</dt><dd>{{ serviceCheckSourceLabel(service.observation?.source) }}</dd>
                    <template v-if="service.observation?.message"><dt>说明</dt><dd>{{ service.observation.message }}</dd></template>
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
          <span>必填字段：name、ip、mac；文件仅在本机解析</span>
          <n-button size="small" type="primary" @click.stop="chooseInventoryFile"><template #icon><Upload /></template>选择文件</n-button>
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
              <colgroup><col class="import-col-apply" /><col class="import-col-result" /><col class="import-col-name" /><col class="import-col-ip" /><col class="import-col-mac" /><col class="import-col-action" /></colgroup>
              <thead><tr><th>应用</th><th>结果</th><th>名称</th><th>IP</th><th>MAC</th><th>处理</th></tr></thead>
            </table>
          </div>
          <div class="import-preview-table">
            <table class="workbench-table compact import-preview-table__table">
              <colgroup><col class="import-col-apply" /><col class="import-col-result" /><col class="import-col-name" /><col class="import-col-ip" /><col class="import-col-mac" /><col class="import-col-action" /></colgroup>
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
                  <td class="import-action-copy">{{ classificationAction(item) }}</td>
                </tr>
              </tbody>
            </table>
          </div>
          <div class="inline-notice info">
            <CheckCircle2 :size="17" />
            <div><strong>确认后只写入最终资产</strong><span>导入预览、逐行明细和冲突草稿等数据仅存在于当前电脑，完成部署后才同步到平台。</span></div>
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
.service-section-heading,
.service-observation > header { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.service-section-heading h3 { margin: 0; }
.service-check-meta { margin: 4px 0; color: var(--inx-color-text-secondary); font-size: 12px; line-height: 1.5; }
.node-table td:first-child > small { margin-top: 1px; }
.service-check-cell > small { display: block; margin-top: 2px; white-space: nowrap; }
.service-observation { padding: 6px 0; border-bottom: 1px solid var(--inx-color-border); }
.service-observation:last-child { border-bottom: 0; padding-bottom: 0; }
.service-observation > header strong { font-size: 13px; }
.service-observation .detail-grid { grid-template-columns: 72px minmax(0, 1fr); gap: 4px 8px; margin-top: 5px; font-size: 12px; line-height: 1.4; }
.service-observation dd { overflow-wrap: anywhere; }
.service-check-failure { margin-top: 10px; }
.version-unrecorded { color: var(--inx-color-text-secondary); font-size: 12px; }
</style>
