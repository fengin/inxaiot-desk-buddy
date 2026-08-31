<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
import { NButton, NDescriptions, NDescriptionsItem, NModal, NSpace, NTag } from "naive-ui";
import {
  Boxes,
  ChevronLeft,
  ChevronRight,
  CircleHelp,
  Cpu,
  History,
  Info,
  ListRestart,
  Maximize2,
  Minus,
  MonitorSmartphone,
  Settings,
  X
} from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { RouterLink, useRoute } from "vue-router";

import ActivityPanel from "@/shell/ActivityPanel.vue";
import PreferencesDialog from "@/shell/PreferencesDialog.vue";
import ProjectSwitcher from "@/shell/ProjectSwitcher.vue";
import {
  confirmApplicationExit,
  listenApplicationExitImpact,
  type ApplicationExitImpact
} from "@/shared/api/applicationLifecycle";
import { useActivityStore } from "@/stores/activity";
import { usePreferencesStore } from "@/stores/preferences";
import { useProjectStore } from "@/stores/projects";
import { useDataDirectoryStore } from "@/stores/dataDirectory";
import { useDiagnosticsStore } from "@/stores/diagnostics";

const activity = useActivityStore();
const preferences = usePreferencesStore();
const projects = useProjectStore();
const dataDirectory = useDataDirectoryStore();
const diagnostics = useDiagnosticsStore();
const preferencesOpen = ref(false);
const aboutOpen = ref(false);
const aioExpanded = ref(true);
const route = useRoute();
const exitImpact = ref<ApplicationExitImpact>();
const exitConfirming = ref(false);
let unlistenExitImpact: (() => void) | undefined;
let sessionCheckTimer: number | undefined;

async function verifyActiveSession() {
  const projectId = projects.activeProjectId;
  if (!projectId || !projects.session) return;
  try {
    await projects.checkSession(projectId);
  } catch {
    // Store已将当前项目置为fail-closed，并保留可重试的本地会话。
  }
}

onMounted(async () => {
  await Promise.allSettled([projects.initialize(), dataDirectory.initialize()]);
  await verifyActiveSession();
  sessionCheckTimer = window.setInterval(() => {
    void verifyActiveSession();
  }, 60_000);
  unlistenExitImpact = await listenApplicationExitImpact((impact) => {
    exitImpact.value = impact;
  });
});
watch(aboutOpen, (open) => {
  if (open) void diagnostics.load(true);
});
watch(
  () => projects.activeProjectId,
  () => {
    void verifyActiveSession();
  }
);
onBeforeUnmount(() => {
  unlistenExitImpact?.();
  if (sessionCheckTimer !== undefined) window.clearInterval(sessionCheckTimer);
});

const isTauri = () => typeof window.__TAURI_INTERNALS__ !== "undefined";
const minimiseWindow = async () => { if (isTauri()) await getCurrentWindow().minimize(); };
const toggleWindow = async () => { if (isTauri()) await getCurrentWindow().toggleMaximize(); };
const closeWindow = async () => { if (isTauri()) await getCurrentWindow().close(); };
const confirmSafeExit = async () => {
  exitConfirming.value = true;
  try {
    await confirmApplicationExit();
  } finally {
    exitConfirming.value = false;
  }
};
const toggleOnDoubleClick = (event: MouseEvent) => {
  const target = event.target as HTMLElement;
  if (!target.closest("button, input, select, textarea, a, [role='button']")) void toggleWindow();
};
const handleTitlebarMouseDown = (event: MouseEvent) => {
  if (event.button !== 0 || !isTauri()) return;
  const target = event.target as HTMLElement;
  if (target.closest("button, input, select, textarea, a, [role='button']")) return;
  void getCurrentWindow().startDragging();
};
const toggleAioNavigation = () => {
  if (preferences.navigationCollapsed) {
    preferences.toggleNavigation();
    aioExpanded.value = true;
    return;
  }
  aioExpanded.value = !aioExpanded.value;
};
const businessRouteBlocked = computed(
  () => route.path.startsWith("/aio/") && !projects.businessMenuEnabled
);
const businessRouteMessage = computed(() => {
  if (!projects.activeProject) return "请先在顶部项目切换器中新建或选择项目。";
  if (!projects.databaseConnected) return projects.activeProject.statusMessage || "当前项目数据库尚未就绪。";
  if (projects.session?.state === "expired") return "平台会话已过期，请在项目切换器中重新登录。";
  return "当前项目尚未登录平台，请在项目切换器中完成登录。";
});
</script>

