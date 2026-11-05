<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { NAlert, NButton, NCheckbox } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useActivityStore } from "@/stores/activity";
import type { ScreenOperationInput, ScreenPreflightItem, ScreenTask } from "@/shared/model/screen";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenOperationStartPanel from "./ScreenOperationStartPanel.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

const props = defineProps<{ action: "ping" | "inspect"; retryOfOperationId?: string }>();
const step = defineModel<number>("step", { required: true });
const concurrency = defineModel<number>("concurrency", { required: true });
const emit = defineEmits<{
  "update:action": [action: "ping" | "inspect"]; busy: [value: boolean]; status: [];
  versions: [ids: string[]]; detail: [id: string]; retry: [task: ScreenTask, ids: string[]]; reset: []; logsOpened: [];
}>();
const store = useSmartScreensStore(), activity = useActivityStore();
const phase = ref<"idle" | "preflight" | "submitting" | "running">("idle");
const error = ref(""), checks = ref<ScreenPreflightItem[]>([]), cancelBusy = ref(false), cancelRequested = ref(false);
type Context = { generation: number; key: string; project: string; taskId: string; input: ScreenOperationInput;
  screens: { id: string; name: string; ip: string }[] };
const context = shallowRef<Context>();
let generation = 0;
const screensById = computed(() => new Map(store.visibleScreens.map(screen => [screen.id, screen])));
const scope = computed(() => JSON.stringify(store.selectedIds.map(id => {
  const screen = screensById.value.get(id);
  return [id, screen ? screenMaintenanceFingerprint(screen) : null];
})));
const formKey = computed(() => JSON.stringify([store.projectId, scope.value, props.action, concurrency.value, props.retryOfOperationId]));
const busy = computed(() => phase.value !== "idle");
const task = computed(() => {
  const current = context.value;
  return current && store.snapshot.tasks.find(task => task.id === current.taskId && task.projectId === current.project && task.action === current.input.action);
});
const rows = computed(() => context.value?.screens.map(screen => ({ ...screen,
  target: task.value?.targets.find(target => target.screenId === screen.id) })) ?? []);
const runningCount = computed(() => rows.value.filter(row => row.target?.state === "running").length);
const completedCount = computed(() => rows.value.filter(row => row.target && !["queued", "running"].includes(row.target.state)).length);
const failedCount = computed(() => rows.value.filter(row => row.target?.state === "failed").length);
const runningNames = computed(() => rows.value.filter(row => row.target?.state === "running").map(row => row.name).join("、"));
function resetState() {
  ++generation;
  if (context.value?.taskId && store.openedTaskId === context.value.taskId) store.openedTaskId = "";
  context.value = undefined; phase.value = "idle"; checks.value = []; error.value = "";
  cancelBusy.value = false; cancelRequested.value = false;
}
function reset() { resetState(); step.value = 0; emit("reset"); }
const current = (value: Context) => value.generation === generation && value.project === store.projectId && value.key === formKey.value;
watch(busy, value => emit("busy", value), { flush: "sync" });
watch(formKey, () => {
  const interrupted = busy.value;
  resetState(); step.value = 0;
  if (interrupted) error.value = "项目、目标身份或检查选项已变化，已停止使用旧页面结果；后台任务请在任务面板查看。";
}, { flush: "sync" });
watch(step, (value, previous) => { if (value === 0 && previous === 1) resetState(); }, { flush: "sync" });
watch([() => task.value?.state, phase], () => {
  const value = context.value, result = task.value;
  if (phase.value !== "running" || !value || !current(value) || !result || ["running", "cancelling"].includes(result.state)) return;
  phase.value = "idle"; step.value = 1;
  void activity.refreshTasks(value.project, value.taskId);
}, { flush: "sync" });
onBeforeUnmount(() => { ++generation; emit("busy", false); });
function networkOnly(checked: boolean) { if (!busy.value) emit("update:action", checked ? "ping" : "inspect"); }
async function start() {
  if (busy.value || !store.selectedIds.length) return;
  if (!Number.isInteger(concurrency.value) || concurrency.value < 1 || concurrency.value > 3) { error.value = "并发台数须为1至3"; return; }
  const ids = [...store.selectedIds], selected = ids.map(id => screensById.value.get(id));
  if (selected.some(screen => !screen)) { error.value = "部分目标已变化，请重新选择智能屏"; return; }
  const input: ScreenOperationInput = { action: props.action, targetIds: ids, appVersion: "", abi: "universal", reinstall: false,
    concurrency: concurrency.value, retryOfOperationId: props.retryOfOperationId,
    expectedTargets: Object.fromEntries(selected.map(screen => [screen!.id, screenMaintenanceFingerprint(screen!)])) };
  const value: Context = { generation: ++generation, key: formKey.value, project: store.projectId, taskId: "", input,
    screens: selected.map(screen => ({ id: screen!.id, name: screen!.name, ip: screen!.ip })) };
  context.value = undefined; error.value = ""; checks.value = []; cancelRequested.value = false; phase.value = "preflight";
  try {
    const result = await useScreenAdapter().preflight(value.project, input);
    if (!current(value)) return;
    if (result.length !== ids.length || ids.some(id => result.filter(row => row.screenId === id).length !== 1)) throw new Error("检查范围与返回目标不一致，请重新检查");
    checks.value = result;
    if (result.some(row => row.state !== "ready")) {
      error.value = "部分目标不能检查或无需执行，请调整选择后重新开始。"; return;
    }
    phase.value = "submitting";
    value.taskId = await useScreenAdapter().execute(value.project, input);
    if (!current(value)) return;
    context.value = value; store.openedTaskId = value.taskId; phase.value = "running";
    await store.refresh();
    if (current(value)) void activity.refreshTasks(value.project, value.taskId);
  } catch (cause) {
    if (current(value)) error.value = cause instanceof Error ? cause.message : "检查任务创建失败，请重新开始";
  } finally {
    if (current(value) && ["preflight", "submitting"].includes(phase.value)) phase.value = "idle";
  }
}
async function cancel() {
  const value = context.value;
  if (!value?.taskId || !current(value) || phase.value !== "running" || cancelBusy.value || cancelRequested.value || task.value?.state === "cancelling") return;
  cancelBusy.value = true;
  try {
    await useScreenAdapter().cancel(value.project, value.taskId);
    if (!current(value)) return;
    cancelRequested.value = true; await store.refresh();
  } catch (cause) { if (current(value)) error.value = cause instanceof Error ? cause.message : "取消请求未完成，请查看当前任务"; }
  finally { if (current(value)) cancelBusy.value = false; }
}
</script>

