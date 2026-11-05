<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import { NButton, NCheckbox, NDrawer, NDrawerContent, NInput, NPagination } from "naive-ui";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import type { ScreenOperationInput, ScreenPreflightItem, ScreenTask } from "@/shared/model/screen";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { validateNtpServer } from "@/shared/model/screenNtp";
import { screenSettingsDefaultPatch, screenSettingsEmptyPatch, screenSettingsFields, screenSettingsNeedsReboot,
  screenSettingsReadFailure, screenSettingsRecord, screenSettingsRetryPatch, screenSettingsSelected, screenSettingsStateClass,
  screenSettingsStateLabel, screenSettingsView, type ScreenSettingsKey, type ScreenSettingsPatch, type ScreenSettingsRead } from "@/shared/model/screenSettings";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";
import ScreenOperationStartPanel from "./ScreenOperationStartPanel.vue";
import ScreenOperationFooter from "./ScreenOperationFooter.vue";

const props = defineProps<{ retryTask?: ScreenTask }>();
const emit = defineEmits<{ busy: [value: boolean]; retry: [task: ScreenTask, ids: string[]]; reset: [] }>();
const step = defineModel<number>("step", { required: true });
const store = useSmartScreensStore();
const rows = ref<ScreenSettingsRead[]>([]), included = ref<string[]>([]), checks = ref<ScreenPreflightItem[]>([]);
const patches = reactive<Record<string, ScreenSettingsPatch>>({});
const busy = ref(false), error = ref(""), readingIds = ref<string[]>([]), server = ref("");
const serverMixed = ref(false), serverUnknown = ref(false), submitting = ref(false);
const detailId = ref(""), page = ref(1), concurrency = ref(3);
const retainedNtp = new Map<string, { server: string }>();
const pageSize = 30;
let generation = 0;
let resetting = false;
let resetStepExpected = false;
const names = computed(() => new Map(store.snapshot.screens.map(screen => [screen.id, screen])));
const scope = computed(() => JSON.stringify(store.selectedScreens.map(screen => [screen.id, screen.source, screen.ip, screen.mac, screen.size])));
const formKey = computed(() => JSON.stringify({ project: store.projectId, scope: scope.value, included: included.value, patches, concurrency: concurrency.value }));
const task = computed(() => store.snapshot.tasks.find(item => item.id === store.openedTaskId));
const selectedRows = computed(() => rows.value.filter(row => included.value.includes(row.screenId)));
const ntpSelected = computed(() => selectedRows.value.some(row => !!patches[row.screenId]?.ntp));
const ntpChecked = computed(() => selectedRows.value.length > 0 && selectedRows.value.every(row => !!patches[row.screenId]?.ntp));
const ntpError = computed(() => selectedRows.value.map(row => patches[row.screenId]?.ntp).filter(value => value != null).map(value => validateNtpServer(value.server)).find(Boolean) ?? "");
const ntpClearing = computed(() => selectedRows.value.some(row => patches[row.screenId]?.ntp?.server === ""));
const ntpPlaceholder = computed(() => serverMixed.value ? ntpSelected.value ? "多个目标，保留逐屏值；输入可统一" : "当前有多个地址；勾选后留空将清除" : serverUnknown.value ? "当前地址未全部确认；勾选后留空将清除" : ntpSelected.value ? "留空将清除自定义地址" : "未设置自定义地址，勾选后可修改");
const failedReads = computed(() => rows.value.filter(row => !!row.error));
const displayed = computed(() => rows.value.map(row => ({ ...row, view: checks.value.find(check => check.screenId === row.screenId)?.settings
  ?? screenSettingsView(row, patches[row.screenId] ?? screenSettingsDefaultPatch()) })));
