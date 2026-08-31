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

import type {
  AioNodeListItem,
  ImportClassification,
  ReconciledImportItem
} from "@/shared/model/aio";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";

const aio = useAioNodesStore();
const projects = useProjectStore();
const message = useMessage();
const router = useRouter();
const dialogs = useSystemDialogAdapter();
const search = ref("");
const stateFilter = ref("all");
const selectedNode = ref<AioNodeListItem>();
const versionOpen = ref(false);
const importOpen = ref(false);
const importStage = ref<"select" | "preview" | "done">("select");
let refreshTimer: number | undefined;

const filteredNodes = computed(() => aio.nodes);
const stats = computed(() => aio.stats);
const importSession = computed(() => aio.importPreview?.session);

const versionRows = computed(() => {
  const rows = new Map<string, { service: string; versions: Map<string, number> }>();
  aio.nodes.forEach((node) => node.versions.forEach((version) => {
    const expected = version.expectedVersion;
    if (!expected) return;
    const item = rows.get(version.serviceName) ?? {
      service: version.serviceName,
      versions: new Map<string, number>()
    };
    item.versions.set(expected, (item.versions.get(expected) ?? 0) + 1);
    rows.set(version.serviceName, item);
  }));
  return [...rows.values()];
});

function managementTone(node: AioNodeListItem) {
  if (node.managementState === "managed") return "success";
  if (node.managementState === "conflict") return "error";
  if (node.managementState === "platform_existing") return "info";
  return "warning";
}

