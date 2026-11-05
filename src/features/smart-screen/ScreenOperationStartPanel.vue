<script setup lang="ts">
import { computed } from "vue";
import { NInputNumber } from "naive-ui";
import type { SmartScreen } from "@/shared/model/screen";

const props = withDefaults(defineProps<{
  title: string;
  concurrency: number;
  disabled?: boolean;
  screens: readonly Pick<SmartScreen, "size">[];
  showConcurrency?: boolean;
}>(), { disabled: false, showConcurrency: true });
const emit = defineEmits<{ "update:concurrency": [value: number] }>();
const sizes = computed(() => [
  { size: "4", label: "4寸" }, { size: "10", label: "10寸" }, { size: "unknown", label: "尺寸待确认" }
].map(item => ({ ...item, count: props.screens.filter(screen => screen.size === item.size).length })).filter(item => item.count));
function updateConcurrency(value: number | null) {
  emit("update:concurrency", Math.min(3, Math.max(1, Math.trunc(value ?? 1))));
}
</script>

<template>
  <aside class="stage-settings screen-parameter-panel screen-operation-start-panel">
    <h3>{{ title }}</h3>
    <div class="screen-parameter-content screen-parameter-form-scroll inx-scroll-area">
      <div class="screen-operation-start-description"><slot name="description" /></div>
      <slot />
      <div v-if="showConcurrency" class="form-grid compact execution-tuning screen-execution-tuning">
        <label title="本批读取与执行的并发台数，其他屏按顺序等待"><span>并发台数</span><n-input-number :value="concurrency" :min="1" :max="3" :precision="0" :disabled="disabled" size="small" aria-label="并发台数" @update:value="updateConcurrency" /></label>
      </div>
      <div class="execution-summary screen-operation-start-selection" aria-label="已选屏类型和数量" data-testid="screen-operation-selection-summary">
        <span>已选 <strong class="metric-number metric-info" data-testid="screen-summary-target-count">{{ screens.length }}</strong> 台</span>
        <span v-for="item in sizes" :key="item.size">{{ item.label }} <strong class="metric-number metric-info">{{ item.count }}</strong> 台</span>
        <span v-if="!screens.length" class="screen-muted">请选择智能屏</span>
      </div>
      <div v-if="$slots.status" class="screen-operation-start-status" aria-live="polite"><slot name="status" /></div>
    </div>
    <div class="screen-parameter-action"><slot name="action" /></div>
  </aside>
</template>

<style scoped>
.screen-operation-start-panel{display:flex;flex-direction:column;min-width:0;min-height:0;overflow:hidden}
.screen-operation-start-description{flex:none;min-width:0}
.screen-operation-start-description :deep(p){margin:0}
.screen-operation-start-status{display:flex;flex-direction:column;gap:6px;min-width:0;font-size:var(--inx-font-size-table)}
.screen-operation-start-panel>.screen-parameter-content{flex:1;min-height:0;overflow-y:auto;overflow-x:hidden}
.screen-operation-start-panel>.screen-parameter-action{flex:none;margin-top:auto}
.screen-operation-start-selection strong{font-weight:600}
</style>
