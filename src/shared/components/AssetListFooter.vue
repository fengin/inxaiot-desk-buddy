<script setup lang="ts">
import { NButton } from "naive-ui";
import { ArrowUpRight } from "lucide-vue-next";

defineProps<{ selectedCount?: number; disabled?: boolean; disabledReason?: string; batchTestId?: string }>();
defineEmits<{ batch: [] }>();
const buttonTheme = {
  textColorPrimary: "#fff", textColorHoverPrimary: "#fff", textColorPressedPrimary: "#fff",
  textColorFocusPrimary: "#fff", textColorDisabledPrimary: "#fff"
};
</script>

<template>
  <footer class="asset-list-footer">
    <div class="asset-footer-summary">
      <span class="asset-footer-count"><slot name="summary" /></span>
      <template v-if="selectedCount !== undefined">
        <span class="asset-footer-divider" aria-hidden="true"></span>
        <span class="asset-footer-selection" :class="{ active: selectedCount > 0 }">已选 <b>{{ selectedCount }}</b> 台</span>
        <n-button class="asset-footer-batch" size="tiny" :theme-overrides="buttonTheme" :type="selectedCount > 0 ? 'primary' : 'default'" :disabled="!selectedCount || disabled" :title="!selectedCount ? '请先选择设备' : disabled ? disabledReason : '进入批量操作'" :data-testid="batchTestId" @click="$emit('batch')">批量操作<ArrowUpRight :size="14" aria-hidden="true" /></n-button>
      </template>
    </div>
    <div class="asset-footer-pagination">
      <slot name="pagination" />
      <span v-if="$slots.meta" class="asset-footer-meta"><slot name="meta" /></span>
    </div>
  </footer>
</template>

<style scoped>
.asset-list-footer { display:flex; flex:none; flex-wrap:wrap; align-items:center; justify-content:space-between; gap:6px 12px; min-width:0; padding:6px 10px; border-top:1px solid var(--inx-color-border); background:var(--inx-color-surface); color:var(--inx-color-text-secondary); font-size:var(--inx-font-size-table); line-height:22px; }
.asset-footer-summary { display:flex; flex-wrap:wrap; align-items:center; gap:8px; min-width:0; }
.asset-footer-count,.asset-footer-selection { white-space:nowrap; }
.asset-footer-divider { width:1px; height:16px; background:var(--inx-color-border-strong); flex:none; }
.asset-footer-selection { display:inline-flex; align-items:center; gap:4px; }
.asset-footer-selection.active b { color:var(--inx-color-info); }
.asset-footer-batch { font-size:var(--inx-font-size-table); }
.asset-footer-batch :deep(.n-button__content) { gap:4px; }
.asset-footer-pagination { display:flex; align-items:center; justify-content:flex-end; flex-wrap:wrap; gap:6px 12px; min-width:0; margin-left:auto; }
.asset-footer-meta { color:var(--inx-color-text-tertiary); white-space:nowrap; }
</style>
