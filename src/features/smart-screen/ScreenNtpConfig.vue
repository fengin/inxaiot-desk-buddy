<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NInput, NInputNumber, NModal, NPagination, NSelect } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useActivityStore } from "@/stores/activity";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { ntpServerDisplay, validateNtpServer } from "@/shared/model/screenNtp";
import type { ScreenNtpPatch, ScreenNtpRead } from "@/shared/model/screenNtp";
import type { ScreenOperationInput, ScreenPreflightItem, ScreenTask } from "@/shared/model/screen";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

const step = defineModel<number>("step", { required: true });
const props = defineProps<{ retryOfOperationId?: string }>();
const emit = defineEmits<{ busy: [value: boolean]; retry: [task: ScreenTask, ids: string[]]; reset: [] }>();
const store = useSmartScreensStore();
const rows = ref<ScreenNtpRead[]>([]), included = ref<string[]>([]), checks = ref<ScreenPreflightItem[]>([]);
const mode = ref<"keep" | "set" | "default">("keep"), server = ref(""), concurrency = ref(3);
const busy = ref(false), error = ref(""), readingId = ref(""), confirmOpen = ref(false);
const prepared = ref<ScreenOperationInput>(), fingerprints = ref<Record<string, string>>({});
const page = ref(1), pageSize = ref(50), detailId = ref("");
let generation = 0;
const screens = computed(() => new Map(store.snapshot.screens.map(screen => [screen.id, screen])));
const selected = computed(() => rows.value.filter(row => included.value.includes(row.screenId) && row.config));
const mixed = computed(() => new Set(selected.value.map(row => row.config!.server)).size > 1);
const targetServer = (row: ScreenNtpRead) => mode.value === "default" ? "" : mode.value === "set" ? server.value.trim() : row.config!.server;
const changed = computed(() => selected.value.filter(row => row.config!.server !== targetServer(row) || !row.config!.autoTime));
const patches = computed<Record<string, ScreenNtpPatch>>(() => Object.fromEntries(selected.value.map(row => [row.screenId, { server: targetServer(row) }])));
const readyIds = computed(() => checks.value.filter(row => row.state === "ready").map(row => row.screenId));
const rebootCount = computed(() => selected.value.filter(row => readyIds.value.includes(row.screenId) && row.capabilities?.rebootRequired).length);
const task = computed(() => store.snapshot.tasks.find(task => task.id === store.openedTaskId));
const running = computed(() => task.value && ["running", "cancelling"].includes(task.value.state));
const readById = computed(() => new Map(rows.value.map(row => [row.screenId, row])));
const checkById = computed(() => new Map(checks.value.map(row => [row.screenId, row])));
const visibleIds = computed(() => store.selectedIds.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
const detail = computed(() => readById.value.get(detailId.value));
const columns = [{ key: "select", title: "执行", width: "48px" }, { key: "name", title: "智能屏", width: "18%" }, { key: "ip", title: "IP 地址", width: "116px" }, { key: "server", title: "当前服务器", width: "25%" }, { key: "auto", title: "自动校时", width: "76px" }, { key: "state", title: "读取 / 检查结果" }, { key: "detail", title: "详情", width: "54px" }];
const modeOptions = [{ value: "keep", label: "保留各屏地址" }, { value: "set", label: "统一设置地址" }, { value: "default", label: "恢复固件默认" }];
function name(id: string) { return screens.value.get(id)?.name ?? id; }
function invalidate() { checks.value = []; prepared.value = undefined; confirmOpen.value = false; }
function clear() { ++generation; rows.value = []; included.value = []; invalidate(); fingerprints.value = {}; mode.value = "keep"; server.value = ""; busy.value = false; readingId.value = ""; error.value = ""; page.value = 1; detailId.value = ""; }
watch(busy, value => emit("busy", value), { flush: "sync" });
watch([mode, server, concurrency, () => included.value.join("|")], invalidate, { flush: "sync" });
watch(() => props.retryOfOperationId, invalidate, { flush: "sync" });
watch(() => store.projectId, () => { clear(); step.value = 0; store.openedTaskId = ""; concurrency.value = 3; });
watch(() => store.selectedIds.join("|"), () => { if (step.value !== 2) { clear(); step.value = 0; } }, { flush: "sync" });
watch(() => step.value, value => { if (value === 0) clear(); }, { flush: "sync" });
watch(() => store.selectedScreens.map(screenMaintenanceFingerprint).join("|"), () => {
  if (step.value !== 2 && rows.value.length) { clear(); step.value = 0; error.value = "屏的地址、身份或资料已变化，请重新读取当前设置。"; }
}, { flush: "sync" });
watch(pageSize, () => { page.value = 1; });
onBeforeUnmount(() => { ++generation; emit("busy", false); });
function setServer(value: string) { server.value = value; mode.value = value.trim() ? "set" : "default"; }
function toggle(id: string, value: boolean) { included.value = value ? [...included.value, id] : included.value.filter(item => item !== id); }
function readState(id: string) {
  const row = readById.value.get(id), check = checkById.value.get(id);
  if (!row) return readingId.value === id ? "正在读取" : busy.value ? "等待读取" : "未读取";
  if (!row.config) return "读取失败";
  return check ? check.state === "ready" ? "检查通过" : check.state === "skip" ? "无需修改" : "检查未通过" : "读取成功";
}
async function read() {
  const adapter = useScreenAdapter();
  if (busy.value || !adapter.readNtp || !store.selectedIds.length) return;
  const request = ++generation, project = store.projectId, ids = [...store.selectedIds];
  const continuing = step.value === 1, previousIncluded = new Set(included.value), previousRows = new Set(rows.value.map(row => row.screenId));
  fingerprints.value = Object.fromEntries(store.selectedScreens.map(screen => [screen.id, screenMaintenanceFingerprint(screen)]));
  rows.value = []; included.value = []; invalidate(); error.value = ""; busy.value = true; step.value = 1; page.value = 1;
  if (!continuing) { mode.value = "keep"; server.value = ""; }
  try {
    for (const id of ids) {
      readingId.value = id;
      let result: ScreenNtpRead[];
      try { result = await adapter.readNtp(project, [id]); }
      catch (cause) { result = [{ screenId: id, readAt: new Date().toISOString(), config: null, message: (cause as Error).message }]; }
      if (request !== generation || project !== store.projectId) return;
      for (const row of result.filter(row => row.screenId === id)) {
        rows.value.push(row);
        if (row.config && (!continuing || !previousRows.has(id) || previousIncluded.has(id))) included.value.push(id);
      }
    }
    if (!continuing) server.value = mixed.value ? "" : selected.value[0]?.config?.server ?? "";
  } finally { if (request === generation && project === store.projectId) { busy.value = false; readingId.value = ""; } }
}
async function prepare() {
  const adapter = useScreenAdapter();
  if (busy.value || !adapter.preflightNtp || !selected.value.length) return;
  error.value = mode.value === "set" ? validateNtpServer(server.value) : "";
  if (error.value) return;
  const project = store.projectId, request = ++generation, ids = selected.value.map(row => row.screenId);
  const input: ScreenOperationInput = { action: "ntp", retryOfOperationId: props.retryOfOperationId, targetIds: ids, appVersion: "", abi: "universal", reinstall: false, concurrency: concurrency.value, expectedTargets: Object.fromEntries(ids.map(id => [id, fingerprints.value[id]!])) };
  const payload = JSON.parse(JSON.stringify(patches.value)) as Record<string, ScreenNtpPatch>;
  busy.value = true; invalidate();
  try {
    const result = await adapter.preflightNtp(project, input, payload);
    if (request !== generation || project !== store.projectId) return;
    checks.value = result; prepared.value = input;
  } catch (cause) { if (request === generation && project === store.projectId) error.value = (cause as Error).message; }
  finally { if (request === generation && project === store.projectId) busy.value = false; }
}
function confirmSubmit() { if (busy.value || !prepared.value || !readyIds.value.length) return; if (rebootCount.value) confirmOpen.value = true; else void submit(); }
async function submit() {
  if (busy.value || !prepared.value || !readyIds.value.length) return;
  const request = ++generation, project = store.projectId, input = { ...prepared.value, targetIds: [...readyIds.value] };
  busy.value = true; confirmOpen.value = false; error.value = "";
  try {
    const taskId = await useScreenAdapter().execute(project, input);
    if (request !== generation || project !== store.projectId) return;
    store.openedTaskId = taskId; step.value = 2; await store.refresh();
    if (request === generation && project === store.projectId) await useActivityStore().refreshTasks(project, taskId);
  } catch (cause) { if (request === generation && project === store.projectId) { error.value = (cause as Error).message; invalidate(); } }
  finally { if (request === generation && project === store.projectId) busy.value = false; }
}
function restart() { emit("reset"); store.openedTaskId = ""; step.value = 0; clear(); }
function retry(ids: string[]) { if (task.value) emit("retry", task.value, ids); }
</script>

<template>
  <div class="screen-ntp-operation">
    <n-alert v-if="error" type="warning" :show-icon="true">{{ error }}</n-alert>
    <div v-if="step === 0" class="screen-ntp-select">
      <screen-target-picker :disabled="busy" />
      <aside class="screen-ntp-guide"><h3>读取现有 NTP 设置</h3><p>先读取屏的当前服务器地址和自动校时状态。未设置自定义服务器时，地址显示为空。</p><p>支持 IP 地址或主机名，服务器应提供标准 UDP 123 授时服务。可以同时选择 4 寸屏和 10 寸屏。</p><div class="screen-callout">保存时启用自动校时，保留原时区和自动时区设置；按固件需要重启屏，再验证授时结果。</div><n-button type="primary" :loading="busy" :disabled="!store.selectedIds.length || !useScreenAdapter().readNtp" @click="read">读取 {{ store.selectedIds.length }} 台 NTP 设置</n-button></aside>
    </div>
    <div v-else-if="step === 1" class="screen-ntp-editor">
      <div class="screen-ntp-form">
        <label>NTP 服务器</label><n-input :value="mode === 'default' ? '' : server" size="small" :disabled="busy || !selected.length" :maxlength="253" :placeholder="mixed && mode === 'keep' ? '各屏地址不同，修改后统一设置' : 'IP 地址或主机名；未设置时为空'" aria-label="NTP服务器地址" @update:value="setServer" />
        <label>地址处理</label><n-select v-model:value="mode" size="small" :disabled="busy || !selected.length" :options="modeOptions" aria-label="NTP地址处理" />
        <label>并发台数</label><n-input-number v-model:value="concurrency" size="small" :disabled="busy" :min="1" :max="5" aria-label="NTP并发台数" />
        <n-button size="small" :disabled="busy" :loading="Boolean(readingId)" @click="read">重新读取设置</n-button>
      </div>
      <p class="screen-ntp-notice">保存会启用自动校时。恢复固件默认会移除自定义地址；地址保存、生效及实际授时分别验证，服务器不可达时授时可能未完成。</p>
      <div class="screen-ntp-summary"><span>已读取 {{ rows.length }} / {{ store.selectedIds.length }} 台 · 已选 {{ selected.length }} 台 · 本次修改 {{ changed.length }} 台 · 同值验证 {{ selected.length - changed.length }} 台</span><span v-if="readingId">正在读取 {{ name(readingId) }}</span></div>
      <compact-operation-table :columns="columns" label="NTP设置逐台列表" :reset-key="`${page}:${pageSize}`">
        <tr v-for="id in visibleIds" :key="id" :data-screen-id="id"><td><n-checkbox :checked="included.includes(id)" :disabled="busy || !readById.get(id)?.config" :aria-label="`选择 ${name(id)}`" @update:checked="toggle(id, $event)" /></td><td :title="name(id)">{{ name(id) }}</td><td :title="screens.get(id)?.ip">{{ screens.get(id)?.ip }}</td><td :title="readById.get(id)?.config ? ntpServerDisplay(readById.get(id)?.config?.server) : '未读取'">{{ readById.get(id)?.config?.server || '—' }}<small v-if="readById.get(id)?.config && !readById.get(id)?.config?.server">未设置自定义地址</small></td><td>{{ readById.get(id)?.config ? readById.get(id)?.config?.autoTime ? '开启' : '关闭' : '—' }}</td><td :title="checkById.get(id)?.reason || readById.get(id)?.message">{{ readState(id) }}<small>{{ checkById.get(id)?.reason || readById.get(id)?.message }}</small></td><td><n-button text size="tiny" type="primary" :disabled="!readById.get(id)" :aria-label="`查看 ${name(id)} NTP详情`" @click="detailId = id">详情</n-button></td></tr>
      </compact-operation-table>
      <div class="screen-ntp-footer"><n-pagination v-model:page="page" v-model:page-size="pageSize" :item-count="store.selectedIds.length" :page-sizes="[20,50,100]" :page-slot="5" size="small" show-size-picker /><span class="screen-spacer"></span><n-button size="small" :disabled="busy" @click="restart">重新选屏</n-button><n-button v-if="!prepared" type="primary" size="small" :loading="busy && !readingId" :disabled="busy || !selected.length" @click="prepare">检查设置</n-button><n-button v-else type="success" size="small" :disabled="busy || !readyIds.length" @click="confirmSubmit">{{ changed.length ? '保存并生效' : '验证授时' }} {{ readyIds.length }} 台</n-button></div>
    </div>
    <template v-else><screen-task-results v-if="task" :task="task" @retry="retry" /><div class="screen-ntp-footer"><span class="screen-muted">保存、生效和授时结果分别记录；待核实结果只读取现状。</span><span class="screen-spacer"></span><n-button size="small" :disabled="Boolean(running) || busy" @click="restart">新的智能屏操作</n-button></div></template>
    <n-modal :show="Boolean(detailId)" preset="card" :title="`${name(detailId)} · NTP 设置`" style="width:min(620px,94vw)" @close="detailId = ''" @update:show="!$event && (detailId = '')"><dl v-if="detail" class="screen-ntp-facts"><dt>读取时间</dt><dd>{{ formatDisplayDateTime(detail.readAt) }}</dd><dt>服务器地址</dt><dd>{{ detail.config ? ntpServerDisplay(detail.config.server) : '未读取' }}</dd><dt>自动校时</dt><dd>{{ detail.config ? detail.config.autoTime ? '开启' : '关闭' : '未读取' }}</dd><dt>时区 / 自动时区</dt><dd>{{ detail.config ? `${detail.config.timeZone} / ${detail.config.autoTimeZone ? '开启' : '关闭'}` : '未读取' }}</dd><dt>生效方式</dt><dd>{{ detail.capabilities?.rebootRequired ? '保存后重启屏系统' : detail.capabilities?.activation || '未确认' }}</dd><dt>读取说明</dt><dd>{{ detail.message }}</dd></dl></n-modal>
    <n-modal v-model:show="confirmOpen" preset="card" title="确认设置并按需重启屏" style="width:min(620px,94vw)" :mask-closable="false"><p>本次设置或验证 {{ readyIds.length }} 台屏，其中 {{ rebootCount }} 台可能需要重启系统使设置生效。已确认生效的同值设置只验证授时；不能确认系统已加载时，可能需要重启一次。</p><p>重启期间屏端业务及管理连接会短时中断；程序等待恢复后验证配置和授时。自动校时将开启，原时区和自动时区设置保留。</p><template #footer><div class="screen-inline-actions"><n-button size="small" @click="confirmOpen = false">返回检查</n-button><n-button size="small" type="warning" :loading="busy" @click="submit">确认设置并按需重启</n-button></div></template></n-modal>
  </div>
</template>

<style scoped>
.screen-ntp-operation{display:flex;flex:1;flex-direction:column;gap:8px;min-height:0;min-width:0;overflow:hidden}
.screen-ntp-operation>.n-alert{flex:none}.screen-ntp-select{display:grid;grid-template-columns:minmax(0,1.1fr) minmax(300px,.9fr);gap:16px;flex:1;min-height:0}
.screen-ntp-guide{display:flex;flex-direction:column;align-items:flex-start;gap:12px;padding:14px;border:1px solid var(--inx-color-border);border-radius:6px;background:var(--inx-color-surface)}
.screen-ntp-guide h3{margin:0;font-size:14px}.screen-ntp-guide p{margin:0;line-height:1.7}.screen-ntp-guide .screen-callout{font-size:var(--inx-font-size-table);padding:8px 10px}
.screen-ntp-editor{display:flex;flex:1;flex-direction:column;gap:8px;min-height:0;overflow:hidden}.screen-ntp-form{display:grid;flex:none;grid-template-columns:max-content minmax(200px,1fr) max-content minmax(140px,.7fr) max-content 76px max-content;align-items:center;gap:8px}
.screen-ntp-form label{font-size:var(--inx-font-size-base);font-weight:400;white-space:nowrap}.screen-ntp-notice{flex:none;margin:0;font-size:var(--inx-font-size-table);line-height:1.5;color:var(--inx-color-text-secondary)}
.screen-ntp-summary,.screen-ntp-footer{display:flex;flex:none;align-items:center;gap:8px;min-height:28px;font-size:var(--inx-font-size-table)}.screen-ntp-summary{justify-content:space-between}.screen-ntp-footer{flex-wrap:wrap}
.screen-ntp-facts{display:grid;grid-template-columns:max-content minmax(0,1fr);gap:8px 14px;margin:0;font-size:var(--inx-font-size-base);font-weight:400;line-height:1.5}.screen-ntp-facts dt{color:var(--inx-color-text-secondary)}.screen-ntp-facts dd{margin:0;overflow-wrap:anywhere}
@media(max-width:1200px){.screen-ntp-form{grid-template-columns:max-content minmax(150px,1fr) max-content minmax(120px,.8fr)}.screen-ntp-form>.n-button{grid-column:3 / 5;justify-self:end}}
</style>
