<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { NButton, NCheckbox, NPopover, NTag } from "naive-ui";
import { ArrowRight, CircleAlert, Pencil } from "lucide-vue-next";
import type { ScreenRegistrationMacConfirmation, ScreenRegistrationPreview, ScreenRegistrationPreviewItem, ScreenPlatformField } from "@/shared/model/screenRegistration";
import type { ScreenSpaceNode } from "@/shared/model/screenSpace";
import { projectSpacePath } from "@/shared/model/projectSpace";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import "./registration.css";

const props = defineProps<{ preview: ScreenRegistrationPreview; spaces: ScreenSpaceNode[]; disabled?: boolean }>();
const included = defineModel<string[]>("included", { required: true });
const macConfirmations = defineModel<Record<string, ScreenRegistrationMacConfirmation>>("macConfirmations", { required: true });
const spaceConfirmations = defineModel<string[]>("spaceConfirmations", { required: true });
const emit = defineEmits<{ edit: [screenId: string]; merge: [] }>();
const focusedId = ref("");
const columns = [{ key: "select", title: "选择", width: "40px" }, { key: "name", title: "智能屏" }, { key: "ip", title: "IP 地址", width: "108px" }, { key: "state", title: "检查结果", width: "76px" }];
const focused = computed(() => props.preview.items.find((item) => item.screenId === focusedId.value) ?? props.preview.items[0]);
const fields: { field: ScreenPlatformField; label: string }[] = [
  { field: "name", label: "名称" }, { field: "ip", label: "IP 地址" }, { field: "mac", label: "MAC 地址" },
  { field: "size", label: "尺寸" }, { field: "spaceId", label: "所在空间" }, { field: "location", label: "详细位置" }
];
const sourceLabels: Record<ScreenRegistrationPreviewItem["macSource"], string> = {
  collected: "本次采集", unchanged: "平台当前记录", history: "历史记录", empty: "暂未获取", conflict: "身份冲突"
};
const summary = computed(() => ({
  create: props.preview.items.filter((item) => item.state === "ready" && item.mode === "create").length,
  update: props.preview.items.filter((item) => item.state === "ready" && item.mode === "update").length,
  skip: props.preview.items.filter((item) => item.state === "skip").length,
  blocked: props.preview.items.filter((item) => item.state === "blocked").length
}));
const selected = computed(() => Boolean(focused.value && included.value.includes(focused.value.screenId)));
const spaceChanged = computed(() => focused.value?.needsSpaceConfirmation);
watch(() => props.preview.id, () => { focusedId.value = props.preview.items[0]?.screenId ?? ""; }, { immediate: true });

function outcome(item: ScreenRegistrationPreviewItem) {
  if (item.state === "blocked") return "需要核对";
  if (item.state === "skip") return "无需更新";
  return item.mode === "create" ? "新增注册" : "更新资料";
}
function toggle(item: ScreenRegistrationPreviewItem, checked: boolean) {
  if (props.disabled || item.state !== "ready") return;
  included.value = checked ? [...new Set([...included.value, item.screenId])] : included.value.filter((id) => id !== item.screenId);
}
function display(field: ScreenPlatformField, side: "before" | "after") {
  const item = focused.value;
  if (!item) return "—";
  if (!item[side]) return "未注册";
  const diff = item.diffs.find((value) => value.field === field);
  if (diff) return diff[side] || "未填写";
  const value = item[side]![field];
  if (field === "spaceId") return value ? projectSpacePath(props.spaces, value) || `空间节点 ${value}` : "待定空间";
  if (field === "size") return value === "unknown" ? "待确认" : `${value} 寸`;
  return value || "未填写";
}
function confirmMac(checked: boolean) {
  const item = focused.value;
  if (!item?.requiredMacConfirmation || props.disabled || !selected.value) return;
  const next = { ...macConfirmations.value };
  if (checked) next[item.screenId] = item.requiredMacConfirmation;
  else delete next[item.screenId];
  macConfirmations.value = next;
}
function confirmSpace(checked: boolean) {
  if (!focused.value || props.disabled || !selected.value) return;
  const id = focused.value.screenId;
  spaceConfirmations.value = checked ? [...new Set([...spaceConfirmations.value, id])] : spaceConfirmations.value.filter((item) => item !== id);
}
</script>

