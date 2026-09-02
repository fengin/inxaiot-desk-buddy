<script setup lang="ts">
import { computed } from "vue";
import { NButton } from "naive-ui";
import { CheckCircle2, CircleAlert, Clock3 } from "lucide-vue-next";
import type { DeploymentPreflightCheck, DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import { preflightGroups, type PreflightNodeIdentity } from "./preflightPresentation";

const props = defineProps<{ report?: DeploymentPreflightReport; nodes: PreflightNodeIdentity[] }>();
const emit = defineEmits<{ remediate: [check: DeploymentPreflightCheck] }>();
const groups = computed(() => preflightGroups(props.report, props.nodes));
const statusLabels = { passed: "通过", warning: "有提示", failed: "需处理", pending: "未检查" };
</script>

<template>
  <div class="preflight-groups">
    <section v-for="group in groups" :key="group.key" class="preflight-group" :data-testid="'preflight-group-' + group.key">
      <header><h3>{{ group.title }}</h3><span v-if="group.subtitle">{{ group.subtitle }}</span></header>
      <div class="check-grid">
        <div v-for="item in group.items" :key="item.key" class="check-item" :class="item.status === 'passed' ? 'success' : item.status === 'failed' ? 'error' : item.status === 'warning' ? 'warning' : 'pending'" data-testid="preflight-business-check">
          <CheckCircle2 v-if="item.status === 'passed'" :size="18" /><Clock3 v-else-if="item.status === 'pending'" :size="18" /><CircleAlert v-else :size="18" />
          <span><strong>{{ item.label }}</strong><small>{{ item.message }}</small>
            <span v-for="issue in item.issues" :key="issue.code" class="preflight-issue">
              <small>{{ issue.code === 'host_key_changed' ? '连接信息与上次不同，已自动记录并继续，无需操作。' : issue.message }}</small>
              <n-button v-if="issue.remediation" size="tiny" quaternary @click="emit('remediate', issue)">{{ issue.remediation.label }}</n-button>
            </span>
          </span>
          <b>{{ statusLabels[item.status] }}</b>
        </div>
      </div>
    </section>
  </div>
</template>