<template>
  <div class="screen-inspect-root">
    <n-alert v-if="error" type="warning" class="screen-inspect-error" role="alert">{{ error }}</n-alert>
    <div v-if="step === 0" class="screen-select-stage screen-inspect-select-stage">
      <screen-target-picker :disabled="busy" />
      <screen-operation-start-panel v-model:concurrency="concurrency" title="检查设备" :screens="store.selectedScreens" :disabled="busy">
        <template #description><p class="screen-operation-help">检查网络连通性，并读取管理连接、系统和小新基础信息。检查结果与平台状态、版本不一致时，由你另行确认更新。</p></template>
        <n-checkbox :checked="action === 'ping'" :disabled="busy" aria-label="仅检查网络连通性" data-testid="screen-inspect-network-only" @update:checked="networkOnly">仅检查网络连通性</n-checkbox>
        <template #status>
          <span data-testid="screen-inspect-mode">{{ action === 'ping' ? '仅网络：不连接ADB，不读取系统或小新信息' : '完整检查：网络、ADB及系统和小新基础信息' }}</span>
          <span v-if="phase === 'preflight'" aria-live="polite">正在核对所选 {{ store.selectedIds.length }} 台目标…</span>
          <span v-else-if="phase === 'submitting'" aria-live="polite">正在创建检查任务…</span>
          <div v-else-if="phase === 'running'" class="screen-inspect-progress" data-testid="screen-inspect-progress" aria-live="polite">
            <p>正在检查 {{ runningCount }} 台 · 已完成 {{ completedCount }}/{{ rows.length }} 台 · <span :class="failedCount ? 'screen-danger' : 'screen-muted'">失败 {{ failedCount }} 台</span></p>
            <p v-if="runningNames">正在检查：{{ runningNames }}</p>
            <p v-if="cancelRequested || task?.state === 'cancelling'" class="screen-muted">已请求取消，等待当前检查收尾</p>
            <p v-else-if="!runningCount" class="screen-muted">等待后台派发检查</p>
          </div>
          <ul v-if="!busy && checks.some(row => row.state !== 'ready')" class="screen-inspect-checks"><li v-for="row in checks" :key="row.screenId" :data-preflight-screen-id="row.screenId" :class="row.state === 'blocked' ? 'screen-danger' : 'screen-muted'">{{ row.name }}：{{ row.reason }}</li></ul>
          <slot name="context" />
        </template>
        <template #action>
          <n-button v-if="phase === 'running'" :loading="cancelBusy" :disabled="cancelBusy || cancelRequested || task?.state === 'cancelling'" data-testid="screen-inspect-cancel" @click="cancel">{{ cancelRequested || task?.state === 'cancelling' ? '正在取消' : '取消本次检查' }}</n-button>
          <n-button v-else type="primary" :loading="busy" :disabled="busy || !store.selectedIds.length" data-testid="screen-inspect-start" @click="start">开始检查 {{ store.selectedIds.length }} 台</n-button>
        </template>
      </screen-operation-start-panel>
    </div>
    <template v-else>
      <div class="screen-inspect-results screen-final-stage"><screen-task-results v-if="task" :task="task" @status="emit('status')" @versions="emit('versions', $event)" @detail="emit('detail', $event)" @retry="emit('retry', task, $event)" @logs-opened="emit('logsOpened')" /><p v-else class="screen-muted">当前任务已变化，请开始新的检查。</p></div>
      <footer class="screen-ops-footer"><span>网络、设备读取和操作记录分别保留实际结果</span><n-button size="small" :disabled="busy" data-testid="screen-inspect-new" @click="reset">新的设备检查</n-button></footer>
    </template>
  </div>
</template>

<style scoped>
.screen-inspect-root{display:flex;flex:1;flex-direction:column;min-width:0;min-height:0;overflow:hidden}
.screen-inspect-error{flex:none;margin:8px 12px 0;width:auto}
.screen-inspect-progress{min-width:0;overflow-wrap:anywhere}.screen-inspect-progress p{margin:0 0 4px}
.screen-inspect-checks{margin:0;padding-left:16px;line-height:1.7;overflow-wrap:anywhere}
.screen-inspect-results{display:flex;flex:1;min-width:0;min-height:0;padding:10px;overflow:hidden}
</style>
