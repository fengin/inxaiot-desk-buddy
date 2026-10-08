<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NDrawer, NDrawerContent, NPagination, NTag } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenVersionPreview } from "@/shared/model/screenMaintenance";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";

const show = defineModel<boolean>("show", { default: false });
const props = defineProps<{ ids: string[] }>();
const store = useSmartScreensStore();
const preview = ref<ScreenVersionPreview>();
const selected = ref<string[]>([]), taskId = ref(""), error = ref("");
const loading = ref(false), saving = ref(false);
const result = computed(() => store.snapshot.tasks.find((task) => task.id === taskId.value));
const readyIds = computed(() => preview.value?.items.filter((item) => item.state === "ready").map((item) => item.screenId) ?? []);
const page = ref(1), pageSize = ref(50);
const rowCount = computed(() => result.value ? result.value.targets.length : preview.value?.items.length ?? 0);
const visiblePreview = computed(() => preview.value?.items.slice((page.value - 1) * pageSize.value, page.value * pageSize.value) ?? []);
const visibleResults = computed(() => result.value?.targets.slice((page.value - 1) * pageSize.value, page.value * pageSize.value) ?? []);
const previewColumns = [
  { key: "select", title: "", width: "36px" }, { key: "name", title: "智能屏", width: "16%" },
  { key: "ip", title: "本次连接 IP", width: "116px" }, { key: "platform", title: "平台版本", width: "84px" },
  { key: "device", title: "读取版本", width: "88px" }, { key: "checkedAt", title: "检查时间", width: "138px" },
  { key: "reason", title: "检查结果" }
];
const resultColumns = [
  { key: "name", title: "智能屏", width: "25%" }, { key: "ip", title: "本次连接 IP", width: "116px" },
  { key: "state", title: "状态", width: "78px" }, { key: "message", title: "处理结果" }
];
watch([preview, taskId, pageSize], () => { page.value = 1; });
watch(rowCount, (count) => { page.value = Math.min(page.value, Math.max(1, Math.ceil(count / pageSize.value))); });
let request = 0;
function clear() { ++request; preview.value = undefined; selected.value = []; taskId.value = ""; error.value = ""; loading.value = saving.value = false; }
async function read() {
  if (saving.value) return;
  clear(); loading.value = true;
  const generation = request, projectId = store.projectId;
  try {
    const data = await useScreenAdapter().previewVersionSync(projectId, [...props.ids]);
    if (generation !== request || projectId !== store.projectId || !show.value) return;
    if (data.projectId !== projectId) throw new Error("版本检查所属项目已变化，请重新打开");
    preview.value = data; selected.value = data.items.filter((item) => item.state === "ready").map((item) => item.screenId);
  } catch (cause) { if (generation === request && projectId === store.projectId) error.value = (cause as Error).message; }
  finally { if (generation === request && projectId === store.projectId) loading.value = false; }
}
async function submit() {
  if (saving.value || !show.value || !preview.value || !selected.value.length || !store.platformAvailable) return;
  const generation = ++request, projectId = store.projectId;
  const previewId = preview.value.id;
  saving.value = true; error.value = "";
  try {
    const id = await useScreenAdapter().submitVersionSync(projectId, previewId, [...selected.value]);
    if (generation !== request || projectId !== store.projectId || !show.value) return;
    taskId.value = id; await store.refresh();
  } catch (cause) { if (generation === request && projectId === store.projectId) error.value = (cause as Error).message; }
  finally { if (generation === request && projectId === store.projectId) saving.value = false; }
}
async function verify() {
  if (!result.value || saving.value) return;
  const projectId = store.projectId, generation = ++request, id = result.value.id;
  saving.value = true;
  try { await useScreenAdapter().verify(projectId, id); if (generation === request && projectId === store.projectId) await store.refresh(); }
  catch (cause) { if (generation === request && projectId === store.projectId) error.value = (cause as Error).message; }
  finally { if (generation === request && projectId === store.projectId) saving.value = false; }
}
watch([show, () => props.ids.join("|")], ([open]) => { if (open) void read(); else clear(); }, { flush: "sync", immediate: true });
watch(() => store.projectId, () => { show.value = false; clear(); }, { flush: "sync" });
onBeforeUnmount(() => { ++request; });
</script>

