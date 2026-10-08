<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NDrawer, NDrawerContent, NPagination, NTag, useMessage } from "naive-ui";
import { ArrowRight, CheckCircle2 } from "lucide-vue-next";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import type { ScreenStatusChange, ScreenStatusResult } from "@/shared/model/screen";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";

const show = defineModel<boolean>("show", { default: false });
const emit = defineEmits<{ recheck: [ids: string[]] }>();
const store = useSmartScreensStore();
const message = useMessage();
const preview = ref<(ScreenStatusChange & { name: string; checkedAt: string | null })[]>([]);
const selected = ref<string[]>([]);
const results = ref<ScreenStatusResult[]>([]);
const saving = ref(false);
const error = ref("");
const page = ref(1), pageSize = ref(50);
const rowCount = computed(() => results.value.length || preview.value.length);
const visiblePreview = computed(() => preview.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
const visibleResults = computed(() => results.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
const previewById = computed(() => new Map(preview.value.map((row) => [row.id, row])));
const previewColumns = [
  { key: "select", title: "", width: "36px" }, { key: "name", title: "智能屏" },
  { key: "ip", title: "IP 地址", width: "116px" }, { key: "checkedAt", title: "检查时间", width: "142px" },
  { key: "change", title: "本机检查 → 平台状态", width: "294px" }
];
const resultColumns = [
  { key: "name", title: "智能屏", width: "25%" }, { key: "ip", title: "IP 地址", width: "116px" },
  { key: "state", title: "状态", width: "78px" }, { key: "message", title: "处理结果" }
];
watch([preview, results, pageSize], () => { page.value = 1; });
watch(rowCount, (count) => { page.value = Math.min(page.value, Math.max(1, Math.ceil(count / pageSize.value))); });
let request = 0;
let previewProjectId = "";
const allSelected = computed(() => preview.value.length > 0 && preview.value.every((row) => selected.value.includes(row.id)));
function clear() {
  ++request; previewProjectId = "";
  preview.value = []; selected.value = []; results.value = []; error.value = ""; saving.value = false;
}
function refresh() {
  clear(); previewProjectId = store.projectId;
  preview.value = store.statusDifferences.map((screen) => ({ id: screen.id, name: screen.name, ip: screen.ip, expected: screen.platformStatus, next: screen.ping!, revision: screen.revision, checkedAt: screen.checkedAt }));
  selected.value = preview.value.map((row) => row.id);
}
watch(show, (open) => { if (open) refresh(); else clear(); }, { flush: "sync" });
watch(() => store.projectId, () => { show.value = false; clear(); }, { flush: "sync" });
onBeforeUnmount(() => { ++request; });
function recheck() {
  if (saving.value || previewProjectId !== store.projectId) return;
  const ids = preview.value.filter((row) => selected.value.includes(row.id)).map((row) => row.id);
  if (!ids.length) return;
  show.value = false;
  emit("recheck", ids);
}
async function cover() {
  if (saving.value || !show.value || !store.platformAvailable || previewProjectId !== store.projectId) return;
  const changes = preview.value.filter((row) => selected.value.includes(row.id));
  if (!changes.length) return;
  const projectId = store.projectId;
  const generation = ++request;
  const current = () => generation === request && store.projectId === projectId && show.value;
  saving.value = true; error.value = "";
  try {
    const nextResults = await useScreenAdapter().coverStatus(projectId, changes);
    if (!current()) return;
    results.value = nextResults;
    const successful = results.value.filter((row) => row.ok).length;
    message.info(`${store.snapshot.mode === 'real' ? '' : '模拟'}覆盖完成：${successful} 项成功，${results.value.length - successful} 项未更新`);
  } catch (cause) { if (current()) error.value = (cause as Error).message; }
  finally { if (current()) saving.value = false; }
}
</script>

<template>
  <n-drawer v-model:show="show" width="min(920px, 96vw)" class="screen-dialog screen-status-dialog">
    <n-drawer-content title="本机检查与平台状态差异" closable body-style="overflow:hidden" body-content-style="height:100%;box-sizing:border-box;display:flex;flex-direction:column;gap:8px;min-height:0;overflow:hidden;padding:12px 16px">
      <n-alert type="info" :show-icon="true"><template v-if="store.snapshot.mode === 'real'">确认后，以勾选屏的有效 IP 检查结果更新平台在线状态。</template><template v-else>本原型使用模拟数据。确认后，以所选屏的本机 IP 检查值更新模拟平台状态；正式功能将仅更新所选屏的状态字段。</template></n-alert>
      <p class="screen-muted screen-status-hint">平台与当前电脑的网络视角可能不同。请核对检查时间；需要获取最新结果时，可重新检查本机状态。平台后续检查仍可再次更新状态。</p>
      <n-alert v-if="error" type="error" class="screen-dialog-alert">{{ error }}</n-alert>
      <template v-if="results.length">
        <h3 class="screen-section-title">本次确认结果</h3>
        <div class="screen-callout">未更新项不会自动重试。刷新差异后，需重新选择并确认。</div>
        <CompactOperationTable class="screen-status-results" :columns="resultColumns" label="状态同步结果" :reset-key="`result-${page}-${pageSize}`">
          <tr v-for="result in visibleResults" :key="result.id" :data-screen-id="result.id">
            <td :title="result.name">{{ result.name }}</td><td :title="previewById.get(result.id)?.ip">{{ previewById.get(result.id)?.ip || '—' }}</td>
            <td><n-tag size="small" :type="result.ok ? 'success' : 'warning'" :bordered="false">{{ result.ok ? '已更新' : '未更新' }}</n-tag></td>
            <td :title="result.message">{{ result.message }}</td>
          </tr>
        </CompactOperationTable>
      </template>
      <template v-else-if="preview.length">
        <div class="screen-selection-bar"><n-checkbox size="small" :checked="allSelected" :indeterminate="selected.length > 0 && !allSelected" :disabled="saving" @update:checked="selected = $event ? preview.map((row) => row.id) : []">选择全部差异</n-checkbox><span>已选 {{ selected.length }} / {{ preview.length }} 台</span></div>
        <CompactOperationTable :columns="previewColumns" label="本机检查与平台状态差异" :reset-key="`preview-${page}-${pageSize}`">
          <tr v-for="row in visiblePreview" :key="row.id" class="screen-status-row" :data-screen-id="row.id">
            <td><n-checkbox size="small" :aria-label="`覆盖 ${row.name}`" :checked="selected.includes(row.id)" :disabled="saving" @update:checked="selected = $event ? [...selected, row.id] : selected.filter((id) => id !== row.id)" /></td>
            <td :title="row.name">{{ row.name }}</td><td :title="row.ip">{{ row.ip }}</td>
            <td class="screen-status-time" :title="row.checkedAt ? formatDisplayDateTime(row.checkedAt) : '样例检查'">{{ row.checkedAt ? formatDisplayDateTime(row.checkedAt) : '样例检查' }}</td>
            <td><div class="screen-status-transition" data-testid="screen-status-transition">
            <n-tag size="small" class="screen-status-source" :type="row.next === 'online' ? 'success' : 'warning'" :bordered="false">本机{{ row.next === 'online' ? '可达' : '不可达' }}</n-tag>
            <ArrowRight :size="15" aria-label="以本机检查值更新平台" />
            <n-tag size="small" class="screen-status-target" :type="row.next === 'online' ? 'success' : 'error'" :bordered="false">平台将{{ row.next === 'online' ? '在线' : '离线' }}</n-tag>
            <small class="screen-status-original">平台原值：{{ row.expected === 'online' ? '在线' : '离线' }}</small>
            </div></td>
          </tr>
        </CompactOperationTable>
      </template>
      <div v-else class="screen-empty"><CheckCircle2 :size="30" /><strong>没有需要覆盖的状态差异</strong><span>未注册屏的检查只保留在本机，不写入平台。</span></div>
      <div v-if="rowCount" class="screen-status-pagination"><span class="screen-muted">共 {{ rowCount }} 台</span><n-pagination v-model:page="page" v-model:page-size="pageSize" :item-count="rowCount" :page-sizes="[20,50,100]" :page-slot="5" size="small" show-size-picker /></div>
      <template #footer><div class="screen-dialog-footer"><n-button size="small" @click="show = false">{{ results.length ? '关闭' : '保留平台状态' }}</n-button><n-button v-if="results.length" size="small" type="primary" @click="refresh">刷新差异</n-button><template v-else><n-button size="small" :disabled="!selected.length || saving" @click="recheck">重新检查本机状态</n-button><n-button size="small" type="warning" :disabled="!selected.length || !store.platformAvailable" :loading="saving" @click="cover">确认覆盖选中 {{ selected.length }} 台</n-button></template></div></template>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.screen-selection-bar { padding: 4px 0; margin: 0; flex: none; }
.screen-status-hint { margin: 0; font-size: 11px; line-height: 1.5; flex: none; }
.screen-section-title { margin: 0; flex: none; }
.screen-callout, .screen-dialog-alert { margin: 0; flex: none; }
tr.screen-status-row { display: table-row; padding: 0; border: 0; }
.screen-status-transition { display: flex; align-items: center; gap: 6px; white-space: nowrap; }
.screen-status-transition > svg, .screen-status-source, .screen-status-target { flex: none; }
.screen-status-transition .screen-status-original { display: inline; margin: 0; overflow: visible; color: var(--inx-color-text-secondary); font-size: 10px; line-height: 1.4; }
.screen-status-time { font-size: 11px; }
.screen-status-pagination { display: flex; align-items: center; justify-content: space-between; gap: 8px; flex: none; min-height: 28px; font-size: 11px; }
</style>
