<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { NButton } from "naive-ui";
import { CheckCircle2, CircleAlert, ChevronRight } from "lucide-vue-next";
import type { DeploymentPreflightCheck, DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import { preflightGroups, preflightGroupPassed, type PreflightNodeIdentity } from "./preflightPresentation";

const props = defineProps<{ report?: DeploymentPreflightReport; nodes: PreflightNodeIdentity[] }>();
const emit = defineEmits<{ remediate: [check: DeploymentPreflightCheck] }>();
const groups = computed(() => preflightGroups(props.report, props.nodes));
const expandedGroups = ref(new Set<string>());
const statusLabels = { passed: "通过", warning: "有提示", failed: "需处理", pending: "未检查" };

watch(() => props.report, () => {
  expandedGroups.value = new Set(groups.value.filter((group) => !preflightGroupPassed(group)).map((group) => group.key));
}, { immediate: true });

function toggleGroup(key: string) {
  if (expandedGroups.value.has(key)) expandedGroups.value.delete(key);
  else expandedGroups.value.add(key);
}
</script>

<template>
  <div class="preflight-groups">
    <section v-for="group in groups" :key="group.key" class="preflight-group" :data-testid="'preflight-group-' + group.key">
      <h3 class="preflight-group-heading">
        <button type="button" class="preflight-group-toggle" :aria-expanded="expandedGroups.has(group.key)" :aria-controls="'preflight-details-' + group.key" @click="toggleGroup(group.key)">
          <CheckCircle2 v-if="preflightGroupPassed(group)" class="preflight-group-status-icon success" :size="18" aria-hidden="true" data-state="passed" />
          <CircleAlert v-else class="preflight-group-status-icon error" :size="18" aria-hidden="true" data-state="failed" />
          <span class="preflight-group-identity"><span class="preflight-group-title">{{ group.title }}</span><span v-if="group.subtitle" class="preflight-group-address">{{ group.subtitle }}</span></span>
          <span v-if="group.items.some((item) => item.issues.some((issue) => issue.status === 'warning'))" class="preflight-group-note">有提示</span>
          <span class="preflight-group-status" :class="preflightGroupPassed(group) ? 'success' : 'error'">{{ preflightGroupPassed(group) ? '通过' : '不通过' }}</span>
          <ChevronRight :size="15" class="preflight-group-chevron" :class="{ expanded: expandedGroups.has(group.key) }" aria-hidden="true" />
        </button>
      </h3>
      <div v-if="expandedGroups.has(group.key)" :id="'preflight-details-' + group.key" class="preflight-group-details">
        <div v-for="item in group.items" :key="item.key" class="preflight-item-row" data-testid="preflight-business-check">
          <span class="preflight-item-label">{{ item.label }}</span>
          <div class="preflight-item-description"><span>{{ item.message }}</span>
            <span v-for="issue in item.issues" :key="issue.code" class="preflight-issue">
              <span>{{ issue.code === 'host_key_changed' ? '连接信息与上次不同，已自动记录并继续，无需操作。' : issue.message }}</span>
              <n-button v-if="issue.remediation" size="tiny" quaternary @click="emit('remediate', issue)">{{ issue.remediation.label }}</n-button>
            </span>
          </div>
          <span class="preflight-item-status" :class="item.status">{{ statusLabels[item.status] }}</span>
        </div>
      </div>
    </section>
  </div>
</template>