const visibleRows = computed(() => displayed.value.slice((page.value - 1) * pageSize, page.value * pageSize));
const detail = computed(() => displayed.value.find(row => row.screenId === detailId.value));
const detailOpen = computed({ get: () => !!detailId.value, set: value => { if (!value) detailId.value = ""; } });
const pending = computed(() => displayed.value.filter(row => included.value.includes(row.screenId)).reduce((sum, row) => sum + row.view.pendingCount, 0));
const blocked = computed(() => displayed.value.filter(row => included.value.includes(row.screenId)).reduce((sum, row) => sum + row.view.blockedCount, 0));
const reboots = computed(() => displayed.value.filter(row => included.value.includes(row.screenId) && screenSettingsNeedsReboot(row.view)).length);
const allSatisfied = computed(() => checks.value.length > 0 && checks.value.every(row => row.state === "skip"));
const columns = [{ key: "select", title: "选择", width: "38px" }, { key: "name", title: "智能屏", width: "21%" },
  { key: "ip", title: "IP 地址", width: "115px" }, { key: "read", title: "检查状态", width: "105px" },
  { key: "pending", title: "本次设置" }, { key: "detail", title: "详情", width: "58px" }];
const detailColumns = [{ key: "item", title: "项目", width: "142px" }, { key: "change", title: "当前 → 目标" },
  { key: "state", title: "状态", width: "76px" }, { key: "reason", title: "说明", width: "28%" }];
