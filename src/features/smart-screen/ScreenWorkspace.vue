<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { NAlert, NButton, NSelect, NTag, useMessage } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { CircleAlert, CircleX, History, Plus, RefreshCw, Upload } from "lucide-vue-next";
import { useProjectStore } from "@/stores/projects";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenAction, SmartScreen } from "@/shared/model/screen";
import ScreenList from "./ScreenList.vue";
import ScreenLocalEditor from "./ScreenLocalEditor.vue";
import ScreenImportDialog from "./ScreenImportDialog.vue";
import ScreenDetail from "./ScreenDetail.vue";
import ScreenMergeDialog from "./ScreenMergeDialog.vue";
import ScreenStatusDialog from "./ScreenStatusDialog.vue";
import ScreenOperations from "./ScreenOperations.vue";
import ScreenVersionSyncDialog from "./ScreenVersionSyncDialog.vue";
import ScreenTakeoverDialog from "./ScreenTakeoverDialog.vue";
import "./screen.css";

const store = useSmartScreensStore();
const projects = useProjectStore();
const router = useRouter();
const route = useRoute();
const message = useMessage();
const prototype = useScreenAdapter().mode === "prototype";
const alerts = computed(() => {
  const messages = [store.error];
  if (store.projectId) {
    if (!prototype) messages.push(store.snapshot.platformMessage ?? "");
    if (!store.snapshot.localOnly && !store.platformAvailable) {
      messages.push("平台不可用 · 已注册屏显示缓存，仍可检查 IP、设备、MAC 和采集诊断；平台写入及已注册屏的设备写操作暂不可用。");
    }
  }
  return [...new Set(messages.map((text) => text.trim()).filter(Boolean))];
});
async function selectBusinessProject(id: string) {
  try { await useScreenAdapter().selectBusinessProject?.(store.projectId, id); await store.refresh({ refreshPlatform: true }); }
  catch (error) { message.error(error instanceof Error ? error.message : "选择业务项目失败"); }
}
const operations = computed(() => route.path.endsWith("/operations"));
const editorOpen = ref(false), importOpen = ref(false), mergeOpen = ref(false), statusOpen = ref(false);
const editing = ref<SmartScreen>();
const detailId = ref("");
const historyOpen = ref(false);
const versionOpen = ref(false), versionIds = ref<string[]>([]);
const operationSession = ref(0);
function findScreen(id: string) {
  return store.snapshot.screens.find(screen => screen.id === id)
    ?? store.snapshot.screens.find(screen => screen.aliases.includes(id));
}
const detail = computed(() => findScreen(detailId.value));
watch(() => projects.activeProjectId, async (id) => {
  editorOpen.value = importOpen.value = mergeOpen.value = statusOpen.value = false;
  detailId.value = ""; editing.value = undefined;
  historyOpen.value = false;
  versionOpen.value = false; versionIds.value = [];
  await store.bindProject(id);
}, { immediate: true });
watch([() => projects.activeProjectId, () => projects.allowsAccess("platform")], ([id, available], [previousId, previousAvailable]) => {
  if (id && id === previousId && id === store.projectId && available !== previousAvailable) {
    void store.refresh({ refreshPlatform: true });
  }
});
onBeforeUnmount(() => store.stop());
function edit(screen?: SmartScreen) { editing.value = screen; editorOpen.value = true; }
function openScreenDetail(id: string) {
  const screen = findScreen(id);
  if (!screen) { message.info("该屏记录已移除，仍可查看本次操作结果和日志。"); return; }
  detailId.value = screen.id;
}
function operate(ids: string[], action: ScreenAction = "install") {
  ++operationSession.value;
  store.selectedIds = ids; store.operation = action; store.openedTaskId = ""; detailId.value = ""; historyOpen.value = false;
  void router.push("/screen/operations");
}
function recheckStatus(ids: string[]) { statusOpen.value = false; historyOpen.value = false; operate(ids, "ping"); }
function checkVersions(ids: string[]) { detailId.value = ""; historyOpen.value = false; versionIds.value = [...ids]; versionOpen.value = true; }
async function refresh() { await store.refresh({ refreshPlatform: true }); message.info(store.platformAvailable ? `已读取${prototype ? '模拟' : ''}平台数据，${store.candidates.length} 组疑似重复待核对` : "平台不可用，可以继续维护本机未注册屏"); }
</script>

