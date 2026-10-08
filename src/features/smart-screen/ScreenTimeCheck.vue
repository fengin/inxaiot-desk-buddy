<script setup lang="ts">
import { computed } from 'vue';
import { NButton, NPopover } from 'naive-ui';
import type { ScreenInspection } from '@/shared/model/screen';
import { formatDisplayDateTime } from '@/shared/format/dateTime';
const props = defineProps<{ observation: ScreenInspection }>();
const checkedTime = (value?: string | null) => value ? formatDisplayDateTime(value) : '未读取';
const enabled = (value?: boolean | null) => value == null ? '未读取' : value ? '开启' : '关闭';
const details = computed(() => `设备时间：${checkedTime(props.observation.deviceTime)}\n读取时电脑时间：${checkedTime(props.observation.computerTime)}\n设备时区：${props.observation.timezone || '未读取'}\n自动校时 / 时区：${enabled(props.observation.automaticTime)} / ${enabled(props.observation.automaticTimezone)}`);
</script>
<template>
  <n-popover trigger="click" placement="bottom">
    <template #trigger><n-button size="tiny" text type="primary" :title="details" aria-label="查看校时检查详情" data-action-owner="time-check-popover">{{ observation.clockOffsetSeconds == null ? '未读取' : `${observation.clockOffsetSeconds} 秒` }} · 详情</n-button></template>
    <dl class="screen-time-check">
    <dt>设备时间</dt><dd>{{ checkedTime(observation.deviceTime) }}</dd>
    <dt>读取时电脑时间</dt><dd>{{ checkedTime(observation.computerTime) }}</dd>
    <dt>设备时区</dt><dd>{{ observation.timezone || '未读取' }}</dd>
    <dt>自动校时 / 时区</dt><dd>{{ enabled(observation.automaticTime) }} / {{ enabled(observation.automaticTimezone) }}</dd>
    <dt>时间偏差</dt><dd>{{ observation.clockOffsetSeconds == null ? '未读取' : `${observation.clockOffsetSeconds} 秒` }}</dd>
    </dl>
  </n-popover>
</template>
<style scoped>
.screen-time-check { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 4px 10px; margin: 0; font-size: var(--inx-font-size-table); }
dt { color: var(--inx-color-text-secondary); } dd { margin: 0; }
</style>
