<script setup lang="ts">
import { NButton } from "naive-ui";
import { CheckCircle2, CircleAlert, ChevronRight } from "lucide-vue-next";
import type { DeploymentPreflightCheck } from "@/shared/model/deploymentWorkflow";
import { preflightGroupPassed, type PreflightGroup } from "./preflightPresentation";

defineProps<{ group: PreflightGroup; expanded: boolean }>();
const emit = defineEmits<{ toggle: []; remediate: [check: DeploymentPreflightCheck] }>();
const statusLabels = { passed: "通过", warning: "有提示", failed: "需处理", pending: "未检查" };
</script>

<template>
  <section class="preflight-group" :class="{ 'preflight-common': group.key === 'common' }" :data-testid="'preflight-group-' + group.key">
    <h3 class="preflight-group-heading">
      <button type="button" class="preflight-group-toggle" :aria-expanded="expanded" :aria-controls="'preflight-details-' + group.key" @click="emit('toggle')">
        <CheckCircle2 v-if="preflightGroupPassed(group)" class="preflight-group-status-icon success" :size="16" aria-hidden="true" data-state="passed" />
        <CircleAlert v-else class="preflight-group-status-icon error" :size="16" aria-hidden="true" data-state="failed" />
        <span class="preflight-group-title" :title="group.title">{{ group.title }}</span>
        <span class="preflight-group-address" :title="group.subtitle">{{ group.subtitle || '—' }}</span>
        <span class="preflight-group-checks" :title="group.items.map(item => `${item.label}：${statusLabels[item.status]}`).join('；')">{{ group.items.map(item => `${item.label}：${statusLabels[item.status]}`).join(' · ') }}</span>
        <span class="preflight-group-status" :class="preflightGroupPassed(group) ? 'success' : 'error'">{{ preflightGroupPassed(group) ? '通过' : '不通过' }}<small v-if="group.items.some(item => item.issues.some(issue => issue.status === 'warning'))" class="preflight-group-note"> · 有提示</small></span>
        <ChevronRight :size="14" class="preflight-group-chevron" :class="{ expanded }" aria-hidden="true" />
      </button>
    </h3>
    <div v-if="expanded" :id="'preflight-details-' + group.key" class="preflight-group-details">
      <div v-for="item in group.items" :key="item.key" class="preflight-item-row" data-testid="preflight-business-check">
        <span class="preflight-item-label">{{ item.label }}</span>
        <div class="preflight-item-description"><span v-if="item.message">{{ item.message }}</span>
          <span v-for="issue in item.issues" :key="issue.code" class="preflight-issue" :class="issue.status">
            <span>{{ issue.code === 'host_key_changed' ? '连接信息与上次不同，已自动记录并继续，无需操作。' : issue.message }}</span>
            <n-button v-if="issue.remediation" size="tiny" quaternary @click="emit('remediate', issue)">{{ issue.remediation.label }}</n-button>
          </span>
        </div>
        <span class="preflight-item-status" :class="item.status">{{ statusLabels[item.status] }}</span>
      </div>
    </div>
  </section>
</template>

<style scoped>
.preflight-group-toggle { display: grid; grid-template-columns: var(--aio-preflight-columns); min-height: 36px; padding: 4px 10px; gap: 8px; }
.preflight-group-toggle > span { min-width: 0; overflow: hidden; white-space: nowrap; text-overflow: ellipsis; }
.preflight-group-title { font-size: var(--inx-font-size-base); font-weight: 500; }
.preflight-group-address { font-family: var(--inx-font-mono); font-size: 12px; }
.preflight-group-checks { font-size: 12px; font-weight: 400; color: var(--inx-color-text-secondary); }
.preflight-group-status { text-align: right; }
.preflight-group-details { padding: 0 10px 4px 34px; background: var(--inx-color-surface-subtle); }
.preflight-item-row { grid-template-columns: 122px minmax(0, 1fr) 48px; gap: 8px; padding: 4px 0; }
.preflight-common .preflight-group-toggle { grid-template-columns: 16px 122px minmax(0, 1fr) 100px 14px; }
.preflight-common .preflight-group-address { display: none; }
</style>
