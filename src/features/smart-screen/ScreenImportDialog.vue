<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NDrawer, NDrawerContent, NInput, NPagination, useMessage } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import type { ProjectSpaceNode } from "@/shared/model/projectSpace";
import type { ScreenSnapshot, SmartScreen } from "@/shared/model/screen";
import { inventorySpacePaths, inventorySpacePathsCsv, parseScreenInventory, revalidateScreenInventory } from "./importInventory";
import type { ScreenImportRow } from "./importInventory";

const show = defineModel<boolean>("show", { default: false });
const store = useSmartScreensStore();
const message = useMessage();
const fileInput = ref<HTMLInputElement>();
const text = ref("");
const rows = ref<ScreenImportRow[]>([]);
const selected = ref<number[]>([]);
const error = ref("");
const stage = ref<"input" | "preview">("input");
const previewColumns=[{key:'selected',title:'选择',width:'36px'},{key:'name',title:'屏名称',width:'12%'},{key:'ip',title:'IP 地址',width:'104px'},{key:'size',title:'尺寸',width:'52px'},{key:'original',title:'原始空间',width:'13%'},{key:'space',title:'空间修正',width:'20%'},{key:'matched',title:'匹配路径',width:'14%'},{key:'message',title:'错误 / 提示'}];
const spaces = ref<ProjectSpaceNode[]>([]), existing = ref<SmartScreen[]>([]);
const spacesAvailable = ref(true), reading = ref(false), checking = ref(false), saving = ref(false);
const batchSpaceId = ref<string | null>(null);
const previewPage = ref(1), previewPageSize = 20;
let request = 0;
const busy = computed(() => reading.value || checking.value || saving.value);
const validCount = computed(() => rows.value.filter((row) => !row.error).length);
const selectedRows = computed(() => rows.value.filter((row) => selected.value.includes(row.line)));
const selectedErrors = computed(() => selectedRows.value.filter((row) => row.error).length);
const allSelected = computed(() => rows.value.length > 0 && selected.value.length === rows.value.length);
const paths = computed(() => inventorySpacePaths(spaces.value, spacesAvailable.value));
const previewRows = computed(() => rows.value.slice((previewPage.value - 1) * previewPageSize, previewPage.value * previewPageSize));
function directoryKey() { return JSON.stringify({ available: spacesAvailable.value, spaces: [...spaces.value].sort((a, b) => a.id.localeCompare(b.id)) }); }
function setContext(snapshot: ScreenSnapshot) {
  spaces.value = snapshot.spaces.map((space) => ({ ...space }));
  spacesAvailable.value = snapshot.spacesAvailable !== false;
  existing.value = snapshot.screens.map((screen) => ({ ...screen }));
}
function reset() {
  ++request; text.value = ""; rows.value = []; error.value = ""; selected.value = []; stage.value = "input";
  reading.value = checking.value = saving.value = false; batchSpaceId.value = null; previewPage.value = 1;
}
watch(show, (open) => { reset(); if (open) setContext(store.snapshot); }, { immediate: true, flush: "sync" });
watch(() => store.projectId, () => { show.value = false; reset(); }, { flush: "sync" });
onBeforeUnmount(() => { ++request; });
function sample() {
  if (busy.value) return;
  const path = (paths.value.at(-1) ?? "").replace(/"/g, '""');
  text.value = `名称,IP,尺寸,MAC,空间路径,安装位置\n会议室新增屏,192.0.2.180,10,,"${path}",会议室入口\n待定空间屏,192.0.2.181,4,,,现场待定位\n待核对平台屏,192.0.2.35,10,,"${path}",东区\n错误地址示例,192.0.2.999,10,,,请修正后导入`;
  error.value = "";
}
async function fileChanged(event: Event) {
  const input = event.target as HTMLInputElement;
  const file = input.files?.[0]; input.value = "";
  if (!file || busy.value || !show.value) return;
  if (file.size > 2_000_000) { error.value = "请选择 2 MB 以内的 CSV 文件"; return; }
  const generation = ++request, projectId = store.projectId;
  const current = () => generation === request && projectId === store.projectId && show.value;
  reading.value = true; error.value = "";
  try { const content = await file.text(); if (current()) text.value = content; }
  catch { if (current()) error.value = "读取文件失败，请确认文件可访问并使用 UTF-8 CSV"; }
  finally { if (current()) reading.value = false; }
}
async function preview() {
  if (busy.value || !show.value) return;
  const projectId = store.projectId, generation = ++request;
  const current = () => generation === request && projectId === store.projectId && show.value;
  checking.value = true;
  try {
    const snapshot = await useScreenAdapter().load(projectId);
    if (!current()) return;
    setContext(snapshot);
    rows.value = parseScreenInventory(text.value, existing.value, spaces.value, spacesAvailable.value);
    if (!rows.value.length) throw new Error("请先选择 CSV 文件或粘贴屏清单");
    selected.value = rows.value.map((row) => row.line);
    error.value = ""; stage.value = "preview"; previewPage.value = 1;
  } catch (cause) { if (current()) error.value = (cause as Error).message; }
  finally { if (current()) checking.value = false; }
}
function select(line: number, checked: boolean) { selected.value = checked ? [...new Set([...selected.value, line])] : selected.value.filter((value) => value !== line); }
function assignSpace(lines: number[], spaceId: string | null) {
  if (busy.value) return;
  rows.value = revalidateScreenInventory(rows.value.map((row) => lines.includes(row.line) ? { ...row, spaceOverrideId: spaceId } : row), existing.value, spaces.value, spacesAvailable.value);
  error.value = "";
}
function spaceLabel(row: ScreenImportRow) {
  if (row.spaceStatus === "empty") return "待定空间";
  if (row.spaceStatus === "matched") return row.spaceOverrideId === undefined ? "路径已匹配" : "人工已修正";
  return ({ missing: "未找到空间", ambiguous: "路径不唯一", unavailable: "目录不可用" })[row.spaceStatus];
}
function downloadPaths() {
  if (busy.value || !paths.value.length) return;
  const url = URL.createObjectURL(new Blob([inventorySpacePathsCsv(paths.value)], { type: "text/csv;charset=utf-8" }));
  const link = document.createElement("a"); link.href = url; link.download = "智能屏-项目空间路径清单.csv";
  document.body.appendChild(link); link.click(); link.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
async function save() {
  if (busy.value || !show.value || !selected.value.length || selectedErrors.value) return;
  const projectId = store.projectId, generation = ++request, previousDirectory = directoryKey();
  const current = () => generation === request && projectId === store.projectId && show.value;
  saving.value = true; error.value = "";
  try {
    const snapshot = await useScreenAdapter().load(projectId);
    if (!current()) return;
    setContext(snapshot);
    rows.value = revalidateScreenInventory(rows.value, existing.value, spaces.value, spacesAvailable.value);
    if (directoryKey() !== previousDirectory) { error.value = "项目空间目录已变化，已重新核对所有行。请检查匹配路径和错误说明后再次提交。"; return; }
    if (selectedErrors.value) { error.value = "选中记录存在错误，未提交任何记录。请修正或明确取消选择这些行。"; return; }
    const inputs = selectedRows.value.map((row) => ({ ...row.input }));
    const omitted = rows.value.length - inputs.length;
    await useScreenAdapter().importLocal(projectId, inputs);
    if (!current()) return;
    message.success(`已导入 ${inputs.length} 台智能屏；${omitted} 条未提交`); show.value = false;
  } catch (cause) { if (current()) error.value = (cause as Error).message; }
  finally { if (current()) saving.value = false; }
}
</script>

<template>
  <n-drawer v-model:show="show" width="min(1120px, 94vw)" class="screen-dialog">
    <n-drawer-content title="导入智能屏清单" closable :body-content-style="stage==='preview'?{display:'flex',flexDirection:'column',height:'100%',minHeight:'0',overflow:'hidden',gap:'8px'}:undefined">
      <div class="screen-callout">只加入本机管理，不创建平台屏记录。相同平台 IP 会在导入后提示核对。</div>
      <div class="screen-import-directory"><n-button size="small" :disabled="busy || !paths.length" data-testid="screen-import-space-export" @click="downloadPaths">下载项目空间路径清单</n-button><span>{{ spacesAvailable ? `${paths.length} 条可唯一匹配的空间路径` : '空间目录不可用；非空路径不能提交' }}</span></div>
      <n-alert v-if="error" type="error" class="screen-dialog-alert">{{ error }}</n-alert>
      <template v-if="stage === 'input'">
        <div class="screen-inline-actions"><n-button :disabled="busy" :loading="reading" @click="fileInput?.click()">选择 CSV 文件</n-button><n-button text type="primary" :disabled="busy" @click="sample">填入演示清单</n-button></div>
        <input ref="fileInput" type="file" accept=".csv,text/csv" hidden :disabled="busy" data-testid="screen-import-file" @change="fileChanged" />
        <p class="screen-muted">UTF-8 CSV：名称、IP、尺寸、MAC、空间路径、安装位置。空间路径填写完整层级，例如“A座/2F/东区/会议室”，可停在任一真实节点；空路径明确归入待定空间。MAC 可为空。</p>
        <n-input v-model:value="text" type="textarea" :rows="12" :disabled="busy" placeholder="名称,IP,尺寸,MAC,空间路径,安装位置" aria-label="屏清单内容" @update:value="error = ''" />
      </template>
      <template v-else>
        <div class="screen-result-summary" data-testid="screen-import-summary"><strong>共 {{ rows.length }} 条 · {{ validCount }} 条校验通过</strong><span>已选 {{ selected.length }} 条（{{ selected.length - selectedErrors }} 条可提交，{{ selectedErrors }} 条需修正）</span><span>不提交 {{ rows.length - selected.length }} 条</span></div>
        <div class="screen-import-batch"><n-checkbox :checked="allSelected" :indeterminate="selected.length > 0 && !allSelected" :disabled="busy" @update:checked="selected = $event ? rows.map(row => row.line) : []">全选记录</n-checkbox><project-space-select v-model="batchSpaceId" :spaces="spaces" :disabled="busy || !spacesAvailable" placeholder="为选中行统一选择空间" aria-label="批量修正导入空间" size="small" /><n-button size="small" :disabled="busy || !selected.length || !batchSpaceId" @click="assignSpace(selected, batchSpaceId)">应用到选中 {{ selected.length }} 行</n-button><n-button size="small" :disabled="busy || !selected.length" @click="assignSpace(selected, null)">设为待定空间</n-button></div>
        <p class="screen-muted">默认勾选全部记录；选中错误行会阻止本次提交。可逐行或批量修正空间，也可明确取消错误行。下载清单不包含歧义路径，重名节点请通过树选择确认。</p>
        <compact-operation-table :columns="previewColumns" label="导入清单预览" :reset-key="previewPage">
          <tr v-for="row in previewRows" :key="row.line" :data-import-line="row.line"><td><n-checkbox :checked="selected.includes(row.line)" :disabled="busy" :aria-label="`导入第 ${row.line} 行`" @update:checked="(checked: boolean) => select(row.line, checked)" /></td><td :title="row.input.name || '未命名'">{{ row.input.name || '未命名' }}<small>文件第 {{ row.line }} 行</small></td><td :title="row.input.ip">{{row.input.ip}}</td><td>{{ row.input.size === 'unknown' ? '待确认' : `${row.input.size} 寸` }}</td><td :title="row.originalSpacePath || '空路径（待定空间）'">{{ row.originalSpacePath || '空路径（待定空间）' }}</td><td><div class="screen-import-space-editor"><project-space-select :model-value="row.input.spaceId" :spaces="spaces" :disabled="busy || !spacesAvailable" :placeholder="row.spaceStatus === 'empty' ? '待定空间' : '选择空间修正'" :aria-label="`修正第 ${row.line} 行空间`" size="small" @update:model-value="assignSpace([row.line], $event)" /><n-button v-if="row.spaceStatus !== 'empty'" text size="tiny" :disabled="busy" @click="assignSpace([row.line], null)">设为待定空间</n-button></div></td><td :title="`${row.matchedSpacePath || (row.spaceStatus === 'empty' ? '待定空间' : '尚未匹配')}\n空间 ID：${row.input.spaceId||'无'}`">{{ row.matchedSpacePath || (row.spaceStatus === 'empty' ? '待定空间' : '尚未匹配') }}<small :class="{'screen-danger':row.spaceError}">{{spaceLabel(row)}}</small></td><td :title="[row.error || '可以导入',row.warning].filter(Boolean).join('\n')"><span :class="row.error ? 'screen-danger' : 'screen-success'">{{ row.error || '可以导入' }}</span><small v-if="row.warning" class="screen-warning">{{ row.warning }}</small></td></tr>
        </compact-operation-table>
        <div v-if="rows.length > previewPageSize" class="screen-import-pages"><span>每页预览 {{ previewPageSize }} 条；选择与错误统计包含全部 {{ rows.length }} 条</span><n-pagination v-model:page="previewPage" :page-size="previewPageSize" :item-count="rows.length" size="small" /></div>
      </template>
      <template #footer><div class="screen-dialog-footer"><n-button :disabled="busy" @click="stage === 'preview' ? stage = 'input' : show = false">{{ stage === 'preview' ? '返回修改' : '取消' }}</n-button><n-button v-if="stage === 'input'" type="primary" :disabled="busy" :loading="checking" @click="preview">检查清单</n-button><n-button v-else type="primary" :disabled="busy || !selected.length || Boolean(selectedErrors)" :loading="saving" data-testid="screen-import-submit" @click="save">导入选中 {{ selected.length }} 台</n-button></div></template>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.screen-import-directory, .screen-import-batch { display: flex; flex:none; flex-wrap: wrap; align-items: center; gap: 8px; margin: 0; }
.screen-import-directory > span { color: var(--inx-color-text-secondary); font-size: 11px; }
.screen-import-batch > .n-tree-select { flex: 1; min-width: 210px; }
.screen-import-space-editor{display:flex;align-items:center;gap:4px}.screen-import-space-editor>.n-tree-select{flex:1;min-width:0}
.screen-result-summary{display:flex;flex:none;flex-wrap:wrap;gap:3px 12px;font-size:12px}.screen-result-summary strong{font-size:12px;font-weight:500}
.screen-callout,.screen-muted{flex:none;margin:0;padding-block:4px;font-size:11px;line-height:18px}
.screen-import-pages { display: flex;flex:none; align-items: center; justify-content: space-between; gap: 8px; margin:0; color: var(--inx-color-text-secondary); font-size: 11px; }
</style>
