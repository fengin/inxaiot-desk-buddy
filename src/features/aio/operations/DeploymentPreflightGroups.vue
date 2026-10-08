<script setup lang="ts">
import { computed, ref, watch } from "vue";
import type { DeploymentPreflightCheck, DeploymentPreflightReport } from "@/shared/model/deploymentWorkflow";
import { preflightGroups, preflightGroupPassed, type PreflightNodeIdentity } from "./preflightPresentation";
import DeploymentPreflightRow from "./DeploymentPreflightRow.vue";

const props = defineProps<{ report?: DeploymentPreflightReport; nodes: PreflightNodeIdentity[] }>();
const emit = defineEmits<{ remediate: [check: DeploymentPreflightCheck] }>();
const groups = computed(() => preflightGroups(props.report, props.nodes));
const expandedGroups = ref(new Set<string>());
const commonGroup = computed(() => groups.value.find(group => group.key === "common"));
const nodeGroups = computed(() => groups.value.filter(group => group.key !== "common"));

watch(() => props.report, () => {
  expandedGroups.value = new Set(groups.value.filter((group) => !preflightGroupPassed(group)).map((group) => group.key));
}, { immediate: true });

function toggleGroup(key: string) {
  if (expandedGroups.value.has(key)) expandedGroups.value.delete(key);
  else expandedGroups.value.add(key);
}
</script>

<template>
  <div class="preflight-groups aio-preflight-table">
    <DeploymentPreflightRow v-if="commonGroup" :group="commonGroup" :expanded="expandedGroups.has('common')" @toggle="toggleGroup('common')" @remediate="emit('remediate', $event)" />
    <div class="aio-preflight-head" aria-hidden="true"><span></span><span>一体机名称</span><span>IP 地址</span><span>检查结果</span><span>状态</span><span></span></div>
    <div class="aio-preflight-rows inx-scroll-area" tabindex="0" aria-label="一体机检查结果列表">
      <DeploymentPreflightRow v-for="group in nodeGroups" :key="group.key" :group="group" :expanded="expandedGroups.has(group.key)" @toggle="toggleGroup(group.key)" @remediate="emit('remediate', $event)" />
    </div>
  </div>
</template>

<style scoped>
.aio-preflight-table { --aio-preflight-columns: 16px minmax(130px, 1fr) 126px minmax(200px, 1.6fr) 100px 14px; display: flex; flex: 1; min-height: 0; flex-direction: column; border: 1px solid var(--inx-color-border); border-radius: var(--inx-radius-sm); overflow: hidden; }
.aio-preflight-head { display: grid; grid-template-columns: var(--aio-preflight-columns); align-items: center; gap: 8px; min-height: 30px; padding: 0 14px 0 10px; border-bottom: 1px solid var(--inx-color-border); background: var(--inx-color-table-header); color: var(--inx-color-text-secondary); font-size: 11px; overflow: hidden; }
.aio-preflight-table > :first-child, .aio-preflight-head { flex: none; }
.aio-preflight-rows { flex: 1; min-height: 0; overflow: auto; }
</style>
