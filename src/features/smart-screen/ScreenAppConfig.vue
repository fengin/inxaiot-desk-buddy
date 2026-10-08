<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NInput, NInputNumber, NModal, NPagination, NPopover, NSelect } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenMaintenanceFingerprint } from "@/shared/model/screenMaintenance";
import type { ScreenOperationInput, ScreenPreflightItem } from "@/shared/model/screen";
import { appConfigChanges, appConfigDisplay, appConfigRestartRequired, appConfigEffectiveAfter, appConfigReadTime, appConfigSources, appEnvironments } from "@/shared/model/screenAppConfig";
import type { AppEnvironment, ScreenAppConfigPatch, ScreenAppConfigRead, ScreenAppConfigDraft } from "@/shared/model/screenAppConfig";
import { appConfigDifferences, appConfigTextEdit, summarizeAppConfigs, summarizeAppConfigValues } from "@/shared/model/screenAppConfigEditor";
import ScreenTargetPicker from "./ScreenTargetPicker.vue";
import ScreenTaskResults from "./ScreenTaskResults.vue";

const step = defineModel<number>("step", { required: true });
const emit = defineEmits<{ busy: [value: boolean] }>();
const store = useSmartScreensStore();
type Mode = "keep" | "set" | "clear";
type Edit = { mode: Mode; value: string };
const environment = ref<AppEnvironment>("pre"), switchEnvironment = ref(false), concurrency = ref(3);
const edits = reactive<Record<string, Edit>>({ otaUrl: { mode: "keep", value: "" }, wsUrl: { mode: "keep", value: "" }, h5Url: { mode: "keep", value: "" } });
const readyMode = ref<"keep" | "on" | "off">("keep");
const names = reactive<Record<string, Edit>>({});
const rows = ref<ScreenAppConfigRead[]>([]), included = ref<string[]>([]), checks = ref<ScreenPreflightItem[]>([]);
const busy = ref(false), error = ref(""), readingId = ref("");
const prepared = ref<ScreenOperationInput>();
const savedDraft = ref<ScreenAppConfigDraft|null>(null), freshRead = ref(false);
const page = ref(1), pageSize = ref(50), detailId = ref("");
const listBody = ref<HTMLElement>();
let draftTimer: ReturnType<typeof setTimeout> | undefined;
let queuedDraft: { project: string; value: ScreenAppConfigDraft } | undefined;
let draftWrites = Promise.resolve();
let restoring = false;
let generation = 0;
const task = computed(() => store.snapshot.tasks.find(t => t.id === store.openedTaskId));
const running = computed(() => task.value && ["running", "cancelling"].includes(task.value.state));
const includedSet = computed(() => new Set(included.value));
const screensById = computed(() => new Map(store.snapshot.screens.map(screen => [screen.id, screen])));
const selected = computed(() => rows.value.filter(r => includedSet.value.has(r.screenId) && r.config));
const fields = [{ key: "otaUrl", label: "OTA 地址" }, { key: "wsUrl", label: "WebSocket(可选)" }, { key: "h5Url", label: "H5地址(可选)" }] as const;
type AddressKey = typeof fields[number]['key'];
const summaries = computed(() => summarizeAppConfigs(selected.value, environment.value));
const patches = computed(() => Object.fromEntries(selected.value.map(row => {
  const patch: ScreenAppConfigPatch = { set: {}, clear: [] };
  if (switchEnvironment.value) patch.set["environments.current"] = environment.value;
  for (const { key } of fields) {
    const field = `environments.${environment.value}.${key}`, edit = edits[key];
    if (edit.mode === "set") patch.set[field] = edit.value.trim();
    else if (edit.mode === "clear") patch.clear.push(field);
  }
  if (readyMode.value !== "keep") patch.set[`environments.${environment.value}.h5ReadyCheckEnabled`] = readyMode.value === "on";
  const name = names[row.screenId];
  if (name?.mode === "set") patch.set.customDeviceName = name.value.trim();
  else if (name?.mode === "clear") patch.clear.push("customDeviceName");
  return [row.screenId, patch];
})));
const changedRows = computed(() => selected.value.filter(row => appConfigChanges(row.config!, patches.value[row.screenId]).length));
const restartCount = computed(() => changedRows.value.filter(row => appConfigRestartRequired(row.config!, patches.value[row.screenId])).length);
const readyIds = computed(() => checks.value.filter(row => row.state === "ready").map(row => row.screenId));
function name(id: string) { return screensById.value.get(id)?.name ?? id; }
function ip(id: string) { return screensById.value.get(id)?.ip ?? ""; }
function changeEnvironment(value: AppEnvironment) {
  if (busy.value || !selected.value.length) return;
  environment.value = value;
  // 只有明确选择才切换；读取自动填充和草稿重读不产生新的修改。
  switchEnvironment.value = selected.value.some(row => row.config!.environments.current !== value);
}
function addressPlaceholder(key: AddressKey) {
  if (!summaries.value[key].total) return "尚未读取";
  if (summaries.value[key].tied) return "多个值，输入后统一修改";
  return key === "wsUrl" ? "ws:// 或 wss://（可留空）" : key === "h5Url" ? "http:// 或 https://（可留空）" : "http:// 或 https://";
}
function addressValue(key: AddressKey) {
  return edits[key].mode === "keep" ? String(summaries.value[key].value ?? "") : edits[key].value;
}
function editAddress(key: AddressKey, value: string) {
  Object.assign(edits[key], appConfigTextEdit(value, summaries.value[key], key !== "otaUrl"));
}
function unifyAddress(key: AddressKey) {
  const value = String(summaries.value[key].value ?? "");
  Object.assign(edits[key], { mode: !value && key !== "otaUrl" ? "clear" : "set", value });
}
function keepAddress(key: AddressKey) { Object.assign(edits[key], { mode: "keep", value: "" }); }
const readyValue = computed(() => readyMode.value === "keep" ? summaries.value.h5ReadyCheckEnabled.value === true : readyMode.value === "on");
function editReady(value: boolean) {
  readyMode.value = value === summaries.value.h5ReadyCheckEnabled.value ? "keep" : value ? "on" : "off";
}
const emptyPatch: ScreenAppConfigPatch = { set: {}, clear: [] };
const listRows = computed(() => {
  const readById = new Map(rows.value.map(row => [row.screenId, row]));
  const checksById = new Map(checks.value.map(row => [row.screenId, row]));
  return store.selectedIds.map(id => {
    const row = readById.get(id);
    const patch = patches.value[id] ?? emptyPatch;
    return { screenId: id, read: row, check: checksById.get(id),
      differences: row?.config ? appConfigDifferences(row.config, summaries.value, environment.value) : [],
      changes: row?.config ? appConfigChanges(row.config, patch) : [] };
  });
});
const visibleRows = computed(() => listRows.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
const detail = computed(() => listRows.value.find(row => row.screenId === detailId.value));
const detailShow = computed({ get: () => Boolean(detailId.value), set: (value: boolean) => { if (!value) detailId.value = ""; } });
function readStatus(row: typeof listRows.value[number]) {
  if (!row.read) return readingId.value === row.screenId ? "正在读取" : busy.value ? "等待读取" : "未完成读取";
  if (!row.read.config) return "读取失败";
  if (row.check) return row.check.state === "blocked" ? "检查未通过" : row.check.state === "ready" ? "检查通过" : "无需修改";
  return freshRead.value || busy.value ? "读取成功" : "待重新读取";
}
function nameValue(row: ScreenAppConfigRead) {
  return names[row.screenId]?.mode === "keep" ? row.config?.customDeviceName ?? "" : names[row.screenId]?.value ?? "";
}
function editName(row: ScreenAppConfigRead, value: string) {
  names[row.screenId] = appConfigTextEdit(value, summarizeAppConfigValues([row.config?.customDeviceName]), true);
}
watch(pageSize, () => { page.value = 1; });
watch([page, pageSize], () => { if (listBody.value) listBody.value.scrollTop = 0; });
function invalidate() { checks.value = []; prepared.value = undefined; }
watch([environment, switchEnvironment, readyMode, concurrency, () => JSON.stringify(edits), () => JSON.stringify(names), () => included.value.join("|")], invalidate);
watch(environment, () => { if(restoring)return;for (const edit of Object.values(edits)) Object.assign(edit, { mode: "keep", value: "" }); readyMode.value = "keep"; switchEnvironment.value = false; },{flush:'sync'});
watch(busy, value => emit("busy", value), { flush: "sync" });
watch(() => store.projectId, () => { void flushDraft();reset();savedDraft.value=null;void loadDraft(); });
watch(() => store.selectedIds.join("|"), () => { if (step.value === 0 && !busy.value) { rows.value = []; invalidate(); } });
onBeforeUnmount(() => { void flushDraft();++generation; emit("busy", false); });
onMounted(loadDraft);
async function loadDraft(){const project=store.projectId;try{const value=await useScreenAdapter().loadAppConfigDraft?.(project);if(project===store.projectId)savedDraft.value=value??null;}catch(cause){if(project===store.projectId)error.value=(cause as Error).message;}}
function flushDraft(){
  if(draftTimer){clearTimeout(draftTimer);draftTimer=undefined;}
  const next=queuedDraft;queuedDraft=undefined;
  const adapter=useScreenAdapter();
  if(next&&adapter.saveAppConfigDraft){draftWrites=draftWrites.then(async()=>{await adapter.saveAppConfigDraft!(next.project,next.value);if(next.project===store.projectId)savedDraft.value=next.value;}).catch(cause=>{if(next.project===store.projectId)error.value=`本机草稿未保存：${(cause as Error).message}`;});}
  return draftWrites;
}
watch([busy,()=>JSON.stringify({environment:environment.value,switchEnvironment:switchEnvironment.value,edits,readyMode:readyMode.value,names,rows:rows.value,includedIds:included.value,targetIds:store.selectedIds})],()=>{
  if(busy.value||restoring||step.value!==1||!rows.value.length||!useScreenAdapter().saveAppConfigDraft)return;
  queuedDraft={project:store.projectId,value:JSON.parse(JSON.stringify({environment:environment.value,switchEnvironment:switchEnvironment.value,edits,readyMode:readyMode.value,names,rows:rows.value,includedIds:included.value,targetIds:store.selectedIds})) as ScreenAppConfigDraft};
  if(draftTimer)clearTimeout(draftTimer);draftTimer=setTimeout(()=>void flushDraft(),400);
});
function restoreDraft(){
  const draft=savedDraft.value;if(!draft)return;
  const ids=draft.targetIds.filter(id=>store.snapshot.screens.some(screen=>screen.id===id));
  if(!ids.length){error.value='草稿中的屏已不在当前项目，请重新选屏';return;}
  restoring=true;environment.value=draft.environment;switchEnvironment.value=draft.switchEnvironment;readyMode.value=draft.readyMode;
  Object.assign(edits,draft.edits);Object.assign(names,draft.names);store.selectedIds=ids;rows.value=draft.rows.filter(row=>ids.includes(row.screenId));included.value=draft.includedIds.filter(id=>ids.includes(id));
  freshRead.value=false;invalidate();step.value=1;restoring=false;
}
function reset() { ++generation; rows.value = []; included.value = []; invalidate(); freshRead.value=false;busy.value = false; error.value = ""; step.value = 0; store.openedTaskId = ""; page.value=1;detailId.value="";for(const key of Object.keys(names))delete names[key];for(const field of fields)keepAddress(field.key);readyMode.value="keep";switchEnvironment.value=false; }
async function read() {
  if(busy.value)return;
  const adapter = useScreenAdapter();
  if (!adapter.readAppConfig) { error.value = "请在正式桌面程序中读取实际屏配置"; return; }
  const request = ++generation, project = store.projectId, ids = [...store.selectedIds];
  const continuing = step.value === 1, previousIncluded = new Set(included.value), previousRows = new Set(rows.value.map(row => row.screenId));
  page.value = 1; detailId.value = "";
  if (!continuing) { for (const field of fields) keepAddress(field.key); readyMode.value = "keep"; switchEnvironment.value = false; for (const id of Object.keys(names)) delete names[id]; }
  rows.value = []; included.value = []; invalidate(); freshRead.value=false;error.value = ""; busy.value = true; step.value = 1;
  try {
    for (const id of ids) {
      readingId.value = id;
      const result = await adapter.readAppConfig(project, [id]);
      if (generation !== request || project !== store.projectId) return;
      rows.value.push(...result);
      for (const row of result) { names[row.screenId] ??= { mode: "keep", value: "" }; if (row.config && (!continuing || !previousRows.has(row.screenId) || previousIncluded.has(row.screenId))) included.value.push(row.screenId); }
    }
    if (!continuing) {
      const current = summaries.value.current.value;
      if (current === "test" || current === "pre" || current === "prod") environment.value = current;
    }
    freshRead.value=true;
  } catch (cause) { if (generation === request) error.value = (cause as Error).message; }
  finally { if (generation === request) { busy.value = false; readingId.value = ""; } }
}
async function prepare() {
  if(busy.value)return;
  if(!freshRead.value){error.value='请先重新读取屏端当前配置，再检查修改内容';return;}
  if (!changedRows.value.length) { error.value = "请至少选择一项需要修改的配置"; return; }
  const adapter = useScreenAdapter();
  if (!adapter.preflightAppConfig) return;
  for (const row of changedRows.value) for (const value of Object.values(patches.value[row.screenId].set)) {
    if (typeof value === "string" && !value.trim()) { error.value = "OTA 地址不能为空。WebSocket、H5地址和设备名可删除内容来清空。"; return; }
  }
  const ids = changedRows.value.map(row => row.screenId);
  const input: ScreenOperationInput = { action: "app_config", targetIds: ids, appVersion: "", abi: "universal", reinstall: false, concurrency: concurrency.value,
    expectedTargets: Object.fromEntries(ids.map(id => [id, screenMaintenanceFingerprint(screensById.value.get(id)!)])) };
  const payload = JSON.parse(JSON.stringify(Object.fromEntries(ids.map(id => [id, patches.value[id]])))) as Record<string, ScreenAppConfigPatch>;
  const request = ++generation; busy.value = true; error.value = "";
  try {
    const result = await adapter.preflightAppConfig(store.projectId, input, payload);
    if (request !== generation) return;
    checks.value = result; prepared.value = input;
  } catch (cause) { if (request === generation) error.value = (cause as Error).message; }
  finally { if (request === generation) busy.value = false; }
}
async function submit() {
  if(busy.value)return;
  if (!prepared.value || !readyIds.value.length) return;
  const request = ++generation, project = store.projectId; busy.value = true; error.value = "";
  try {
    const taskId = await useScreenAdapter().execute(project, { ...prepared.value, targetIds: [...readyIds.value] });
    if (request !== generation || project !== store.projectId) return;
    store.openedTaskId = taskId; step.value = 2;
    await flushDraft();await useScreenAdapter().saveAppConfigDraft?.(project,null);
    if(request!==generation||project!==store.projectId)return;
    savedDraft.value=null;
    await store.refresh();
  } catch (cause) { if (request === generation) error.value = (cause as Error).message; }
  finally { if (request === generation) busy.value = false; }
}
function toggle(id: string, value: boolean) { included.value = value ? [...included.value, id] : included.value.filter(item => item !== id); }
function retry(ids: string[]) { store.selectedIds = ids; reset(); }
function restart(ids: string[]) { store.selectedIds = ids; store.operation = "restart"; }
</script>

<template>
  <div class="screen-config-operation">
    <n-alert v-if="error" type="warning" :show-icon="true">{{ error }}</n-alert>
    <div v-if="step === 0" class="screen-config-select">
      <screen-target-picker :disabled="busy" />
      <aside class="screen-config-guide">
        <h3>读取小新当前配置</h3><p>先读取实际屏，再选择需要修改的字段。可以同时选择不同尺寸的屏。</p>
        <p>支持运行环境、服务地址、网页启动检查和逐台自定义名称。需要配套版本的小新应用。</p>
        <div class="screen-callout">读取不修改配置。保存运行中的环境配置后，会按需重启小新，保留应用数据。</div>
        <n-button v-if="savedDraft" :disabled="busy" @click="restoreDraft">继续本机未提交修改（{{savedDraft.targetIds.length}} 台）</n-button>
        <n-button type="primary" :loading="busy" :disabled="!store.selectedIds.length || !useScreenAdapter().readAppConfig" @click="read">读取 {{ store.selectedIds.length }} 台配置</n-button>
      </aside>
    </div>
    <div v-else-if="step === 1" class="screen-config-editor">
      <p v-if="!freshRead && !busy" class="screen-config-notice">请重新读取屏端当前配置后再检查提交；本机草稿不会直接写入设备。</p>
      <fieldset :disabled="busy" class="screen-config-form" aria-label="批量配置表单">
        <div class="screen-config-field">
          <div class="screen-config-label"><label>选择环境</label></div>
          <div class="screen-config-control">
            <n-select :value="selected.length ? environment : null" size="small" :options="appEnvironments" :disabled="busy || !selected.length" :status="switchEnvironment ? 'warning' : undefined" placeholder="尚未读取" aria-label="选择环境" @update:value="changeEnvironment" />
          </div>
        </div>
        <div v-for="field in fields" :key="field.key" class="screen-config-field" :data-field="field.key">
          <div class="screen-config-label">
            <label>{{ field.label }}<span v-if="field.key === 'otaUrl'" class="screen-danger"> *</span></label>
          </div>
          <div class="screen-config-control">
            <n-input :value="addressValue(field.key)" size="small" :disabled="busy || !selected.length" :maxlength="4096" :status="edits[field.key].mode !== 'keep' ? 'warning' : undefined" :placeholder="addressPlaceholder(field.key)" :aria-label="field.label" @update:value="editAddress(field.key, $event)">
              <template #suffix>
                <n-button v-if="edits[field.key].mode !== 'keep'" text size="tiny" :disabled="busy" :aria-label="`撤销${field.label}修改`" @click="keepAddress(field.key)">撤销</n-button>
                <n-button v-else-if="summaries[field.key].mixed && !summaries[field.key].tied" text size="tiny" type="primary" :disabled="busy" :aria-label="`统一${field.label}`" title="将此值用于所选屏" @click="unifyAddress(field.key)">统一</n-button>
              </template>
            </n-input>
          </div>
        </div>
        <div class="screen-config-field">
          <div class="screen-config-label"><label>H5检测</label></div>
          <div class="screen-config-control">
            <div class="screen-config-check-control" :class="{ 'is-edited': readyMode !== 'keep' }">
              <n-checkbox :checked="readyValue" :indeterminate="readyMode === 'keep' && summaries.h5ReadyCheckEnabled.tied" :disabled="busy || !selected.length" size="small" aria-label="H5检测" @update:checked="editReady">检查H5地址</n-checkbox>
              <n-button v-if="readyMode !== 'keep'" text size="tiny" :disabled="busy" aria-label="撤销H5检测修改" @click="readyMode = 'keep'">撤销</n-button>
              <n-button v-else-if="summaries.h5ReadyCheckEnabled.mixed && !summaries.h5ReadyCheckEnabled.tied" text size="tiny" type="primary" :disabled="busy" title="将此值用于所选屏" @click="readyMode = readyValue ? 'on' : 'off'">统一</n-button>
            </div>
          </div>
        </div>
        <div class="screen-config-field screen-config-reread">
          <n-button size="small" :loading="Boolean(readingId)" :disabled="busy" @click="read">重新读取配置</n-button>
          <span class="screen-muted">{{ readingId ? `正在读取 ${name(readingId)} · ${rows.length}/${store.selectedIds.length} 台` : '未修改项各自保留；有差异时展示多数值。' }}</span>
        </div>
      </fieldset>
      <section class="screen-config-list" aria-label="智能屏配置检查列表" role="table" :aria-rowcount="listRows.length + 1">
        <div class="screen-config-list-header" role="rowgroup"><div class="screen-config-list-row" role="row">
          <span role="columnheader"><n-checkbox :checked="Boolean(selected.length) && selected.length === rows.filter(row => row.config).length" :indeterminate="selected.length > 0 && selected.length < rows.filter(row => row.config).length" :disabled="busy || !rows.some(row => row.config)" aria-label="选择全部已读取屏" @update:checked="included = $event ? rows.filter(row => row.config).map(row => row.screenId) : []" /></span>
          <span role="columnheader">屏名称</span><span role="columnheader">IP 地址</span><span role="columnheader">检查时间</span><span role="columnheader">检查状态</span><span role="columnheader">配置</span><span role="columnheader">操作</span>
        </div></div>
        <div ref="listBody" class="screen-config-list-body inx-scroll-area" role="rowgroup">
          <div v-for="row in visibleRows" :key="row.screenId" class="screen-config-list-row" role="row" :data-screen-id="row.screenId">
            <span role="cell"><n-checkbox :checked="included.includes(row.screenId)" :disabled="busy || !row.read?.config" :aria-label="`选择 ${name(row.screenId)}`" @update:checked="toggle(row.screenId, $event)" /></span>
            <span role="cell" class="screen-config-cell-text" :title="name(row.screenId)">{{ name(row.screenId) }}</span>
            <span role="cell" class="screen-config-cell-text">{{ ip(row.screenId) }}</span>
            <span role="cell" class="screen-config-time">{{ row.read ? appConfigReadTime(row.read.readAt) : '—' }}</span>
            <span role="cell" class="screen-config-status" :class="{ 'screen-warning': row.read && (!row.read.config || row.check?.state === 'blocked') }" :title="row.check?.reason || row.read?.message">
              <span>{{ readStatus(row) }}</span><small v-if="row.read && (!row.read.config || row.check?.state === 'blocked')">{{ row.check?.reason || row.read.message }}</small>
            </span>
            <span role="cell" class="screen-config-row-summary">
              <n-popover v-if="row.differences.length" trigger="hover" placement="left" :keep-alive-on-hover="true">
                <template #trigger><n-button text size="tiny" type="warning" :aria-label="`${name(row.screenId)}配置差异`" @click="detailId = row.screenId">{{ row.differences.length }} 项不同</n-button></template>
                <div class="screen-config-differences"><b>与当前所选屏的配置不同</b><div v-for="diff in row.differences" :key="diff.key"><strong>{{ diff.label }}</strong><div>本屏：{{ diff.value }}</div><div class="screen-muted">参考：{{ diff.reference }}</div></div></div>
              </n-popover>
              <span v-else class="screen-muted">{{ row.read?.config && selected.length ? '一致' : '—' }}</span>
              <n-button v-if="row.changes.length" text size="tiny" type="primary" @click="detailId = row.screenId">待修改 {{ row.changes.length }} 项</n-button>
            </span>
            <span role="cell"><n-button text size="tiny" type="primary" :aria-label="`查看 ${name(row.screenId)} 配置详情`" @click="detailId = row.screenId">详情</n-button></span>
          </div>
        </div>
      </section>
      <div class="screen-config-pagination"><span class="screen-muted">共 {{ listRows.length }} 台 · 已选 {{ selected.length }} 台<span v-if="rows.some(row => !row.config)" class="screen-warning"> · {{ rows.filter(row => !row.config).length }} 台读取失败</span></span><n-pagination v-model:page="page" v-model:page-size="pageSize" :item-count="listRows.length" :page-sizes="[20, 50, 100]" :page-slot="5" size="small" show-size-picker /></div>
      <div class="screen-config-submit"><span>本次修改 {{ changedRows.length }} 台 · 预计重启 {{ restartCount }} 台</span><label>并发台数<n-input-number v-model:value="concurrency" :min="1" :max="3" :disabled="busy" size="small" /></label><n-button v-if="!prepared" size="small" type="primary" :loading="busy && !readingId" :disabled="busy || !changedRows.length || !freshRead" @click="prepare">检查修改内容</n-button><n-button v-else size="small" type="success" :loading="busy" :disabled="busy || !readyIds.length" @click="submit">确认修改 {{ readyIds.length }} 台</n-button></div>
    </div>
    <div v-else class="screen-config-results"><screen-task-results v-if="task" :task="task" @retry="retry" @restart="restart" /><p v-else>正在读取任务结果…</p><footer class="screen-config-result-footer"><n-button size="small" :disabled="busy || !task || Boolean(running)" @click="reset">新的配置操作</n-button></footer></div>
    <n-modal v-model:show="detailShow" preset="card" :title="`${name(detailId)} · 配置详情`" class="screen-config-detail-modal" :style="{ width: 'min(760px, 94vw)' }" :bordered="false" :auto-focus="false">
      <div v-if="detail" class="screen-config-detail-body inx-scroll-area">
        <p class="screen-muted">{{ ip(detail.screenId) }} · {{ detail.read ? appConfigReadTime(detail.read.readAt) : '尚未读取' }}<span v-if="detail.read?.capabilities"> · 小新{{ detail.read.capabilities.appVersionName }}-{{ detail.read.capabilities.appVersionCode }}</span></p>
        <template v-if="detail.read?.config">
          <div class="screen-config-name-field"><label>设备名<small>仅修改本屏的小新名称</small></label><n-input :value="nameValue(detail.read)" size="small" :maxlength="128" :disabled="busy || !included.includes(detail.screenId)" :status="names[detail.screenId]?.mode !== 'keep' ? 'warning' : undefined" :aria-label="`${name(detail.screenId)}设备名`" @update:value="editName(detail.read!, $event)" /><n-button v-if="names[detail.screenId]?.mode !== 'keep'" text size="tiny" :disabled="busy" @click="names[detail.screenId] = { mode: 'keep', value: '' }">撤销</n-button></div>
          <div class="screen-config-current"><div>当前运行：{{ appConfigDisplay(detail.read.config.environments.current, true) }} · 查看{{ appConfigDisplay(environment, true) }}配置</div><div v-for="field in fields" :key="field.key">{{ field.label }}：{{ detail.read.config.environments[environment][field.key] || '未设置' }}</div><div>H5检测：{{ appConfigDisplay(detail.read.config.environments[environment].h5ReadyCheckEnabled) }}</div><div>语音使用值（{{ appConfigSources[detail.read.config.environments[environment].wsSource] }}）：{{ detail.read.config.environments[environment].effectiveWsUrl || '未设置' }}</div><div>网页使用值（{{ appConfigSources[detail.read.config.environments[environment].h5Source] }}）：{{ detail.read.config.environments[environment].effectiveH5Url }}</div><div>服务端语音地址：{{ detail.read.config.environments[environment].otaWsUrl || '未下发' }}</div><div>服务端网页地址：{{ detail.read.config.environments[environment].otaH5Url || '未下发' }}</div></div>
          <table v-if="detail.changes.length" class="screen-table screen-small-table"><thead><tr><th>修改内容</th><th>当前值</th><th>修改为</th></tr></thead><tbody><tr v-for="change in detail.changes" :key="change.key"><td>{{ change.label }}</td><td>{{ change.before }}</td><td>{{ change.after }}</td></tr></tbody></table>
          <p class="screen-muted">{{ detail.changes.length ? (appConfigRestartRequired(detail.read.config, patches[detail.screenId]) ? '保存成功后重启小新一次，再读取配置确认。' : '本次无需重启，保存后读取配置确认。') : '当前未修改，或所选值已经相同。' }}</p>
          <div v-if="detail.changes.some(change => change.key.endsWith('Url'))" class="screen-config-current"><div>保存后语音使用（{{ appConfigSources[appConfigEffectiveAfter(detail.read.config, patches[detail.screenId], environment).ws.source] }}）：{{ appConfigEffectiveAfter(detail.read.config, patches[detail.screenId], environment).ws.value }}</div><div>保存后网页使用（{{ appConfigSources[appConfigEffectiveAfter(detail.read.config, patches[detail.screenId], environment).h5.source] }}）：{{ appConfigEffectiveAfter(detail.read.config, patches[detail.screenId], environment).h5.value }}</div></div>
        </template>
        <template v-else><p class="screen-warning">{{ detail.read?.message || '尚未读取配置' }}</p><n-button v-if="detail.read" size="small" :disabled="busy" @click="restart([detail.screenId]); detailId = ''">启动 / 重启小新</n-button></template>
        <p v-if="detail.check" :class="detail.check.state === 'blocked' ? 'screen-warning' : 'screen-muted'">{{ detail.check.reason }}</p>
      </div>
    </n-modal>
  </div>
</template>

<style scoped>
.screen-config-operation{display:flex;flex-direction:column;gap:12px;min-height:0;flex:1;overflow:hidden;padding:12px}
.screen-config-select{display:grid;grid-template-columns:minmax(0,1fr) 300px;gap:14px;min-height:0;flex:1}
.screen-config-guide{display:flex;flex-direction:column;gap:14px;padding:16px;border:1px solid var(--inx-color-border);border-radius:8px}
.screen-config-guide p{margin:0;color:var(--inx-color-text-secondary);line-height:1.7}
.screen-config-editor{display:flex;flex-direction:column;flex:1;overflow:hidden;min-height:0;gap:8px}
.screen-config-form{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));column-gap:24px;row-gap:10px;border:0;padding:0 0 4px;margin:0;min-width:0;flex:none}
.screen-config-field{display:grid;grid-template-columns:116px minmax(0,1fr);gap:8px;align-items:center;min-width:0;min-height:28px}
.screen-config-label{min-width:0;line-height:20px}
.screen-config-label label{font-size:var(--inx-font-size-base);font-weight:400;white-space:nowrap}
.screen-config-control{min-width:0}
.screen-config-control :deep(.n-input__suffix .n-button){font-size:var(--inx-font-size-table)}
.screen-config-check-control{display:flex;align-items:center;gap:10px;min-height:28px;padding:0 6px;border:1px solid transparent;border-radius:5px}
.screen-config-check-control>.n-button{margin-left:auto;font-size:var(--inx-font-size-table)}
.screen-config-check-control.is-edited{border-color:var(--inx-color-warning)}
.screen-config-reread{display:flex;align-items:center;gap:10px}.screen-config-reread>span{min-width:0;font-size:var(--inx-font-size-table);line-height:17px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.screen-config-notice{margin:0;padding:5px 8px;color:var(--inx-color-text-secondary);background:var(--inx-color-surface-subtle);font-size:var(--inx-font-size-table);flex:none}
.screen-config-list{display:flex;flex-direction:column;flex:1;min-height:0;overflow:hidden;border:1px solid var(--inx-color-border);border-radius:6px;--config-columns:26px minmax(100px,1.1fr) 118px 144px minmax(100px,1fr) 96px 42px}
.screen-config-list-header{flex:none;padding-right:4px;background:var(--inx-color-table-header);color:var(--inx-color-text-secondary);border-bottom:1px solid var(--inx-color-border)}
.screen-config-list-row{display:grid;grid-template-columns:var(--config-columns);column-gap:8px;align-items:center;min-height:36px;padding:4px 10px;border-bottom:1px solid var(--inx-color-border);box-sizing:border-box}
.screen-config-list-header .screen-config-list-row{min-height:30px;border:0;font-size:11px}
.screen-config-list-body{min-height:0;flex:1;overflow-y:auto;overflow-x:hidden}
.screen-config-list-body .screen-config-list-row:hover{background:var(--inx-color-hover)}
.screen-config-cell-text{min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.screen-config-time{font-size:11px;color:var(--inx-color-text-secondary);white-space:nowrap}
.screen-config-status{display:flex;flex-direction:column;min-width:0;line-height:18px}
.screen-config-status small{font-size:11px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.screen-config-row-summary{display:flex;flex-direction:column;align-items:flex-start;line-height:18px;font-size:11px}
.screen-config-differences{max-width:460px;font-size:12px;overflow-wrap:anywhere}.screen-config-differences>div{margin-top:8px}.screen-config-differences strong{font-weight:500}
.screen-config-pagination{display:flex;align-items:center;justify-content:space-between;gap:10px;flex:none;font-size:11px;min-height:28px}
.screen-config-submit{display:flex;align-items:center;gap:16px;justify-content:flex-end;flex:none;padding:8px 0 0;border-top:1px solid var(--inx-color-border)}
.screen-config-submit>span{margin-right:auto}.screen-config-submit label{display:flex;align-items:center;gap:8px}.screen-config-submit .n-input-number{width:78px}
.screen-config-current{margin:12px 0;color:var(--inx-color-text-secondary);font-size:12px;overflow-wrap:anywhere}.screen-config-current div{margin-top:4px}
.screen-config-detail-body{max-height:65vh;overflow:auto;padding-right:4px}.screen-config-detail-body td{white-space:normal;overflow-wrap:anywhere}
.screen-config-name-field{display:flex;align-items:center;gap:10px}.screen-config-name-field label{min-width:130px;font-size:var(--inx-font-size-base);font-weight:400}.screen-config-name-field small{display:block;font-size:var(--inx-font-size-table);color:var(--inx-color-text-secondary)}
.screen-config-results{display:flex;flex:1;flex-direction:column;gap:8px;min-height:0;overflow:hidden}
.screen-config-result-footer{display:flex;flex:none;justify-content:flex-end;padding-top:8px;border-top:1px solid var(--inx-color-border)}
@media(max-width:1100px){.screen-config-form{column-gap:16px}.screen-config-field{grid-template-columns:110px minmax(0,1fr);gap:6px}.screen-config-list{--config-columns:24px minmax(90px,1fr) 104px 132px minmax(80px,1fr) 80px 36px}.screen-config-list-row{column-gap:6px;padding-inline:8px}}
@media(max-width:1000px){.screen-config-select{grid-template-columns:minmax(0,1fr) 250px}}
</style>
