<script setup lang="ts">
defineProps<{ total: number; selected: number; concurrency: number }>();
</script>

<template>
  <footer class="screen-operation-footer" data-testid="screen-operation-footer">
    <div class="screen-operation-footer-layout">
      <div class="screen-operation-footer-summary">
        <div class="screen-operation-footer-group" data-summary-group="devices">
          <span class="screen-operation-footer-label">设备</span>
          <div class="screen-operation-footer-values"><span>共 {{ total }} 台</span><span aria-hidden="true"> · </span><span class="screen-operation-footer-selected">已选 {{ selected }} 台</span><slot name="device-status" /></div>
        </div>
        <div class="screen-operation-footer-group" data-summary-group="changes">
          <span class="screen-operation-footer-label">修改</span>
          <div class="screen-operation-footer-values"><slot name="changes" /></div>
        </div>
        <div class="screen-operation-footer-group" data-summary-group="concurrency">
          <span class="screen-operation-footer-label">并发</span>
          <div class="screen-operation-footer-values"><span>{{ concurrency }} 台</span></div>
        </div>
      </div>
      <div class="screen-operation-footer-controls">
        <div v-if="$slots.pagination" class="screen-operation-footer-pagination"><slot name="pagination" /></div>
        <div class="screen-operation-footer-action"><slot name="action" /></div>
      </div>
    </div>
    <slot name="status" />
  </footer>
</template>

<style scoped>
.screen-operation-footer{display:flex;flex:none;flex-direction:column;align-items:stretch;gap:4px;min-width:0;padding-top:8px;border-top:1px solid var(--inx-color-border);font-size:var(--inx-font-size-table);color:var(--inx-color-text-secondary)}
.screen-operation-footer-layout{display:grid;grid-template-columns:minmax(0,1fr) max-content;align-items:center;gap:12px;min-width:0;width:100%;box-sizing:border-box}
.screen-operation-footer-summary{display:flex;align-items:center;flex-wrap:wrap;gap:4px 16px;min-width:0}
.screen-operation-footer-group{display:flex;flex:none;align-items:baseline;gap:6px;min-width:0;max-width:100%}
.screen-operation-footer-label{flex:none;font-size:11px;line-height:18px;color:var(--inx-color-text-tertiary)}
.screen-operation-footer-values{display:flex;align-items:center;flex-wrap:wrap;gap:0 4px;min-width:0;line-height:18px}
.screen-operation-footer-values :deep(>span){white-space:nowrap}
.screen-operation-footer-selected{color:var(--inx-color-info)}
.screen-operation-footer-controls{display:flex;align-items:center;justify-content:flex-end;flex-wrap:wrap;gap:8px;max-width:460px;min-width:0}
.screen-operation-footer-pagination{min-width:0}
.screen-operation-footer-action{flex:none}
@media(max-width:1120px){.screen-operation-footer-layout{gap:10px}.screen-operation-footer-summary{column-gap:10px}.screen-operation-footer-controls{max-width:440px}}
</style>
