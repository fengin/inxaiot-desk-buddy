<script setup lang="ts">
defineProps<{ columns: string[]; widths: string; label: string }>();
</script>

<template>
  <div class="aio-operation-table" role="table" :aria-label="label" :style="{ '--aio-table-columns': widths }">
    <div class="aio-operation-table__head" role="rowgroup">
      <div class="aio-operation-table__row" role="row">
        <span v-for="column in columns" :key="column" role="columnheader">{{ column }}</span>
      </div>
    </div>
    <div class="aio-operation-table__body inx-scroll-area" role="rowgroup" tabindex="0" :aria-label="label + '列表'">
      <slot />
    </div>
  </div>
</template>

<style scoped>
.aio-operation-table { display: flex; flex: 1; min-height: 0; min-width: 0; flex-direction: column; overflow: hidden; border: 1px solid var(--inx-color-border); border-radius: var(--inx-radius-sm); }
.aio-operation-table__head { flex: none; overflow: hidden; padding-right: 4px; background: var(--inx-color-table-header); border-bottom: 1px solid var(--inx-color-border); }
.aio-operation-table__row, .aio-operation-table__body :deep(.aio-table-row) { display: grid; grid-template-columns: var(--aio-table-columns); align-items: center; gap: 8px; padding: 4px 10px; box-sizing: border-box; }
.aio-operation-table__row { min-height: 30px; font-size: 11px; color: var(--inx-color-text-secondary); }
.aio-operation-table__body { flex: 1; min-height: 0; overflow-y: auto; overflow-x: hidden; }
.aio-operation-table__body :deep(.aio-table-row) { min-height: var(--inx-table-row-height, 36px); border-bottom: 1px solid var(--inx-color-border); }
.aio-operation-table__body :deep(.aio-table-row:last-child) { border-bottom: 0; }
.aio-operation-table__body :deep(.aio-table-row:hover) { background: var(--inx-color-hover); }
.aio-operation-table__body :deep(.aio-table-row > *) { min-width: 0; }
.aio-operation-table__body :deep(.aio-table-row > span), .aio-operation-table__body :deep(.aio-table-row > strong), .aio-operation-table__body :deep(.aio-table-row > time) { overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.aio-operation-table__body :deep(strong) { font-weight: 500; }
.aio-operation-table__body :deep(.aio-table-identity) { display: flex; align-items: center; gap: 6px; }
.aio-operation-table__body :deep(.aio-table-identity strong) { min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.aio-operation-table__body :deep(.aio-table-address) { font-family: var(--inx-font-mono); color: var(--inx-color-text-secondary); font-size: 12px; }
.aio-operation-table__body :deep(.aio-table-message) { font-size: 12px; }
.aio-operation-table__body :deep(button.aio-table-row) { width: 100%; border-top: 0; border-inline: 0; background: transparent; color: inherit; font: inherit; text-align: left; cursor: pointer; }
.aio-operation-table__body :deep(.active) { background: var(--inx-color-info-soft); }
.aio-operation-table__body :deep(.done) { background: color-mix(in srgb, var(--inx-color-success-soft) 55%, var(--inx-color-surface)); }
.aio-operation-table__body :deep(.result-not-success .aio-table-identity > svg) { color: var(--inx-color-warning); }
.aio-operation-table__body :deep(.aio-table-identity > svg) { flex: none; color: var(--inx-color-operation); }
</style>
