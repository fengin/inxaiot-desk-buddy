<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, shallowRef, watch } from "vue";
import { NAlert, NButton, NDrawer, NDrawerContent, NInputNumber, NModal, NSelect, useMessage } from "naive-ui";
import { Archive, Check, ChevronRight, Clock3, History, ListChecks, Monitor, Play } from "lucide-vue-next";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useActivityStore } from "@/stores/activity";
import { screenActionLabel, screenActions, screenInstallSizeWarning, screenStateLabel, validateScreenApk } from "@/shared/model/screen";
import type { ScreenApkSelection, ScreenOperationInput, ScreenPreflightItem, ScreenTask, SmartScreen } from "@/shared/model/screen";
import type { ScreenRegistrationMacConfirmation, ScreenRegistrationPreview as RegistrationPreview } from "@/shared/model/screenRegistration";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { screenOperationFlows } from "./screenOperationFlow";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";
import ScreenAppConfig from "./ScreenAppConfig.vue";
import ScreenNtpConfig from "./ScreenNtpConfig.vue";
import ScreenPreflightTable from "./ScreenPreflightTable.vue";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import ScreenInstallFields from "./ScreenInstallFields.vue";
import ScreenSharedHistory from "./ScreenSharedHistory.vue";
import ScreenRegistrationPreview from "./ScreenRegistrationPreview.vue";
import { isScreenReadOnlyAction, screenCriticalDraftFields, screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import "./registration.css";

const historyOpen = defineModel<boolean>("historyOpen", { default: false });
const emit = defineEmits<{ status: []; edit: [screen: SmartScreen]; merge: []; versions: [ids: string[]]; detail: [id: string] }>();
const store = useSmartScreensStore();
const message = useMessage();
const step = ref(0), checking = ref(false), submitting = ref(false);
const checks = ref<ScreenPreflightItem[]>([]), included = ref<string[]>([]), error = ref("");
const preparedInput = ref<ScreenOperationInput>();
const retrySource = ref<{id:string;action:string;targets:string[]}>();
const retryOperationId = computed(() => retrySource.value?.action === store.operation && store.selectedIds.every(id => retrySource.value?.targets.includes(id)) ? retrySource.value.id : undefined);
const criticalDrafts = computed(() => store.selectedScreens.map((screen) => ({ screen, fields: screenCriticalDraftFields(screen, store.snapshot.platformDrafts?.[screen.id]) })).filter((row) => row.fields.length));
const registrationPreview = shallowRef<RegistrationPreview | null>(null);
const macConfirmations = ref<Record<string, ScreenRegistrationMacConfirmation>>({});
const spaceConfirmations = ref<string[]>([]);
const historyTaskId = ref("");
const criticalOpen = ref(false);
const historySource = ref<"local"|"shared">("local");
const form = reactive({ reinstall: false, concurrency: 3 });
const selectedApk = shallowRef<File | ScreenApkSelection | null>(null);
const apkSize = ref<"4" | "10" | null>(null);
const apk = computed(() => selectedApk.value ? { name: selectedApk.value.name, size: selectedApk.value.size, lastModified: selectedApk.value.lastModified, ...("path" in selectedApk.value ? selectedApk.value : {}) } : undefined);
const sizeWarning = computed(() => store.operation === "install" ? screenInstallSizeWarning(store.selectedScreens) : "");
const selectedSize = computed(() => {
  if (screenInstallSizeWarning(store.selectedScreens)) return null;
  const size = store.selectedScreens[0]?.size;
  return size === "4" || size === "10" ? size : null;
});
let request = 0;
const flow = computed(() => screenOperationFlows[store.operation]);
const steps = computed(() => flow.value.steps);
const lastStep = computed(() => steps.value.length - 1);
const task = computed(() => store.snapshot.tasks.find((item) => item.id === store.openedTaskId));
const historyTask = computed(() => store.snapshot.tasks.find((item) => item.id === historyTaskId.value) ?? store.snapshot.tasks[0]);
const selectedAction = computed(() => screenActions.find((item) => item.value === store.operation)!);
const readyCount = computed(() => checks.value.filter((item) => item.state === "ready").length);
const executing = computed(() => Boolean(task.value && ["running", "cancelling"].includes(task.value.state) && step.value === lastStep.value));
const operationOptions = screenActions.map((action) => ({ label: action.label, value: action.value }));
const selectedColumns = [{ key: "name", title: "智能屏" }, { key: "edit", title: "操作", width: "66px" }];
const draftColumns = [{ key: "name", title: "待提交资料" }, { key: "edit", title: "操作", width: "80px" }];
const registrationPending = computed(() => registrationPreview.value?.items.filter((item) => included.value.includes(item.screenId) && (
  item.state !== "ready" || (item.requiredMacConfirmation && macConfirmations.value[item.screenId] !== item.requiredMacConfirmation)
  || (item.needsSpaceConfirmation && !spaceConfirmations.value.includes(item.screenId))
)).length ?? 0);
const registrationReady = computed(() => store.operation !== "register" || Boolean(registrationPreview.value && !registrationPending.value));
const nextLabel = computed(() => {
  if (step.value === 0 && steps.value.length > 2) return steps.value[1].title;
  if (step.value < lastStep.value - 1) return `下一步：${steps.value[step.value + 1].title}`;
  return `${flow.value.executeLabel} ${checks.value.length || registrationPreview.value ? included.value.length : store.selectedIds.length} 台`;
});
const installationReady = computed(() => store.operation !== "install" || (!sizeWarning.value && !validateScreenApk(apk.value).length));
const params = (): ScreenOperationInput => ({ action: store.operation, retryOfOperationId: retryOperationId.value, targetIds: [...store.selectedIds], expectedTargets: Object.fromEntries(store.selectedScreens.map((screen) => [screen.id, screenMaintenanceFingerprint(screen)])), appVersion: apk.value?.appVersion ?? "", abi: "universal", ...form,
  ...(store.operation === "install" ? { applicationId: "xiaoxin", apk: apk.value ? { ...apk.value } : undefined } : {}) });

// 文件用 getter 比较引用，避免 shallowRef 使相同目标列表的刷新也强制重置操作。
watch([() => store.operation, () => store.selectedIds.join("|"), () => selectedApk.value, () => form.reinstall, () => form.concurrency], () => {
  if (step.value === 0) { ++request; checks.value = []; included.value = []; preparedInput.value = undefined; clearRegistration(); checking.value = false; error.value = ""; }
}, { flush: "sync" });
watch(() => store.operation, (nextOperation, previousOperation) => {
  if ([nextOperation,previousOperation].some(value=>['register','app_config','ntp'].includes(value))) { fresh(); submitting.value = false; }
}, { flush: "sync" });
// 预览绑定当前资料与目录。普通轮询中的相同快照不会使预览反复失效。
const registrationFingerprint = computed(() => JSON.stringify({
  targets: store.selectedScreens.map((screen) => [screen.id, screen.revision, screen.name, screen.ip, screen.mac, screen.observedMac, screen.size, screen.spaceId, screen.location, store.snapshot.platformDrafts?.[screen.id]]),
  identities: store.snapshot.screens.map((screen) => [screen.id, screen.ip, screen.mac]),
  spaces: store.snapshot.spaces, spacesAvailable: store.snapshot.spacesAvailable, platformAvailable: store.platformAvailable
}));
watch(registrationFingerprint, () => {
  if (store.operation !== "register" || submitting.value || step.value === lastStep.value) return;
  const wasChecking = checking.value || Boolean(registrationPreview.value);
  fresh();
  if (wasChecking) error.value = "设备资料或平台目录已变化，请重新检查并预览。";
}, { flush: "sync" });
watch([() => store.operation, () => store.selectedScreens.map((screen) => screen.size).sort().join("|"), () => selectedApk.value], ([operation, , file], [, , previousFile]) => {
  if (!file || file !== previousFile) apkSize.value = null;
  if (operation !== "install") return;
  const changedSize = Boolean(file && file === previousFile && selectedSize.value && apkSize.value && selectedSize.value !== apkSize.value);
  if (sizeWarning.value || changedSize) {
    selectedApk.value = null;
    apkSize.value = null;
    fresh(); submitting.value = false;
    if (changedSize) error.value = "所选屏尺寸已变化，请重新选择对应 APK 文件";
    return;
  }
  if (file && selectedSize.value && (file !== previousFile || !apkSize.value)) apkSize.value = selectedSize.value;
}, { flush: "sync" });
watch(() => store.projectId, () => {
  ++request; selectedApk.value = null; apkSize.value = null; checking.value = false; submitting.value = false; historyTaskId.value = "";
  Object.assign(form, { reinstall: false, concurrency: 3 }); criticalOpen.value = false; fresh();
});
onBeforeUnmount(() => { ++request; selectedApk.value = null; });
function clearRegistration() { registrationPreview.value = null; macConfirmations.value = {}; spaceConfirmations.value = []; }
function previous() { if (submitting.value) return; if (step.value > 0) step.value--; if (step.value === 0) { checks.value = []; included.value = []; preparedInput.value = undefined; clearRegistration(); } }
function fresh() { ++request; retrySource.value=undefined; step.value = 0; checks.value = []; included.value = []; preparedInput.value = undefined; clearRegistration(); store.openedTaskId = ""; error.value = ""; checking.value = false; }
function reviewDrafts() { const ids = criticalDrafts.value.map(({ screen }) => screen.id); fresh(); store.selectedIds = ids; store.operation = "register"; }
function editTarget(screenId: string) {
  if (checking.value || submitting.value) return;
  const screen = store.selectedScreens.find((item) => item.id === screenId);
  if (!screen) return;
  criticalOpen.value = false; fresh(); emit("edit", screen);
}
function mergeTargets() { if (submitting.value) return; fresh(); emit("merge"); }
async function execute() {
  if (submitting.value) return;
  const projectId = store.projectId;
  const generation = ++request;
  const input = { ...(preparedInput.value ?? params()), targetIds: [...included.value] };
  submitting.value = true;
  try {
    let id: string;
    if (store.operation === "register") {
      const preview = registrationPreview.value;
      if (!preview || preview.projectId !== projectId || !registrationReady.value) throw new Error("请完成逐屏确认后重新提交。");
      id = await useScreenAdapter().submitPlatformRegistration(projectId, {
        previewId: preview.id, screenIds: [...included.value], macConfirmations: { ...macConfirmations.value }, spaceConfirmations: [...spaceConfirmations.value]
      });
    } else id = await useScreenAdapter().execute(projectId, input);
    if (generation !== request || store.projectId !== projectId) return;
    store.openedTaskId = id; step.value = lastStep.value; await store.refresh();
    if (generation === request && store.projectId === projectId) await useActivityStore().refreshTasks(projectId, id);
  } catch (cause) {
    if (generation !== request || store.projectId !== projectId) return;
    error.value = (cause as Error).message; step.value = 0; checks.value = []; included.value = []; clearRegistration();
  } finally { if (generation === request && store.projectId === projectId) submitting.value = false; }
}
async function next() {
  if (checking.value || submitting.value || !store.selectedIds.length) return;
  error.value = "";
  if (store.operation === "register" && step.value === 0) {
    const generation = ++request, projectId = store.projectId;
    checking.value = true;
    try {
      const preview = await useScreenAdapter().previewPlatformRegistration(projectId, [...store.selectedIds]);
      if (generation !== request || store.projectId !== projectId) return;
      if (preview.projectId !== projectId) throw new Error("预览所属项目不一致，请重新检查。");
      registrationPreview.value = preview;
      included.value = preview.items.filter((item) => item.state === "ready").map((item) => item.screenId);
      macConfirmations.value = {}; spaceConfirmations.value = []; step.value = 1;
    } catch (cause) { if (generation === request && store.projectId === projectId) error.value = (cause as Error).message; }
    finally { if (generation === request && store.projectId === projectId) checking.value = false; }
    return;
  }
  if (store.operation === "register" && !registrationReady.value) { error.value = "请逐屏确认 MAC 来源及所属空间变更，或取消不提交的设备。"; return; }
  if (store.operation === "install") {
    error.value = screenInstallSizeWarning(store.selectedScreens) || validateScreenApk(apk.value).join("；");
    if (error.value) return;
  }
  if (store.operation !== "register" && step.value === 0 && !checks.value.length) {
    const generation = ++request;
    const projectId = store.projectId;
    checking.value = true;
    const input = params();
    try {
      await new Promise((resolve) => setTimeout(resolve, 350));
      if (generation !== request || store.projectId !== projectId) return;
      const result = await useScreenAdapter().preflight(projectId, input);
      if (generation !== request || store.projectId !== projectId) return;
      checks.value = result; included.value = result.filter((item) => item.state === "ready").map((item) => item.screenId);
      preparedInput.value = input;
      if (steps.value.length > 2) { step.value = 1; return; }
      if (result.some((item) => item.state !== "ready")) { error.value = "部分目标不能执行或无需执行，请核对右侧检查结果后确认可执行范围。"; return; }
    } catch (cause) { if (generation === request && store.projectId === projectId) error.value = (cause as Error).message; return; }
    finally { if (generation === request && store.projectId === projectId) checking.value = false; }
  }
  if (!included.value.length) { error.value = "没有检查通过且已勾选的目标，请调整选择。"; return; }
  if (step.value < lastStep.value - 1) { step.value++; return; }
  await execute();
}
function retry(previousTask: ScreenTask, ids: string[]) {
  if(previousTask.action==='ntp'){
    historyOpen.value=false;fresh();store.selectedIds=[...new Set(ids.flatMap(id=>{const current=store.snapshot.screens.find(screen=>screen.id===id||screen.aliases.includes(id));return current?[current.id]:[];}))];store.operation='ntp';
    retrySource.value={id:previousTask.id,action:previousTask.action,targets:[...store.selectedIds]};
    message.info('请重新读取当前NTP设置，核对尚未完成的设置；待核实结果只读取现状，不重复重启。');return;
  }
  if(previousTask.action==='app_config'){historyOpen.value=false;fresh();store.selectedIds=ids.filter(id=>store.snapshot.screens.some(screen=>screen.id===id));store.operation='app_config';message.info('请重新读取当前配置，只选择未完成的修改。已保存但未重启的屏可直接重启小新。');return;}
  if (previousTask.action === "register") {
    const currentIds = ids.flatMap((id) => {
      const screen = store.snapshot.screens.find((item) => item.id === id) ?? store.snapshot.screens.find((item) => item.aliases.includes(id));
      return screen ? [screen.id] : [];
    });
    const missing = ids.length - currentIds.length;
    historyOpen.value = false; fresh(); store.selectedIds = [...new Set(currentIds)]; store.operation = "register";
    if (missing) message.warning(`${missing} 台历史设备记录已不存在，已排除。${currentIds.length ? "其余设备已选择，请重新检查平台当前资料。" : "请从当前列表重新选择设备。"}`);
    else message.info("已选择失败或未提交设备，请重新检查资料与平台当前值后提交");
    return;
  }
  if (!previousTask.input) return;
  historyOpen.value = false; fresh(); store.selectedIds = [...new Set(ids.flatMap((id) => {
    const current = store.snapshot.screens.find((screen) => screen.id === id || screen.aliases.includes(id));
    return current ? [current.id] : [];
  }))]; store.operation = previousTask.input.action;
  selectedApk.value = null;
  Object.assign(form, { reinstall: previousTask.input.reinstall, concurrency: previousTask.input.concurrency });
  retrySource.value={id:previousTask.id,action:previousTask.action,targets:[...store.selectedIds]};
  message.info(previousTask.input.action === "install" ? "已选择失败或未执行设备，请重新选择本地 APK 文件后检查" : "已选择失败或未执行设备，请重新核对后继续");
}
function restartConfig(ids:string[]){historyOpen.value=false;fresh();store.selectedIds=ids.filter(id=>store.snapshot.screens.some(screen=>screen.id===id));store.operation='restart';message.info('已选择需要重启的屏；将使用现有重启流程，不重复保存配置。');}
</script>

<template>
  <section class="operation-shell screen-ops-shell">
    <div class="operation-flow-bar screen-operation-flow">
      <div class="screen-operation-choice"><n-select v-model:value="store.operation" aria-label="选择智能屏操作" :options="operationOptions" size="small" :disabled="step > 0 || checking || submitting" /></div>
      <p class="screen-operation-description" :title="selectedAction.description">{{ selectedAction.description }}</p>
      <div class="process-stepper">
        <template v-for="(item,index) in steps" :key="`${store.operation}-${item.title}`">
          <button @click="index === 0 ? fresh() : step = index" type="button" class="process-step" :class="{active:step===index,done:step>index}" :disabled="index >= step || executing || checking || submitting || (store.operation === 'register' && step === lastStep && index !== 0)">
            <b><Check v-if="step>index" :size="14" /><span v-else>{{ index+1 }}</span></b>
            <span class="process-step-copy"><span class="process-step-heading"><strong>{{ item.title }}</strong><i v-if="index < steps.length-1" class="step-line" :class="{done:step>index}"></i></span><small>{{ item.hint }}</small></span>
          </button>
        </template>
      </div>
    </div>
    <n-alert v-if="error" type="warning" class="screen-ops-alert">{{ error }}</n-alert>
    <screen-app-config v-if="store.operation==='app_config'" v-model:step="step" @busy="checking=$event" />
    <screen-ntp-config v-else-if="store.operation==='ntp'" v-model:step="step" :retry-of-operation-id="retryOperationId" @busy="checking=$event" @retry="retry" @reset="fresh" />
    <div v-else class="operation-content screen-operation-content" :class="{'selection-scroll-owner':step===0,'screen-result-content':step===lastStep}">
      <div v-if="step === 0" class="operation-stage select-stage screen-select-stage">
        <screen-target-picker :disabled="checking || submitting" />
        <aside class="stage-settings screen-parameter-panel">
          <h3>{{ store.operation === 'register' ? '本次提交资料' : `本次${flow.readOnly ? '检查' : '执行'}设置` }}</h3>
          <div class="screen-parameter-content" :class="{ 'screen-parameter-form-scroll': store.operation === 'install', 'inx-scroll-area': store.operation === 'install' }">
          <screen-install-fields v-if="store.operation==='install'" :key="store.projectId" v-model:file="selectedApk" v-model:reinstall="form.reinstall" :real="store.snapshot.mode === 'real'" :disabled="checking || submitting" :size-warning="sizeWarning" />
          <template v-else-if="store.operation==='register'">
            <div class="screen-callout screen-operation-note">新屏注册到平台，已注册的屏只更新信息，MAC信息会自动获取。</div>
            <p class="screen-registration-note">先补齐名称、IP、尺寸和所属空间。已注册屏的修改保留为本机待提交资料，预览确认前不改变平台。</p>
          </template>
          <template v-else-if="store.operation==='time'"><div class="screen-setting-info"><Clock3 :size="17" /><div><strong>当前电脑时间</strong><p>{{ formatDisplayDateTime(new Date().toISOString()) }}</p></div></div><div class="screen-callout">以当前电脑时间同时校准系统时间和硬件时钟，两项回读偏差均不超过 15 秒。保留原时区和自动校时设置；已测4寸固件完全断电后仍可能丢时。</div></template>
          <div v-else class="screen-callout screen-operation-note">
            <template v-if="store.operation==='ping'">从本机检查 IP 可达性。与平台状态有差异时，在检查结果中由你确认是否覆盖；未注册屏仅保留本机结果。</template>
            <template v-else-if="store.operation==='adb'">仅支持已验证的 10 寸屏。先检查当前连接，再确认设置持久 5555 端口并重启，最后验证恢复。</template>
            <template v-else-if="store.operation==='reboot'">所选屏将暂时不可用。确认目标后分批重启，等待系统与管理连接恢复。</template>
            <template v-else-if="store.operation==='restart'">只重启小新应用，保留应用数据。未安装小新的屏不可执行。</template>
            <template v-else-if="store.operation==='mac'">读取物理网卡 MAC 并核对身份，不改写应用设备标识或平台 MAC。</template>
            <template v-else-if="store.operation==='diagnostics'">读取系统、应用、启动信息及限定范围的近期系统日志。完成后可查看或导出诊断文件；完整文件只保存在本机。</template>
            <template v-else>读取系统、管理连接、应用版本和可用空间。不会自动改变平台注册状态。</template>
          </div>
          <div v-if="store.operation !== 'register'" class="screen-target-context">
            <p v-if="!criticalDrafts.length" class="screen-muted">列表 IP 为本次实际连接地址，使用已确认资料；本机待提交修改尚未生效。</p>
            <div v-if="criticalDrafts.length" class="screen-callout screen-critical-summary" data-testid="screen-critical-drafts">
              <b class="screen-warning">{{ criticalDrafts.length }} 台有关键资料待提交</b>
              <p>{{ isScreenReadOnlyAction(store.operation) ? '本次只读检查仍连接下列确认地址，不使用草稿地址。' : '这些屏暂不执行设备写操作，请先核对更新平台或放弃对应修改。' }}</p>
              <div class="screen-critical-actions"><n-button size="tiny" text type="primary" :disabled="checking || submitting" @click="criticalOpen = true">查看修改</n-button><n-button size="tiny" type="warning" secondary :disabled="checking || submitting" @click="reviewDrafts">前往注册/更新到平台</n-button></div>
            </div>
          </div>
          <div v-if="store.operation !== 'register'" class="form-grid compact execution-tuning screen-execution-tuning"><label :title="`最多同时处理 ${form.concurrency} 台，其余设备排队等待`"><span>并发台数</span><n-input-number :value="form.concurrency" size="small" :min="1" :max="3" :disabled="checking || submitting" aria-label="并发台数" @update:value="form.concurrency=$event??1" /></label></div>
          <div class="execution-summary">
            <span><Monitor :size="16" /><strong class="metric-number metric-info" data-testid="screen-summary-target-count">{{ store.selectedIds.length }}</strong> 台目标</span>
            <span v-if="store.operation === 'install'"><Archive :size="16" /><strong class="metric-number" :class="selectedApk ? 'metric-operation' : 'metric-warning'" data-testid="screen-summary-package-count">{{ selectedApk ? 1 : 0 }}/1</strong> 个应用包</span>
          </div>
          </div>
          <div v-if="store.operation === 'register' && store.selectedScreens.length" class="screen-registration-selected-list screen-parameter-device-list" aria-label="编辑所选屏资料">
            <div class="screen-registration-selected-heading"><span>所选设备资料</span><span>{{ store.selectedScreens.length }} 台</span></div>
            <compact-operation-table :columns="selectedColumns" label="所选设备资料">
              <tr v-for="screen in store.selectedScreens" :key="screen.id" class="screen-registration-selected-row" :data-screen-id="screen.id">
                <td><strong :title="store.snapshot.platformDrafts?.[screen.id]?.values.name || screen.name">{{ store.snapshot.platformDrafts?.[screen.id]?.values.name || screen.name }}</strong><small>{{ store.snapshot.platformDrafts?.[screen.id]?.values.ip || screen.ip }} · {{ screen.source === 'local' ? '未注册' : store.snapshot.platformDrafts?.[screen.id] ? '资料待更新' : '已注册' }}</small></td>
                <td><n-button size="tiny" text type="primary" :disabled="checking || submitting" :aria-label="`编辑 ${screen.name} 资料`" @click="editTarget(screen.id)">编辑资料</n-button></td>
              </tr>
            </compact-operation-table>
          </div>
          <screen-preflight-table v-if="checks.length" v-model:included="included" :rows="checks" :disabled="checking || submitting" compact class="screen-parameter-device-list" />
          <div class="screen-parameter-action"><n-button class="full-button" type="primary" :loading="checking || submitting" :disabled="checking || submitting || !store.selectedIds.length || !installationReady" data-testid="screen-operation-first-action" @click="next"><template #icon><ListChecks v-if="steps.length > 2" /><Play v-else /></template>{{ nextLabel }}</n-button></div>
        </aside>
      </div>
      <div v-else-if="store.operation === 'register' && step < lastStep && registrationPreview" class="operation-stage screen-registration-stage">
        <screen-registration-preview :preview="registrationPreview" :spaces="store.snapshot.spaces" :disabled="submitting" v-model:included="included" v-model:mac-confirmations="macConfirmations" v-model:space-confirmations="spaceConfirmations" @edit="editTarget" @merge="mergeTargets" />
      </div>
      <div v-else-if="step < lastStep" class="operation-stage screen-check-stage">
        <div class="screen-result-header"><h3>{{ steps[step].title }}</h3><span class="screen-success">{{ readyCount }} 台{{ store.operation === 'install' && store.snapshot.mode !== 'real' ? '可演示' : '通过' }} <span class="screen-muted">/ {{ checks.length }} 台检查</span></span></div>
        <n-alert v-if="store.operation==='install'" type="info" :show-icon="true" data-testid="screen-apk-check-boundary"><template v-if="store.snapshot.mode === 'real'">已核对 {{ apk?.name }}：小新 {{ apk?.appVersion }}，数字版本 {{ apk?.appVersionCode }}。确认后按下方逐台结果安装，保留应用数据。</template><template v-else>已选择 {{ apk?.name }}。当前仅校验文件后缀和非空，尚未解析包名、版本或架构；后续执行为流程演示，不会安装应用或更新版本。</template></n-alert>
        <n-alert v-if="store.operation==='adb'&&step===2" type="warning" :show-icon="true">将为选中设备保存持久端口并重启。屏会短时不可用，需等待 5555 和系统启动状态回读。</n-alert>
        <n-alert v-if="store.operation==='reboot'" type="warning" :show-icon="true">只重启下面勾选的设备。重启命令发出后无法撤销，取消只能停止尚未开始的操作。</n-alert>
        <screen-preflight-table v-model:included="included" :rows="checks" :disabled="submitting" :time="store.operation === 'time'" />
        <div class="screen-callout">已勾选 {{ included.length }} 台；不适用或无需执行的设备不会被派发。{{ store.operation==='install'?'升级失败不覆盖原有版本。':'' }}</div>
        <n-button v-if="store.operation === 'install' && checks.some(row => row.state === 'skip')" text type="primary" @click="emit('versions', checks.filter(row => row.state === 'skip').map(row => row.screenId))">核对已跳过设备的版本并同步平台</n-button>
        <n-button v-if="criticalDrafts.length" text type="warning" @click="reviewDrafts">处理关键资料待提交修改</n-button>
      </div>
      <div v-else class="operation-stage screen-final-stage"><screen-task-results v-if="task" :task="task" @retry="retry(task,$event)" @status="emit('status')" @versions="emit('versions', $event)" @detail="emit('detail', $event)" /></div>
    </div>
    <footer v-if="step > 0 && !['app_config','ntp'].includes(store.operation)" class="screen-ops-footer"><span>{{ step<lastStep ? `本次${store.operation === 'register' ? '提交' : '执行'} ${included.length} 台${registrationPending ? ` · ${registrationPending} 台待逐屏确认` : ''}` : '结果逐台记录，资产只保留成功后的最终状态' }}</span><n-button @click="previous" v-if="step<lastStep" size="small" :disabled="submitting">上一步</n-button><n-button v-if="step<lastStep" type="success" size="small" :loading="checking||submitting" :disabled="checking || submitting || !store.selectedIds.length || !included.length || !registrationReady" data-testid="screen-operation-submit" @click="next"><template #icon><Play :size="15" /></template>{{ nextLabel }}<ChevronRight :size="14" /></n-button><n-button v-else size="small" :disabled="executing" @click="fresh">新的智能屏操作</n-button></footer>
  </section>

  <n-modal v-model:show="criticalOpen" preset="card" title="关键资料待提交" class="screen-dialog" style="width:min(720px,94vw)">
    <p class="screen-muted">设备检查仍使用当前已确认地址，待提交资料尚未生效。请核对后更新平台，或在编辑中放弃修改。</p>
    <div class="screen-critical-dialog-body screen-parameter-device-list" data-testid="screen-critical-draft-list">
      <compact-operation-table :columns="draftColumns" label="关键资料待提交设备">
        <tr v-for="row in criticalDrafts" :key="row.screen.id" class="screen-critical-target"><td><span :title="`${row.screen.name} · ${row.fields.join('、')}`">{{ row.screen.name }} · {{ row.fields.join('、') }}</span><small>当前 {{ row.screen.ip }}<template v-if="store.snapshot.platformDrafts?.[row.screen.id]?.values.ip !== row.screen.ip"> → 待提交 {{ store.snapshot.platformDrafts?.[row.screen.id]?.values.ip }}</template></small></td><td><n-button size="tiny" text type="primary" :disabled="checking || submitting" @click="editTarget(row.screen.id)">查看修改</n-button></td></tr>
      </compact-operation-table>
    </div>
  </n-modal>

  <n-drawer v-model:show="historyOpen" width="min(1040px,96vw)" class="screen-dialog"><n-drawer-content title="智能屏运维历史记录" closable body-content-class="screen-history-body"><div v-if="store.snapshot.mode==='real'" class="screen-inline-actions"><n-button size="small" :type="historySource==='local'?'primary':'default'" @click="historySource='local'">本机记录</n-button><n-button size="small" :type="historySource==='shared'?'primary':'default'" :disabled="!store.snapshot.businessProjectId" @click="historySource='shared'">平台结果记录</n-button></div><screen-shared-history v-if="historyOpen&&historySource==='shared'&&store.snapshot.mode==='real'" :project-id="store.projectId"/><div v-else class="screen-history-layout screen-history-drawer"><aside class="screen-task-list inx-scroll-area"><div class="screen-section-label">本机记录</div><button v-for="item in store.snapshot.tasks" :key="item.id" type="button" :class="{active:historyTask?.id===item.id}" @click="historyTaskId=item.id"><b>{{ screenActionLabel(item.action) }}</b><span>{{ item.targets.length }} 台 · {{ screenStateLabel[item.state] }}</span><small>{{ formatDisplayDateTime(item.createdAt) }}</small></button><p v-if="!store.snapshot.tasks.length" class="screen-muted">暂无操作记录</p></aside><div class="screen-history-detail screen-local-result-detail"><screen-task-results v-if="historyTask" :task="historyTask" @retry="retry(historyTask,$event)" @status="emit('status')" @versions="emit('versions', $event)" @restart="restartConfig" @logs-opened="historyOpen=false" @detail="emit('detail', $event)"/><div v-else class="screen-empty"><History :size="26"/><span>执行完成后在这里查看逐台结果和日志。</span></div></div></div></n-drawer-content></n-drawer>
</template>