<template>
  <section class="screen-page" data-testid="screen-prototype">
    <header class="page-header">
      <div class="screen-heading"><h1>{{ operations ? '智能屏操作' : '智能屏列表' }}</h1><n-tag v-if="prototype" size="small" :bordered="false" type="info" title="模拟数据，不连接设备或写入数据库">交互原型</n-tag></div>
      <div class="page-actions" v-if="!operations">
        <n-button size="small" :disabled="!store.projectId" @click="refresh"><template #icon><RefreshCw :size="15" /></template>刷新平台</n-button>
        <n-button size="small" :disabled="!store.projectId" @click="importOpen = true"><template #icon><Upload :size="15" /></template>导入智能屏</n-button>
        <n-button size="small" type="primary" :disabled="!store.projectId" @click="edit()"><template #icon><Plus :size="16" /></template>新增智能屏</n-button>
      </div>
      <div v-else class="page-actions"><n-button size="small" @click="router.push('/screen/nodes')">返回智能屏列表</n-button><n-button size="small" secondary @click="historyOpen = true"><template #icon><History :size="15" /></template>查看历史记录</n-button></div>
    </header>
    <n-alert v-if="alerts.length" class="screen-workspace-alert" :type="store.error ? 'error' : 'warning'" :show-icon="false">
      <div v-for="alert in alerts" :key="alert" class="screen-workspace-alert-message">
        <component :is="store.error ? CircleX : CircleAlert" class="screen-workspace-alert-icon" aria-hidden="true" />
        <span>{{ alert }}</span>
      </div>
    </n-alert>
    <div v-if="!store.projectId" class="screen-empty">请先在顶部选择项目。</div>
    <template v-else>
      <n-alert v-if="store.snapshot.availableProjects?.length && !store.snapshot.businessProjectId" type="info">请选择本机项目对应的业务项目，确认后该项目的资料和历史将固定在此范围。<n-select aria-label="平台业务项目" :options="store.snapshot.availableProjects.map(item => ({ label: item.name, value: item.id }))" @update:value="selectBusinessProject" /></n-alert>
      <n-alert v-if="store.snapshot.localOnly" type="info" :show-icon="true">当前为本机项目，可以新增、导入和维护未注册屏；配置平台连接后可读取平台资料和登记设备。</n-alert>
      <screen-operations v-if="operations" :key="`${store.projectId}-${operationSession}`" v-model:history-open="historyOpen" @status="statusOpen = true" @edit="edit" @merge="mergeOpen = true" @versions="checkVersions" @detail="openScreenDetail" />
      <screen-list v-else @detail="openScreenDetail" @operate="operate(store.selectedIds)" @register="operate([$event], 'register')" @merge="mergeOpen = true" @status="statusOpen = true" />
    </template>
    <screen-local-editor :key="`${store.projectId}-${editing?.id ?? 'new'}`" v-model:show="editorOpen" :screen="editing" />
    <screen-import-dialog v-model:show="importOpen" />
    <screen-merge-dialog v-model:show="mergeOpen" />
    <screen-status-dialog v-model:show="statusOpen" @recheck="recheckStatus" />
    <screen-version-sync-dialog v-model:show="versionOpen" :ids="versionIds" />
    <screen-takeover-dialog v-if="!prototype" :project-id="store.projectId" :context-key="`${route.path}:${operationSession}:${mergeOpen}:${statusOpen}:${versionOpen}`" />
    <screen-detail :screen="detail" @close="detailId = ''" @edit="edit" @operate="(action) => detail && operate([detail.id], action)" @versions="detail && checkVersions([detail.id])" />
  </section>
</template>