const booleanFields = [{ key: "adb" as const, label: "保持ADB端口" }, { key: "time" as const, label: "校准时间" }, { key: "sleepOff" as const, label: "关闭屏睡眠" }];
const screenName = (id: string) => names.value.get(id)?.name ?? id;
function rowPendingClass(row: typeof displayed.value[number]) {
  if (!included.value.includes(row.screenId) && !busy.value || Object.values(row.view.items).every(item => item.state === "not_selected")) return "screen-muted";
  return row.view.pendingCount ? "screen-warning" : row.view.blockedCount ? "screen-danger" : "screen-success";
}
function rowPendingLabel(row: typeof displayed.value[number], initial = false) {
  if (!included.value.includes(row.screenId) && !busy.value) return "未选择";
  if (row.view.pendingCount) return `待处理 ${row.view.pendingCount} 项`;
  if (row.view.blockedCount) return "目标尚未确认";
  if (Object.values(row.view.items).every(item => item.state === "not_selected")) return "未选择项目";
  return initial ? "默认目标已满足" : "目标已满足";
}
function invalidate() { checks.value = []; }
function reset() {
  if (resetting) return;
  resetting = true;
  ++generation; rows.value = []; included.value = []; readingIds.value = []; busy.value = false; error.value = "";
  server.value = ""; serverMixed.value = false; serverUnknown.value = false; submitting.value = false;
  retainedNtp.clear(); detailId.value = ""; page.value = 1; invalidate();
  for (const id of Object.keys(patches)) delete patches[id];
  if (step.value !== 0) resetStepExpected = true;
  step.value = 0; store.openedTaskId = ""; resetting = false;
}
watch(busy, value => emit("busy", value), { flush: "sync" });
watch(formKey, () => { invalidate(); if (!busy.value) error.value = ""; }, { flush: "sync" });
watch([() => store.projectId, scope, () => props.retryTask?.id], (_, before) => {
  const hadRead = rows.value.length > 0 || busy.value;
  reset(); if (hadRead && before) error.value = "项目、设备范围或身份已变化，请重新检查。";
});
watch(step, (value, previous) => {
  if (value !== 0 || previous <= 0) return;
  if (resetStepExpected) { resetStepExpected = false; return; }
  if (!resetting) reset();
}, { flush: "sync" });
onBeforeUnmount(() => { ++generation; emit("busy", false); });
function retryPatch(id: string) {
  if (!props.retryTask) return screenSettingsDefaultPatch();
  const screen = names.value.get(id);
  const target = props.retryTask.targets.find(item => item.screenId === id || screen?.aliases.includes(item.screenId));
  return target ? screenSettingsRetryPatch(props.retryTask, target) ?? screenSettingsEmptyPatch() : screenSettingsEmptyPatch();
}
async function read() {
  if (busy.value || !store.selectedIds.length) return;
  const adapter = useScreenAdapter();
  if (!adapter.readSettings) { error.value = "当前程序尚未提供智能屏设置读取能力"; return; }
  const ids = [...store.selectedIds], project = store.projectId, key = scope.value, request = ++generation;
  const readConcurrency = Math.min(3, Math.max(1, Math.trunc(concurrency.value || 1)));
  concurrency.value = readConcurrency;
  busy.value = true; error.value = ""; rows.value = []; included.value = []; invalidate();
  retainedNtp.clear();
  let next = 0;
  const current = () => request === generation && project === store.projectId && key === scope.value;
  try {
    const completed = new Map<string, ScreenSettingsRead>();
    await Promise.all(Array.from({ length: Math.min(readConcurrency, ids.length) }, async () => {
      while (current() && next < ids.length) {
        const id = ids[next++]!; readingIds.value.push(id);
        let row: ScreenSettingsRead;
        try {
          const result = await adapter.readSettings!(project, [id]);
          if (result.length !== 1 || result[0]?.screenId !== id) throw new Error("读取结果与目标屏不一致");
          row = result[0];
        } catch (cause) { row = screenSettingsReadFailure(id, cause instanceof Error ? cause.message : "读取失败，请重新检查"); }
        if (!current()) return;
        completed.set(id, row); readingIds.value = readingIds.value.filter(value => value !== id);
        patches[id] = retryPatch(id);
        if (patches[id].ntp) retainedNtp.set(id, { ...patches[id].ntp });
        rows.value = ids.flatMap(value => completed.has(value) ? [completed.get(value)!] : []);
      }
    }));
    if (!current()) return;
    // 初查没有待处理项的屏仍保留，后续可以单独填写NTP目标。
    included.value = [...ids];
    syncNtpInput();
    if (rows.value.some(row => !row.error && Object.values(row.items).some(item => item.current != null && item.state !== "unknown"))) step.value = 1;
    else error.value = "所选智能屏均未取得可靠状态，请检查连接后重新读取。";
  } finally { if (current()) { busy.value = false; readingIds.value = []; } }
}
function syncNtpInput() {
  const targets = selectedRows.value.flatMap(row => patches[row.screenId]?.ntp ? [patches[row.screenId].ntp!.server] : []);
  const values = targets.length ? targets : selectedRows.value.map(row => screenSettingsRecord(row.items.ntp.current).server);
  const known = values.filter((value): value is string => typeof value === "string");
  const unique = [...new Set(known)];
  serverUnknown.value = known.length !== values.length;
  serverMixed.value = unique.length > 1;
  server.value = unique.length === 1 && !serverUnknown.value ? unique[0]! : "";
}
function checked(key: "adb" | "time" | "sleepOff") { return selectedRows.value.length > 0 && selectedRows.value.every(row => patches[row.screenId]?.[key]); }
function mixed(key: "adb" | "time" | "sleepOff") { return !checked(key) && selectedRows.value.some(row => patches[row.screenId]?.[key]); }
function editBoolean(key: "adb" | "time" | "sleepOff", value: boolean) {
  if (busy.value) return;
  for (const row of selectedRows.value) patches[row.screenId][key] = value;
}
function editNtp(value: string) {
  if (busy.value || !ntpSelected.value) return;
  server.value = value; serverMixed.value = false; serverUnknown.value = false;
  for (const row of selectedRows.value) if (patches[row.screenId].ntp !== null) {
    patches[row.screenId].ntp = { server: value.trim() };
    retainedNtp.set(row.screenId, { server: value.trim() });
  }
}
function editNtpEnabled(value: boolean) {
  if (busy.value) return;
  const restoreTargets = selectedRows.value.some(row => retainedNtp.has(row.screenId));
  for (const row of selectedRows.value) {
    const id = row.screenId;
    if (!value) {
      if (patches[id].ntp) retainedNtp.set(id, { ...patches[id].ntp! });
      patches[id].ntp = null;
      continue;
    }
    // 恢复逐屏目标；混合重试未选NTP的屏不会被公共开关额外带入。
    const previous = retainedNtp.get(id);
    patches[id].ntp = restoreTargets ? previous ? { ...previous } : null : { server: server.value.trim() };
    if (patches[id].ntp) retainedNtp.set(id, { ...patches[id].ntp! });
  }
  syncNtpInput();
}
function toggle(id: string, value: boolean) {
  if (busy.value) return;
  included.value = value ? [...new Set([...included.value, id])] : included.value.filter(item => item !== id);
  syncNtpInput();
}
function editDetail(key: ScreenSettingsKey, value: boolean) {
  if (busy.value || step.value !== 1 || !detail.value) return;
  const id = detail.value.screenId;
  if (key === "ntp") {
    if (patches[id].ntp) retainedNtp.set(id, { ...patches[id].ntp! });
    patches[id].ntp = value ? { ...(retainedNtp.get(id) ?? { server: server.value.trim() }) } : null;
    syncNtpInput();
  }
  else patches[id][key] = value;
}
async function submit() {
  if (busy.value || !selectedRows.value.length || ntpError.value) return;
  const adapter = useScreenAdapter();
  if (!adapter.preflightSettings) { error.value = "当前程序尚未提供智能屏设置检查能力"; return; }
  const ids = selectedRows.value.map(row => row.screenId);
  const input: ScreenOperationInput = { action: "screen_settings", targetIds: ids, appVersion: "", abi: "universal", reinstall: false, concurrency: concurrency.value,
    retryOfOperationId: props.retryTask?.id, expectedTargets: Object.fromEntries(ids.map(id => [id, screenMaintenanceFingerprint(names.value.get(id)!)])) };
  const payload = JSON.parse(JSON.stringify(Object.fromEntries(ids.map(id => [id, patches[id]])))) as Record<string, ScreenSettingsPatch>;
  const key = formKey.value, request = ++generation, project = store.projectId;
  const current = () => request === generation && project === store.projectId && key === formKey.value;
  busy.value = true; submitting.value = false; error.value = ""; invalidate();
  try {
    const result = await adapter.preflightSettings(project, input, payload);
    if (!current()) return;
    if (result.length !== ids.length || new Set(result.map(row => row.screenId)).size !== ids.length || result.some(row => !ids.includes(row.screenId))) throw new Error("检查结果与所选智能屏不一致，请重新设置。");
    checks.value = result;
    const blockedRows = result.filter(row => row.state !== "ready" && row.state !== "skip");
    if (blockedRows.length) { error.value = `本批存在不可执行项目，尚未设置任何屏。${blockedRows.map(row => `${row.name}：${row.reason}`).join("；")}`; return; }
    const readyIds = result.filter(row => row.state === "ready").map(row => row.screenId);
    if (!readyIds.length) return;
    submitting.value = true;
    const id = await adapter.execute(project, { ...input, targetIds: readyIds });
    if (!current()) return;
    store.openedTaskId = id; step.value = 2; await store.refresh();
  } catch (cause) { if (current()) { error.value = cause instanceof Error ? cause.message : "设置未提交，请重新确认"; invalidate(); } }
  finally { if (request === generation) { busy.value = false; submitting.value = false; } }
}
</script>