<template>
  <n-drawer v-model:show="show" width="min(920px, 96vw)" class="screen-dialog screen-version-dialog">
    <n-drawer-content title="核对应用版本并同步平台" closable body-style="overflow:hidden" body-content-style="height:100%;box-sizing:border-box;display:flex;flex-direction:column;gap:8px;min-height:0;overflow:hidden;padding:12px 16px">
      <n-alert type="info" :show-icon="true">重新读取所选屏的小新版本，确认后只同步平台版本信息，不重新安装应用。<template v-if="store.snapshot.mode !== 'real'">当前为模拟采集与写入。</template></n-alert>
      <p class="screen-muted screen-version-hint">按每台屏的当前确认地址读取。平台未注册或暂不可用时，读取结果保留在本机；恢复后重新核对再同步。</p>
      <n-alert v-if="error" type="warning">{{ error }}</n-alert>
      <div v-if="loading" class="screen-callout">正在读取设备应用版本…</div>
      <template v-else-if="result">
        <CompactOperationTable :columns="resultColumns" label="应用版本同步结果" :reset-key="`result-${page}-${pageSize}`">
          <tr v-for="target in visibleResults" :key="target.screenId" :data-version-screen="target.screenId">
            <td :title="target.name">{{ target.name }}</td><td :title="target.ip">{{ target.ip }}</td>
            <td><n-tag size="small" :type="target.state === 'succeeded' ? 'success' : 'warning'" :bordered="false">{{ target.state === 'succeeded' ? '已同步' : target.state === 'needs_review' ? '待核实' : '未同步' }}</n-tag></td>
            <td :title="target.message">{{ target.message }}</td>
          </tr>
          <tr v-if="!result.targets.length"><td :colspan="resultColumns.length" class="screen-muted">暂无设备结果</td></tr>
        </CompactOperationTable>
      </template>
      <template v-else-if="preview">
        <div class="screen-selection-bar"><n-checkbox size="small" :disabled="saving || !readyIds.length" :checked="readyIds.length > 0 && selected.length === readyIds.length" :indeterminate="selected.length > 0 && selected.length < readyIds.length" @update:checked="selected = $event ? [...readyIds] : []">选择全部版本差异</n-checkbox><span>已选 {{ selected.length }} / {{ preview.items.length }} 台检查</span></div>
        <CompactOperationTable :columns="previewColumns" label="应用版本检查" :reset-key="`preview-${page}-${pageSize}`">
          <tr v-for="row in visiblePreview" :key="row.screenId" :data-version-screen="row.screenId">
            <td><n-checkbox size="small" :aria-label="`同步 ${row.name} 版本`" :disabled="saving || row.state !== 'ready'" :checked="selected.includes(row.screenId)" @update:checked="selected = $event ? [...new Set([...selected, row.screenId])] : selected.filter(id => id !== row.screenId)" /></td>
            <td :title="row.name">{{ row.name }}</td><td :title="row.ip">{{ row.ip }}</td>
            <td :title="row.platformVersion || '未记录'">{{ row.platformVersion || '未记录' }}</td><td :title="row.deviceVersion || '未取得有效版本'">{{ row.deviceVersion || '未取得有效版本' }}</td>
            <td class="screen-version-time" :title="formatDisplayDateTime(row.checkedAt)">{{ formatDisplayDateTime(row.checkedAt) }}</td>
            <td :class="row.state === 'ready' || row.state === 'skip' ? 'screen-success' : 'screen-warning'" :title="row.reason">{{ row.reason }}</td>
          </tr>
          <tr v-if="!preview.items.length"><td :colspan="previewColumns.length" class="screen-muted">暂无设备检查结果</td></tr>
        </CompactOperationTable>
      </template>
      <div v-if="!loading && (result || preview)" class="screen-version-pagination"><span class="screen-muted">共 {{ rowCount }} 台</span><n-pagination v-model:page="page" v-model:page-size="pageSize" :item-count="rowCount" :page-sizes="[20,50,100]" :page-slot="5" size="small" show-size-picker /></div>
      <template #footer><div class="screen-dialog-footer"><n-button size="small" @click="show = false">关闭</n-button><n-button size="small" :disabled="saving || loading || result?.state === 'needs_review'" @click="read">重新读取版本</n-button><n-button v-if="result?.state === 'needs_review'" size="small" type="warning" :loading="saving" @click="verify">核实已有同步结果</n-button><n-button v-else-if="!result" size="small" type="primary" :disabled="loading || !selected.length || !store.platformAvailable" :loading="saving" data-testid="screen-version-submit" @click="submit">确认同步 {{ selected.length }} 台版本</n-button></div></template>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.screen-version-hint { margin: 0; font-size: 11px; line-height: 1.5; flex: none; }
.screen-selection-bar { padding: 4px 0; margin: 0; flex: none; }
.screen-callout { margin: 0; flex: none; }
.screen-version-time { font-size: 11px; }
.screen-version-pagination { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex: none; min-height: 28px; font-size: 11px; }
</style>
