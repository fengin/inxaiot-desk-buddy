<script setup lang="ts">
import { screenSettingsFields, screenSettingsRebootLabel, screenSettingsResultClass, screenSettingsResultLabel,
  type ScreenSettingsEvidence } from "@/shared/model/screenSettings";
defineProps<{ evidence: ScreenSettingsEvidence }>();
</script>
<template>
  <div class="screen-settings-sub-results" data-testid="screen-settings-sub-results">
    <span v-for="field in screenSettingsFields" :key="field.key" :title="evidence.items[field.key]?.message" :class="screenSettingsResultClass(evidence.items[field.key]?.state ?? 'unknown')" :data-setting-result="field.key">{{ field.label }}：{{ screenSettingsResultLabel(evidence.items[field.key]?.state ?? 'unknown') }}</span>
    <span class="screen-secondary">{{ screenSettingsRebootLabel(evidence) }}</span>
  </div>
</template>
<style scoped>
.screen-settings-sub-results{display:flex;flex-wrap:wrap;gap:3px 12px;margin-top:4px;min-width:0;width:100%;font-size:11px;line-height:1.7;white-space:normal}
.screen-settings-sub-results>span{flex:0 0 auto;white-space:nowrap}
.screen-settings-sub-results .screen-secondary{max-width:none;margin:0;white-space:normal}
</style>
