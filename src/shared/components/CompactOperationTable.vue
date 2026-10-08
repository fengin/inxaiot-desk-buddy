<script setup lang="ts">
import { ref, watch } from "vue";

const props = defineProps<{
  columns: { key: string; title: string; width?: string }[];
  label?: string;
  resetKey?: string | number;
}>();
const body = ref<HTMLElement>();
watch(() => props.resetKey, () => { if (body.value) body.value.scrollTop = 0; });
</script>

<template>
  <div class="compact-operation-table">
    <div class="compact-operation-table-header">
      <table :aria-label="`${label || '设备列表'}表头`">
        <colgroup><col v-for="column in columns" :key="column.key" :style="{ width: column.width }" /></colgroup>
        <thead><tr><th v-for="column in columns" :key="column.key" scope="col"><slot :name="`header-${column.key}`">{{ column.title }}</slot></th></tr></thead>
      </table>
    </div>
    <div ref="body" class="compact-operation-table-body inx-scroll-area">
      <table :aria-label="label || '设备列表'">
        <colgroup><col v-for="column in columns" :key="column.key" :style="{ width: column.width }" /></colgroup>
        <tbody><slot /></tbody>
      </table>
    </div>
  </div>
</template>

<style scoped>
.compact-operation-table{display:flex;flex:1;flex-direction:column;min-width:0;min-height:0;overflow:hidden;border:1px solid var(--inx-color-border);border-radius:6px}
.compact-operation-table-header{flex:none;padding-right:4px;background:var(--inx-color-table-header)}
.compact-operation-table-body{flex:1;min-height:0;overflow-y:auto;overflow-x:hidden}
table{width:100%;min-width:0;table-layout:fixed;border-spacing:0;border-collapse:separate;text-align:left;font-size:var(--inx-font-size-table)}
th{height:32px;padding:5px 8px;color:var(--inx-color-text-secondary);font-size:10px;font-weight:600;line-height:20px;border-bottom:1px solid var(--inx-color-border)}
.compact-operation-table :deep(td){height:var(--inx-table-row-height);padding:5px 8px;line-height:20px;vertical-align:middle;border-bottom:1px solid var(--inx-color-border);overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.compact-operation-table :deep(tbody tr:last-child td){border-bottom:0}
.compact-operation-table :deep(tbody tr:hover){background:var(--inx-color-hover)}
.compact-operation-table :deep(small){display:block;margin-top:1px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:10px;line-height:15px;color:var(--inx-color-text-secondary)}
</style>