function serviceTone(node: AioNodeListItem) {
  if (node.serviceState === "healthy") return "success";
  if (node.serviceState === "warning") return "warning";
  if (node.serviceState === "unreachable") return "error";
  return "default";
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

async function refresh() {
  if (!projects.activeProjectId || !projects.isReady) return;
  await aio.refresh(projects.activeProjectId, search.value, stateFilter.value);
}

function scheduleRefresh() {
  if (refreshTimer !== undefined) window.clearTimeout(refreshTimer);
  refreshTimer = window.setTimeout(() => void refresh(), 180);
}

async function openNode(node: AioNodeListItem) {
  selectedNode.value = node;
  await aio.loadDetail(node.mac);
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
  aio.page = 1;
  scheduleRefresh();
});
watch(
  () => aio.page,
  () => void refresh()
);
watch(
  () => projects.activeProjectId,
  () => {
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
        <n-button size="small" secondary @click="versionOpen = true">
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
      v-else-if="aio.platformIssues.length"
      type="warning"
      :bordered="false"
      class="pending-import-alert"
    >
      平台存在 {{ aio.platformIssues.length }} 条无法形成可靠MAC身份的记录；这些记录未静默并入资产列表。
      <span>{{ aio.platformIssues.slice(0, 3).map((issue) => issue.message).join('；') }}</span>
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
      <button class="summary-item" :class="{ active: stateFilter === 'all' }" type="button" @click="stateFilter = 'all'">
        <span class="summary-label summary-label--all"><Boxes :size="14" />全部一体机</span>
        <span class="summary-value"><strong>{{ stats.total }}</strong><small>当前项目资产视图</small></span>
      </button>
      <button class="summary-item" :class="{ active: stateFilter === 'online' }" type="button" @click="stateFilter = 'online'">
        <span class="summary-label summary-label--online"><Wifi :size="14" />平台在线</span>
        <span class="summary-value"><strong>{{ stats.online }}</strong><small>以平台状态时间为准</small></span>
      </button>
      <button class="summary-item" :class="{ active: stateFilter === 'offline' }" type="button" @click="stateFilter = 'offline'">
        <span class="summary-label summary-label--offline"><WifiOff :size="14" />平台离线</span>
        <span class="summary-value"><strong>{{ stats.offline }}</strong><small>执行前仍会检查SSH</small></span>
      </button>
      <button class="summary-item" :class="{ active: stateFilter === 'pending' }" type="button" @click="stateFilter = 'pending'">
        <span class="summary-label summary-label--pending"><Box :size="14" />待实施</span>
        <span class="summary-value"><strong>{{ stats.pending }}</strong><small>已确认但尚未部署</small></span>
      </button>
      <button class="summary-item warning" :class="{ active: stateFilter === 'conflict' }" type="button" @click="stateFilter = 'conflict'">
        <span class="summary-label summary-label--conflict"><CircleAlert :size="14" />信息冲突</span>
        <span class="summary-value"><strong>{{ stats.conflicts }}</strong><small>处理后才能执行</small></span>
      </button>
    </div>

    <section class="data-panel">
      <header class="data-toolbar">
        <div class="toolbar-spacer"></div>
        <n-input v-model:value="search" size="small" clearable placeholder="搜索名称、IP、MAC或位置" class="search-input">
          <template #prefix><Search :size="15" /></template>
        </n-input>
        <n-select
          v-model:value="stateFilter"
          size="small"
          class="state-select"
          :options="[
            { label: '全部状态', value: 'all' },
            { label: '已管理', value: 'managed' },
            { label: '待实施', value: 'pending' },
            { label: '平台已存在', value: 'platform_existing' },
            { label: '信息冲突', value: 'conflict' }
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
          <thead><tr><th>名称</th><th>IP</th><th>MAC地址</th><th>位置</th><th>部署状态</th><th>平台状态</th><th>服务状态</th><th></th></tr></thead>
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
              <tr
                v-for="node in filteredNodes"
                :key="node.macNormalized"
                tabindex="0"
                @click="openNode(node)"
                @keydown.enter="openNode(node)"
              >
                <td><strong>{{ node.name }}</strong><small>{{ node.lastOperation }}</small></td>
                <td class="mono">{{ node.ip }}</td>
                <td class="mono muted-cell">{{ node.mac }}</td>
                <td>{{ node.location }}</td>
                <td><n-tag size="small" :bordered="false" :type="managementTone(node)">{{ node.deployLabel }}</n-tag></td>
                <td>
                  <span class="state-with-time" :class="node.platformState">
                    <i></i>
                    <span>
                      {{ node.platformState === "online" ? "在线" : node.platformState === "offline" ? "离线" : "未知" }}
                      <small>{{ node.platformUpdatedAt }}</small>
                    </span>
                  </span>
                </td>
                <td><n-tag size="small" :bordered="false" :type="serviceTone(node)">{{ node.serviceLabel }}</n-tag></td>
                <td><ChevronRight :size="16" class="row-chevron" /></td>
              </tr>
            </tbody>
          </table>
          <div v-if="!aio.loading && !filteredNodes.length" class="empty-inline">
            <XCircle :size="34" />
            <strong>没有匹配的一体机</strong>
            <span>调整搜索条件、状态筛选或项目连接后再试。</span>
          </div>
        </n-spin>
      </div>
      <footer class="table-footer">
        <span>每页 {{ aio.pageSize }} 条 · 共 {{ aio.total }} 条</span>
        <n-pagination v-if="aio.total > aio.pageSize" v-model:page="aio.page" :page-size="aio.pageSize" :item-count="aio.total" size="small" />
        <span>数据更新时间：{{ aio.refreshedAt || "—" }}</span>
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
                <dt>资产来源</dt><dd>{{ aio.detail.node.source }}</dd>
                <dt>记录版本</dt><dd>{{ aio.detail.node.version || "平台只读" }}</dd>
                <dt>位置</dt><dd>{{ aio.detail.node.location }}</dd>
                <dt>最近操作</dt><dd>{{ aio.detail.node.lastOperation }}</dd>
              </dl>
            </div>
            <div class="detail-section">
              <h3>服务与版本</h3>
              <div v-if="aio.detail.versions.length" class="service-version-list">
                <div v-for="version in aio.detail.versions" :key="version.serviceName">
                  <span><strong>{{ version.serviceName }}</strong><small>{{ version.expectedImageName ?? "未记录镜像" }}</small></span>
                  <span class="version-copy"><b>{{ version.observedVersion ?? "未检查" }}</b><small>{{ version.observedAt ?? "—" }}</small></span>
                </div>
              </div>
              <div v-else class="empty-compact">尚未检查服务版本</div>
            </div>
            <div class="detail-section">
              <h3>最近检查</h3>
              <dl class="detail-grid">
                <dt>SSH</dt><dd>{{ aio.detail.latestSshCheck?.state ?? "尚未检查" }}</dd>
                <dt>服务</dt><dd>{{ aio.detail.latestServiceCheck?.state ?? "尚未检查" }}</dd>
              </dl>
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
      <p class="modal-description">聚合工作台记录版本和最近一次远端观测结果，不上传任何镜像文件。</p>
      <div class="version-summary">
        <span><strong>{{ versionRows.length }}</strong> 个服务</span>
        <span><strong>{{ aio.nodes.filter((node) => node.versions.length).length }}</strong> 台已检查</span>
      </div>
      <table class="workbench-table compact">
        <thead><tr><th>服务</th><th>记录版本分布</th><th>覆盖节点</th><th>状态</th></tr></thead>
        <tbody>
          <tr v-for="row in versionRows" :key="row.service">
            <td><strong>{{ row.service }}</strong></td>
            <td><span v-for="([version, count]) in row.versions" :key="version" class="version-token">{{ version }} <small>×{{ count }}</small></span></td>
            <td>{{ [...row.versions.values()].reduce((sum, count) => sum + count, 0) }} 台</td>
            <td><n-tag size="small" :bordered="false" :type="row.versions.size > 1 ? 'warning' : 'success'">{{ row.versions.size > 1 ? "存在版本差异" : "版本一致" }}</n-tag></td>
          </tr>
        </tbody>
      </table>
      <div v-if="!versionRows.length" class="empty-compact">当前页尚无记录版本</div>
      <template #footer><n-space justify="end"><n-button size="small" @click="versionOpen = false">关闭</n-button><n-button size="small" secondary @click="refresh"><template #icon><RefreshCw /></template>刷新记录</n-button></n-space></template>
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
            <span class="success"><strong>{{ importSession.counts.newPending }}</strong>新增待实施</span>
            <span><strong>{{ importSession.counts.existingUnchanged }}</strong>已存在无变化</span>
            <span class="info"><strong>{{ importSession.counts.platformExisting }}</strong>平台已存在</span>
            <span class="warning"><strong>{{ importSession.counts.conflicts }}</strong>信息冲突</span>
          </div>
          <n-alert v-if="aio.importPreview?.platformIssues.length" type="warning" :bordered="false">
            平台有 {{ aio.importPreview.platformIssues.length }} 条 MAC 无效记录未参与匹配。
          </n-alert>
          <div class="import-preview-table">
            <table class="workbench-table compact">
              <thead><tr><th>应用</th><th>结果</th><th>名称</th><th>IP</th><th>MAC</th><th>处理</th></tr></thead>
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
            <div><strong>确认后只写入最终资产</strong><span>导入预览、逐行明细和冲突草稿仅保存在当前实例；确认时会重新对账。</span></div>
          </div>
        </div>
        <div v-else class="result-state">
          <span class="result-icon success"><CheckCircle2 :size="32" /></span>
          <strong>导入已应用</strong>
          <p>已写入 {{ importSession?.counts.selected ?? 0 }} 台最终资产；冲突和格式错误数据未写入项目侧。</p>
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
