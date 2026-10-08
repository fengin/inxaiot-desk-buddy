<script setup lang="ts">
import { computed, reactive } from "vue";
import { NButton, NCheckbox, NInput, NSelect, NTag } from "naive-ui";
import { Monitor, Search } from "lucide-vue-next";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { screenMatchesSpace, screenSpaceLabel } from "@/shared/model/screenSpace";
import { effectiveScreenMac, normalizeScreenMac } from "@/shared/model/screen";
import type { SmartScreen } from "@/shared/model/screen";
import ScreenSpaceSelect from "./ScreenSpaceSelect.vue";

defineProps<{ disabled?: boolean }>();
const store = useSmartScreensStore();
const filter = reactive({ keyword: "", space: "", source: "all", size: "all" });
const spacesAvailable = computed(() => store.snapshot.spacesAvailable !== false);
const matched = computed(() => store.visibleScreens.filter((screen) => {
  const key = filter.keyword.trim().toLocaleLowerCase();
  const mac = normalizeScreenMac(key);
  const textMatch = !key || [screen.name, screen.location, screenSpaceLabel(screen, store.snapshot.spaces, spacesAvailable.value), screen.ip, effectiveScreenMac(screen)].some((value) => value.toLocaleLowerCase().includes(key)) || (/^[0-9A-F]{4,}$/.test(mac) && normalizeScreenMac(effectiveScreenMac(screen)).includes(mac));
  return textMatch && screenMatchesSpace(screen, filter.space, store.snapshot.spaces, spacesAvailable.value) && (filter.source === "all" || screen.source === filter.source) && (filter.size === "all" || screen.size === filter.size);
}));
const selectedMatched = computed(() => matched.value.filter((screen) => store.selectedIds.includes(screen.id)).length);
const allMatched = computed(() => matched.value.length > 0 && selectedMatched.value === matched.value.length);
function selectMatched(checked: boolean) { for (const screen of matched.value) store.toggleSelection(screen.id, checked); }
function locationLabel(screen: SmartScreen) {
  return `${screenSpaceLabel(screen, store.snapshot.spaces, spacesAvailable.value)} · ${screen.location || '位置未填写'}`;
}
</script>

<template>
  <section class="stage-main screen-target-picker" aria-label="选择屏设备">
    <header class="stage-heading"><div><span class="feature-icon info"><Monitor :size="20" /></span><span><strong>选择智能屏范围</strong><small v-if="store.selectedIds.length > selectedMatched">另有 {{ store.selectedIds.length - selectedMatched }} 台已选，不在当前筛选内</small></span></div></header>
    <div class="selection-toolbar screen-target-toolbar">
      <screen-space-select v-model="filter.space" :spaces="spacesAvailable ? store.snapshot.spaces : []" :disabled="disabled" size="tiny" />
      <n-select v-model:value="filter.source" aria-label="运维注册状态" :disabled="disabled" size="tiny" :options="[{label:'全部注册状态',value:'all'},{label:'平台已注册',value:'platform'},{label:'平台未注册',value:'local'}]" />
      <n-select v-model:value="filter.size" aria-label="运维屏尺寸" :disabled="disabled" size="tiny" :options="[{label:'全部尺寸',value:'all'},{label:'10 寸',value:'10'},{label:'4 寸',value:'4'},{label:'待确认',value:'unknown'}]" />
      <n-input v-model:value="filter.keyword" class="target-search" aria-label="搜索运维目标" title="搜索屏名称、位置、MAC、IP" :disabled="disabled" size="tiny" clearable placeholder="搜索"><template #prefix><Search :size="14" /></template></n-input>
      <n-button size="tiny" quaternary :disabled="disabled || !matched.length" :title="allMatched ? '取消当前匹配设备的选择，保留其他已选设备' : '选择全部匹配设备，保留其他已选设备'" @click="selectMatched(!allMatched)">{{ allMatched ? '取消全选' : '选择全部' }}</n-button>
      <n-button size="tiny" quaternary :disabled="disabled || !store.selectedIds.length" @click="store.selectedIds = []">清空选择</n-button>
      <b>已选 <strong class="metric-number metric-info">{{ store.selectedIds.length }}</strong> / 匹配 <strong class="metric-number metric-operation">{{ matched.length }}</strong> 台</b>
    </div>
    <div class="node-selection-list screen-target-rows">
      <label v-for="screen in matched" :key="screen.id" class="screen-target-row" :class="{selected:store.selectedIds.includes(screen.id), disabled}">
        <n-checkbox :checked="store.selectedIds.includes(screen.id)" :disabled="disabled" :aria-label="`运维选择 ${screen.name}`" @update:checked="store.toggleSelection(screen.id,$event)" />
        <strong class="node-selection-name" :title="screen.name">{{ screen.name }}</strong>
        <span class="node-selection-ip" :title="`本次连接 IP：${screen.ip}（已确认地址，待提交修改尚未生效）`">{{ screen.ip }}</span>
        <span class="node-selection-location" :title="locationLabel(screen)">{{ locationLabel(screen) }}</span>
        <span class="screen-target-size" :title="screen.size === 'unknown' ? '尺寸待确认' : `${screen.size} 寸`">{{ screen.size === 'unknown' ? '待确认' : `${screen.size} 寸` }}</span>
        <n-tag size="small" :bordered="false" :type="screen.source === 'platform' ? 'info' : 'default'">{{ screen.source === 'platform' ? '已注册' : '未注册' }}</n-tag>
      </label>
      <div v-if="!matched.length" class="screen-empty"><span>没有匹配的屏，请调整筛选条件。</span></div>
    </div>
  </section>
</template>