<template>
  <div
    class="desktop-shell"
    :data-navigation-collapsed="preferences.navigationCollapsed"
    :data-activity-open="activity.panelOpen"
  >
    <header class="application-bar" data-tauri-drag-region @mousedown="handleTitlebarMouseDown" @dblclick="toggleOnDoubleClick">
      <div class="brand-lockup" data-tauri-drag-region>
        <span class="brand-mark">INX</span>
        <span class="brand-copy"><strong>INX 实施工作台</strong><small>项目实施与边缘资源维护</small></span>
      </div>
      <div class="application-divider"></div>
      <project-switcher />
      <div class="application-bar__spacer" data-tauri-drag-region></div>
      <div class="application-actions">
        <button class="top-action" type="button" @click="activity.openPanel('tasks')"><ListRestart :size="17" /><span>任务</span><b v-if="activity.activeTaskCount">{{ activity.activeTaskCount }}</b></button>
        <button class="top-action" type="button" data-testid="open-preferences" @click="preferencesOpen = true"><Settings :size="17" /><span>设置</span></button>
        <button class="top-action" type="button" data-testid="open-about" @click="aboutOpen = true"><Info :size="17" /><span>关于</span></button>
      </div>
      <div class="window-controls">
        <button type="button" aria-label="最小化" @click="minimiseWindow"><Minus :size="15" /></button>
        <button type="button" aria-label="最大化或还原" @click="toggleWindow"><Maximize2 :size="13" /></button>
        <button type="button" class="close" aria-label="关闭" @click="closeWindow"><X :size="15" /></button>
      </div>
    </header>

    <aside class="business-navigation">
      <nav class="side-nav" aria-label="产品功能导航">
        <button class="side-link nav-parent" :class="{ active: route.path.startsWith('/aio') }" type="button" @click="toggleAioNavigation">
          <Boxes :size="18" /><span>一体机管理</span><ChevronRight class="nav-parent__arrow" :class="{ expanded: aioExpanded }" :size="15" />
        </button>
        <div v-show="aioExpanded && !preferences.navigationCollapsed" class="sub-nav" :data-disabled="!projects.businessMenuEnabled">
          <router-link to="/aio/nodes" class="sub-link" :aria-disabled="!projects.businessMenuEnabled" @click="!projects.businessMenuEnabled && $event.preventDefault()"><span>一体机列表</span></router-link>
          <router-link to="/aio/release" class="sub-link" :aria-disabled="!projects.businessMenuEnabled" @click="!projects.businessMenuEnabled && $event.preventDefault()"><span>发布参数</span></router-link>
          <router-link to="/aio/operations" class="sub-link" :aria-disabled="!projects.businessMenuEnabled" @click="!projects.businessMenuEnabled && $event.preventDefault()"><span>部署升级</span></router-link>
        </div>
        <button class="side-link planned" type="button" disabled><MonitorSmartphone :size="18" /><span>智能屏管理</span><em>规划中</em></button>
        <button class="side-link planned" type="button" disabled><Cpu :size="18" /><span>网关管理</span><em>规划中</em></button>
      </nav>
      <button class="navigation-collapse" type="button" @click="preferences.toggleNavigation"><ChevronLeft :size="18" /><span>折叠导航</span></button>
    </aside>

    <main class="workbench-content">
      <section v-if="businessRouteBlocked" class="route-guard-empty" data-testid="project-access-guard">
        <Boxes :size="34" />
        <strong>当前项目上下文未就绪</strong>
        <p>{{ businessRouteMessage }}</p>
        <small>业务页面已阻止加载，不会绕过真实数据库、Schema或会话门禁。</small>
      </section>
      <router-view v-else />
    </main>
    <activity-panel />

    <footer class="status-bar">
      <span class="status-segment"><i class="status-dot" :class="projects.databaseConnected ? 'success' : projects.activeProject?.databaseState === 'failed' ? 'error' : ''"></i>{{ projects.databaseConnected ? '工作台数据库已连接' : projects.activeProject?.databaseState === 'failed' ? '工作台数据库连接失败' : '工作台数据库未连接' }}</span>
      <span class="status-segment">{{ projects.session?.state === 'active' ? `平台已登录 · ${projects.session.username}` : projects.session?.state === 'expired' ? '平台会话已过期' : '平台未登录' }}</span>
      <span class="status-segment status-context">{{ projects.activeProject?.name ?? '未选择项目' }}</span>
      <span class="status-segment" :title="dataDirectory.activeDirectory">{{ dataDirectory.status?.restartRequired ? '数据目录待重启切换' : '数据目录已生效' }}</span>
      <button class="status-segment status-activity" type="button" @click="activity.openPanel('logs')">
        <History :size="13" />任务与日志<n-tag v-if="activity.activeTaskCount" size="tiny" type="info" :bordered="false">{{ activity.activeTaskCount }}</n-tag>
      </button>
    </footer>

    <preferences-dialog v-model:show="preferencesOpen" />
    <n-modal v-model:show="aboutOpen" preset="card" title="关于 INX 实施工作台" class="about-modal" :bordered="false">
      <div class="about-product"><span class="brand-mark large">INX</span><div><strong>INX 实施工作台</strong><span>Rust + Tauri 阶段 7.5 最终收口版</span></div></div>
      <p class="modal-description">面向项目实施与维护人员，统一管理一体机部署、升级和结果追踪；生产路径仅使用 Tauri Real Adapter。</p>
      <n-descriptions :column="1" size="small" bordered label-placement="left">
        <n-descriptions-item label="版本">{{ diagnostics.value?.applicationVersion ?? '读取中…' }}</n-descriptions-item>
        <n-descriptions-item label="源码提交"><span class="diagnostic-value">{{ diagnostics.value?.sourceCommit ?? '读取中…' }}</span></n-descriptions-item>
        <n-descriptions-item label="数据模式">Tauri Real Adapter · 浏览器 Fixture 物理隔离</n-descriptions-item>
        <n-descriptions-item label="Schema">本地实际 v{{ diagnostics.value?.localSchemaVersion ?? '—' }} · 工作台支持 v{{ diagnostics.value?.workbenchSchemaVersion ?? '—' }}</n-descriptions-item>
        <n-descriptions-item label="Agent">v{{ diagnostics.value?.agentVersion ?? '—' }} · 协议 {{ diagnostics.value?.agentProtocolVersion ?? '—' }}</n-descriptions-item>
        <n-descriptions-item label="Agent SHA-256"><span class="diagnostic-value">{{ diagnostics.value?.agentSha256 ?? '—' }}</span></n-descriptions-item>
        <n-descriptions-item label="运行平台">{{ diagnostics.value?.operatingSystem ?? '—' }} / {{ diagnostics.value?.architecture ?? '—' }}</n-descriptions-item>
        <n-descriptions-item label="实际数据目录"><span class="diagnostic-value">{{ diagnostics.value?.dataDirectory ?? dataDirectory.activeDirectory ?? '—' }}</span></n-descriptions-item>
        <n-descriptions-item label="待清理本机凭据">{{ diagnostics.value?.pendingSecretCleanupCount ?? '—' }}</n-descriptions-item>
      </n-descriptions>
      <div class="about-notice"><CircleHelp :size="16" />原 Go/Wails 工作台仅在阶段 8 全量用户验收和替换决策后退役。</div>
    </n-modal>
    <n-modal :show="Boolean(exitImpact)" preset="card" title="确认安全关闭" class="compact-modal" data-testid="application-exit-impact" :bordered="false" :mask-closable="false" :close-on-esc="false">
      <p class="modal-description">当前有 {{ exitImpact?.activeTaskCount ?? 0 }} 个活动任务，其中排队 {{ exitImpact?.queuedTaskCount ?? 0 }} 个、运行 {{ exitImpact?.runningTaskCount ?? 0 }} 个。</p>
      <p class="modal-description">安全关闭会取消排队任务，并向运行任务发出取消信号，最多等待 {{ exitImpact?.waitTimeoutSeconds ?? 5 }} 秒；仍未形成最终结果的任务会标记为中断，重启后不会自动重放远端命令。</p>
      <template #footer>
        <n-space justify="end">
          <n-button size="small" @click="exitImpact = undefined">取消关闭</n-button>
          <n-button size="small" type="warning" data-testid="application-exit-confirm" :loading="exitConfirming" @click="confirmSafeExit">安全关闭</n-button>
        </n-space>
      </template>
    </n-modal>
  </div>
</template>