<template>
  <section class="screen-registration-preview" aria-label="平台注册与更新预览">
    <div class="screen-registration-summary">
      <span>新增注册 <b class="metric-info">{{ summary.create }}</b></span>
      <span>更新资料 <b class="metric-operation">{{ summary.update }}</b></span>
      <span>无需更新 <b>{{ summary.skip }}</b></span>
      <span>需要核对 <b class="screen-warning">{{ summary.blocked }}</b></span>
      <span class="screen-spacer"></span><span>本次提交 <b class="metric-operation">{{ included.length }}</b> 台</span>
    </div>
    <p v-if="preview.mode !== 'real'" class="screen-registration-boundary" data-testid="screen-registration-prototype">当前为交互原型：MAC 采集、平台注册、更新和回读均为模拟，不连接设备或写入真实平台。</p>
    <p v-else class="screen-registration-boundary">请核对逐台差异，确认后更新平台资料。</p>
    <div class="screen-registration-preview-layout">
      <aside class="screen-registration-targets" aria-label="逐屏检查结果">
        <compact-operation-table :columns="columns" label="平台注册逐屏检查" :reset-key="preview.id">
          <tr v-for="item in preview.items" :key="item.screenId" class="screen-registration-target" :class="{ active: focused?.screenId === item.screenId }" :data-screen-id="item.screenId" @click="focusedId = item.screenId">
            <td @click.stop><n-checkbox :checked="included.includes(item.screenId)" :disabled="disabled || item.state !== 'ready'" :aria-label="`提交 ${item.after.name}`" @update:checked="toggle(item, $event)" /></td>
            <td><button type="button" :aria-pressed="focused?.screenId === item.screenId" :title="item.after.name" @click="focusedId = item.screenId">{{ item.after.name || '未命名屏' }}</button></td>
            <td :title="item.after.ip" class="screen-network">{{ item.after.ip }}</td>
            <td :title="item.reason" :class="item.state === 'blocked' ? 'screen-warning' : item.state === 'skip' ? 'screen-muted' : 'screen-success'">{{ outcome(item) }}</td>
          </tr>
        </compact-operation-table>
      </aside>
      <div v-if="focused" class="screen-registration-detail" :data-screen-id="focused.screenId">
        <div class="screen-registration-detail-heading">
          <strong :title="focused.after.name">{{ focused.after.name || '未命名屏' }}</strong>
          <n-tag size="small" :bordered="false" :type="focused.state === 'blocked' ? 'warning' : focused.state === 'skip' ? 'default' : focused.mode === 'create' ? 'info' : 'success'">{{ outcome(focused) }}</n-tag>
          <span class="screen-spacer"></span>
          <n-button size="tiny" text :disabled="disabled" @click="emit('edit', focused.screenId)"><template #icon><Pencil :size="13" /></template>编辑资料</n-button>
        </div>
        <p class="screen-registration-reason" :title="focused.reason" :class="{ 'screen-warning': focused.state === 'blocked' }">{{ focused.reason }}</p>
        <div class="screen-registration-compact-diff">
          <n-popover trigger="click" placement="bottom-end" class="screen-registration-comparison-popover">
            <template #trigger><n-button size="small" secondary type="primary" aria-label="查看逐屏资料差异" data-action-owner="registration-comparison-popover">{{ focused.mode === 'create' ? '查看注册资料' : `查看资料差异（${focused.diffs.length} 项）` }}</n-button></template>
            <table class="screen-registration-comparison"><thead><tr><th>资料</th><th>平台当前</th><th aria-label="更新方向"></th><th>本次拟更新</th></tr></thead>
              <tbody><tr v-for="field in fields" :key="field.field" :class="{ changed: focused.diffs.some((diff) => diff.field === field.field) }">
                <th>{{ field.label }}</th><td :title="display(field.field, 'before')">{{ display(field.field, 'before') }}</td><td><ArrowRight :size="13" /></td><td :title="display(field.field, 'after')">{{ display(field.field, 'after') }}</td>
              </tr></tbody>
            </table>
          </n-popover>
        </div>
        <table class="screen-registration-comparison screen-registration-inline-comparison"><thead><tr><th>资料</th><th>平台当前</th><th aria-label="更新方向"></th><th>本次拟更新</th></tr></thead>
          <tbody><tr v-for="field in fields" :key="field.field" :class="{ changed: focused.diffs.some((diff) => diff.field === field.field) }">
            <th>{{ field.label }}</th><td :title="display(field.field, 'before')">{{ display(field.field, 'before') }}</td>
            <td><ArrowRight :size="13" aria-label="平台当前到本次拟更新" /></td><td :title="display(field.field, 'after')">{{ display(field.field, 'after') }}</td>
          </tr></tbody>
        </table>
        <div class="screen-registration-mac"><span>MAC 来源：<b>{{ sourceLabels[focused.macSource] }}</b></span><span class="screen-registration-checked-at" data-testid="screen-registration-checked-at">检查时间：{{ formatDisplayDateTime(preview.createdAt) }}</span><p :title="focused.macMessage">{{ focused.macMessage }}</p></div>
        <n-checkbox v-if="focused.requiredMacConfirmation" :key="`mac-${focused.screenId}`" :checked="macConfirmations[focused.screenId] === focused.requiredMacConfirmation" :disabled="disabled || !selected || focused.state !== 'ready'" class="screen-registration-confirmation" :aria-label="`确认 ${focused.after.name} ${focused.requiredMacConfirmation === 'existing' ? '沿用历史 MAC' : 'MAC 暂空'}`" @update:checked="confirmMac">
          {{ focused.requiredMacConfirmation === 'existing' ? '已核实设备身份，本次沿用历史 MAC；该值不是本次采集结果。' : '本次允许 MAC 暂空登记，后续获取后再补充；不据此判断设备身份。' }}
        </n-checkbox>
        <n-checkbox v-if="spaceChanged" :key="`space-${focused.screenId}`" :checked="spaceConfirmations.includes(focused.screenId)" :disabled="disabled || !selected || focused.state !== 'ready'" class="screen-registration-confirmation" :aria-label="`确认 ${focused.after.name} 所属空间变更`" @update:checked="confirmSpace">已确认所属空间变更；这会改变屏在平台中的业务归属。</n-checkbox>
        <div v-if="focused.duplicateIds.length" class="screen-registration-duplicate"><CircleAlert :size="16" /><span>存在重复候选，请先核对关联后重新检查。</span><n-button size="tiny" type="warning" text :disabled="disabled" @click="emit('merge')">核对疑似重复</n-button></div>
        <p v-if="focused.state === 'ready' && !selected" class="screen-registration-excluded">已取消本屏，本次不会提交。</p>
      </div>
    </div>
  </section>
</template>