<template>
  <div class="screen-settings-root">
    <p v-if="error" class="screen-settings-error screen-danger" role="alert">{{ error }}</p>
    <p v-if="retryTask && step < 2" class="screen-settings-notice">重试仅保留原任务未完成的项目，已成功或无需执行的项目不会再次选择；请重新核对当前状态。 <n-button text size="tiny" type="primary" :disabled="busy" data-testid="screen-settings-new" @click="reset(); emit('reset')">新的智能屏设置</n-button></p>
    <div v-if="step === 0" class="operation-stage select-stage screen-select-stage screen-settings-select-stage">
      <screen-target-picker :disabled="busy" />
      <screen-operation-start-panel v-model:concurrency="concurrency" title="设置智能屏" :disabled="busy" :screens="store.selectedScreens">
        <template #description><p class="screen-operation-help">读取端口、NTP、时间和睡眠状态，再选择修改项；按需共用一次重启。睡眠仅接电时生效，保留H5屏保。</p></template>
        <template #status><p v-if="busy" class="screen-muted" aria-live="polite" data-testid="screen-settings-read-progress">正在读取 {{ readingIds.map(screenName).join('、') }} · 已完成 {{ rows.length }}/{{ store.selectedIds.length }} 台</p><template v-if="!busy && failedReads.length"><p v-for="row in failedReads.slice(0, 3)" :key="row.screenId" class="screen-danger">{{ screenName(row.screenId) }}：{{ row.error }}</p><p v-if="failedReads.length > 3" class="screen-muted">共 {{ failedReads.length }} 台读取失败</p></template></template>
        <template #action><n-button class="full-button" type="primary" :loading="busy" :disabled="busy || !store.selectedIds.length" data-testid="screen-settings-read" @click="read">{{ rows.length ? '重新检查智能屏' : '检查智能屏' }}</n-button></template>
      </screen-operation-start-panel>
    </div>
    <div v-else-if="step === 1" class="screen-settings-editor">
      <fieldset :disabled="busy" class="screen-settings-form">
        <div class="screen-settings-options"><n-checkbox v-for="field in booleanFields" :key="field.key" :checked="checked(field.key)" :indeterminate="mixed(field.key)" :disabled="busy" :aria-label="field.label" @update:checked="editBoolean(field.key, $event)">{{ field.label }}</n-checkbox><div class="screen-settings-ntp"><n-checkbox :checked="ntpChecked" :indeterminate="ntpSelected && !ntpChecked" :disabled="busy || !selectedRows.length" aria-label="NTP服务器" data-testid="screen-settings-ntp-enabled" @update:checked="editNtpEnabled">NTP服务器</n-checkbox><n-input :value="server" :disabled="busy || !ntpSelected" size="small" :placeholder="ntpPlaceholder" :status="ntpError ? 'error' : undefined" aria-label="NTP服务器" data-testid="screen-settings-ntp" @update:value="editNtp" /></div></div>
        <p v-if="ntpError" class="screen-danger">{{ ntpError }}</p>
        <p v-if="ntpSelected && serverMixed" class="screen-muted">NTP目标已逐屏保留，请查看详情；重新输入可统一所选NTP项目的目标。</p>
        <p v-else-if="ntpClearing" class="screen-warning" data-testid="screen-settings-ntp-clear">已勾选且目标为空：将清除所选NTP项目的自定义地址，使用固件默认。</p>
      </fieldset>
      <compact-operation-table :columns="columns" label="智能屏设置确认" :reset-key="page" class="screen-settings-table">
        <tr v-for="row in visibleRows" :key="row.screenId" :data-screen-id="row.screenId">
          <td><n-checkbox :checked="included.includes(row.screenId)" :disabled="busy" :aria-label="`设置 ${screenName(row.screenId)}`" @update:checked="toggle(row.screenId, $event)" /></td><td :title="screenName(row.screenId)">{{ screenName(row.screenId) }}</td><td>{{ names.get(row.screenId)?.ip }}</td>
          <td :title="checks.find(check => check.screenId === row.screenId)?.reason || row.error" :class="checks.find(check => check.screenId === row.screenId)?.state === 'blocked' || row.view.blockedCount ? 'screen-danger' : 'screen-success'">{{ !included.includes(row.screenId) ? '未选择' : row.error ? '读取失败' : checks.find(check => check.screenId === row.screenId)?.state === 'ready' ? '检查通过' : row.view.blockedCount ? '部分项目不可执行' : '已读取' }}</td>
          <td><span :class="rowPendingClass(row)">{{ rowPendingLabel(row) }}</span><small v-if="row.view.blockedCount" class="screen-danger">{{ row.view.blockedCount }} 项不可执行</small></td>
          <td><n-button text size="tiny" type="primary" data-testid="screen-settings-detail" @click="detailId = row.screenId">详情</n-button></td>
        </tr>
      </compact-operation-table>
    </div>
    <div v-else class="screen-settings-result"><screen-task-results v-if="task" :task="task" @retry="emit('retry', task, $event)" /></div>
    <screen-operation-footer v-if="step === 1" class="screen-settings-footer" :total="rows.length" :selected="selectedRows.length" :concurrency="concurrency">
      <template #changes><span :class="pending ? 'screen-warning' : 'screen-success'">待处理 {{ pending }} 项</span><span aria-hidden="true"> · </span><span :class="reboots ? 'screen-warning' : 'screen-success'">预计重启 {{ reboots }} 台</span><span v-if="reboots" class="screen-muted">（每台最多一次）</span></template>
      <template v-if="rows.length > pageSize" #pagination><n-pagination v-model:page="page" :page-size="pageSize" :item-count="rows.length" size="small" /></template>
      <template #action><n-button size="small" type="primary" :loading="busy" :disabled="busy || !selectedRows.length || !!ntpError || allSatisfied" data-testid="screen-settings-submit" @click="submit">设置智能屏 {{ selectedRows.length }} 台</n-button></template>
      <template #status><p v-if="busy" class="screen-settings-footer-notice" aria-live="polite">{{ submitting ? '正在提交设置任务…' : '正在核对设置内容…' }}</p><p v-if="blocked" class="screen-settings-footer-notice screen-danger">{{ blocked }} 项不可执行，可在逐屏详情取消对应项目</p><p v-if="allSatisfied" class="screen-settings-footer-notice screen-success" data-testid="screen-settings-all-satisfied">所选目标均已满足或未选择，无需执行；仍可勾选NTP调整目标。</p></template>
    </screen-operation-footer>
    <footer v-else-if="step > 1" class="screen-ops-footer">
      <span>各子项和共享重启结果分别记录</span>
      <n-button size="small" :disabled="task && ['running','cancelling'].includes(task.state)" @click="reset(); emit('reset')">新的智能屏设置</n-button>
    </footer>
    <n-drawer v-model:show="detailOpen" width="min(920px,96vw)" class="screen-dialog"><n-drawer-content :title="`${screenName(detailId)} · 设置详情`" closable body-content-class="screen-settings-detail-body">
      <template v-if="detail"><p class="screen-muted">{{ names.get(detailId)?.ip }} · {{ formatDisplayDateTime(detail.readAt) }} · 待处理 {{ detail.view.pendingCount }} 项</p>
        <compact-operation-table :columns="detailColumns" label="智能屏逐项设置详情">
          <tr v-for="field in screenSettingsFields" :key="field.key" :data-setting-key="field.key"><td><n-checkbox v-if="step === 1" :checked="screenSettingsSelected(patches[detailId], field.key)" :disabled="busy || !included.includes(detailId)" :aria-label="`本屏${field.label}`" size="small" @update:checked="editDetail(field.key, $event)">{{ field.label }}</n-checkbox><span v-else>{{ field.label }}</span></td>
            <td :title="`${detail.view.items[field.key].currentDisplay} → ${detail.view.items[field.key].targetDisplay}`">{{ detail.view.items[field.key].currentDisplay }} → {{ detail.view.items[field.key].targetDisplay }}</td>
            <td :class="screenSettingsStateClass(detail.view.items[field.key].state)">{{ screenSettingsStateLabel(detail.view.items[field.key].state) }}</td><td :title="detail.view.items[field.key].reason">{{ detail.view.items[field.key].reason }}</td></tr>
        </compact-operation-table>
        <p class="screen-operation-help">未选项目保留原设置；存在不可执行项目时本批不会提交。NTP仅勾选后处理，勾选且目标为空表示清除自定义地址。</p>
      </template>
    </n-drawer-content></n-drawer>
  </div>
