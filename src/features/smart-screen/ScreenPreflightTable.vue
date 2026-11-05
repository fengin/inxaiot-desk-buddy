<script setup lang="ts">
import { computed } from "vue";
import { NCheckbox } from "naive-ui";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";
import type { ScreenPreflightItem } from "@/shared/model/screen";
import ScreenTimeCheck from "./ScreenTimeCheck.vue";

const props = defineProps<{ rows: ScreenPreflightItem[]; disabled?: boolean; compact?: boolean; time?: boolean; sleepOff?: boolean }>();
const included = defineModel<string[]>("included", { required: true });
const columns = computed(() => props.compact ? [
  { key: "select", title: "选择", width: "40px" },
  { key: "name", title: "智能屏", width: "44%" },
  { key: "result", title: "检查结果" }
] : [
  { key: "select", title: "选择", width: "40px" },
  { key: "name", title: "智能屏", width: "21%" },
  { key: "ip", title: "IP 地址", width: "126px" },
  { key: "state", title: "检查状态", width: "80px" },
  ...(props.time ? [{ key: "time", title: "时间偏差", width: "130px" }] : []),
  { key: "result", title: "检查说明" }
]);
const stateText = (row: ScreenPreflightItem) => row.state === "ready" ? props.sleepOff ? "待关闭" : "检查通过" : row.state === "blocked" ? "不可执行" : props.sleepOff ? "已关闭" : "无需执行";
function stateClass(row: ScreenPreflightItem) {
  if (row.state === "blocked") return "screen-danger";
  if (props.sleepOff) return row.state === "ready" ? "screen-warning" : "screen-success";
  return row.state === "ready" ? "screen-success" : "screen-muted";
}
function toggle(row: ScreenPreflightItem, checked: boolean) {
  if (props.disabled || row.state !== "ready") return;
  included.value = checked ? [...new Set([...included.value, row.screenId])] : included.value.filter(id => id !== row.screenId);
}
</script>

<template>
  <compact-operation-table :columns="columns" label="逐台执行检查" class="screen-preflight-table" :class="{ 'screen-preflight-compact': compact }">
    <tr v-for="row in rows" :key="row.screenId" class="screen-check-row" :data-screen-id="row.screenId">
      <td><n-checkbox :checked="included.includes(row.screenId)" :disabled="disabled || row.state !== 'ready'" :aria-label="`执行 ${row.name}`" @update:checked="toggle(row, $event)" /></td>
      <td><span class="screen-preflight-name" :title="row.name">{{ row.name }}</span><small v-if="compact" :title="row.ip">{{ row.ip }}</small></td>
      <td v-if="!compact" class="screen-network">{{ row.ip }}</td>
      <td v-if="!compact" :class="stateClass(row)">{{ stateText(row) }}</td>
      <td v-if="!compact && time"><screen-time-check v-if="row.observation" :observation="row.observation" /><span v-else class="screen-muted">未读取</span></td>
      <td :title="row.reason" :class="compact ? stateClass(row) : undefined"><span class="screen-preflight-reason">{{ row.reason }}</span></td>
    </tr>
  </compact-operation-table>
</template>

<style scoped>
.screen-preflight-name, .screen-preflight-reason { display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.screen-preflight-name { font-weight: 500; }
.screen-preflight-table small { display: block; margin-top: 1px; color: var(--inx-color-text-secondary); font-size: 10px; }
.screen-preflight-table :deep(.screen-check-row) { display: table-row; }
.screen-preflight-table :deep(.screen-check-row td) { vertical-align: middle; }
.screen-preflight-compact :deep(th), .screen-preflight-compact :deep(td) { padding-inline: 5px; }
</style>