</template>

<style scoped>
.screen-settings-root{display:flex;flex:1;min-height:0;min-width:0;flex-direction:column;overflow:hidden}
.screen-settings-root .screen-settings-select-stage{height:100%;min-height:0}
.screen-settings-editor{display:flex;flex:1;min-height:0;min-width:0;flex-direction:column;gap:8px;padding:10px;overflow:hidden}
.screen-settings-form{flex:none;display:flex;flex-direction:column;gap:8px;margin:0;padding:10px;border:1px solid var(--inx-color-border);border-radius:6px;min-width:0}
.screen-settings-form p{margin:0}
.screen-settings-options{display:flex;align-items:center;gap:16px;font-size:var(--inx-font-size-table);min-width:0}
.screen-settings-options>.n-checkbox,.screen-settings-ntp>.n-checkbox{flex:none;white-space:nowrap}
.screen-settings-ntp{display:flex;flex:none;min-width:0;align-items:center;gap:8px;font-size:var(--inx-font-size-table)}
.screen-settings-ntp>.n-input{flex:0 0 240px;width:240px;min-width:240px}
.screen-settings-footer{margin:0 10px 10px}
.screen-settings-footer-notice{margin:0;line-height:18px}
.screen-settings-notice,.screen-settings-error{flex:none;margin:0;padding:6px 12px;font-size:var(--inx-font-size-table)}
.screen-settings-notice{color:var(--inx-color-text-secondary)}
.screen-settings-result{display:flex;flex:1;min-height:0;padding:10px;overflow:hidden}
:deep(.screen-settings-detail-body){display:flex;flex-direction:column;gap:12px;min-height:0;overflow:hidden}
:deep(.screen-settings-detail-body .compact-operation-table){flex:none;min-height:190px}
@media(max-width:1120px){.screen-settings-options{gap:10px}.screen-settings-ntp{gap:6px}}
</style>
