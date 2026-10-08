<script setup lang="ts">
import { computed, ref } from "vue";
import { NButton, NCheckbox, NInput, NPagination, NSelect, NTag } from "naive-ui";
import { Building2, FilePenLine, GitCompareArrows, MapPinOff, Monitor, Search, Wifi, WifiOff, Laptop, ArrowUpRight } from "lucide-vue-next";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { effectiveScreenMac } from "@/shared/model/screen";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { buildScreenSpaceOptions, screenSpaceLabel, screenMatchesSpace, SCREEN_UNLOCATED_SPACE_KEY } from "@/shared/model/screenSpace";
import type { SmartScreen } from "@/shared/model/screen";
import AssetListFooter from "@/shared/components/AssetListFooter.vue";

const store = useSmartScreensStore();
const tableHeader = ref<HTMLDivElement>();
const columnWidths = ["34px", undefined, "122px", "60px", "150px", "92px", "110px", "58px"];
function syncTableScroll(event: Event) {
  if (tableHeader.value) tableHeader.value.scrollLeft = (event.currentTarget as HTMLElement).scrollLeft;
}
defineEmits<{ detail: [id: string]; operate: []; register: [id: string]; merge: []; status: [] }>();
const spacesAvailable = computed(() => store.snapshot.spacesAvailable !== false);
const buildings = computed(() => spacesAvailable.value ? buildScreenSpaceOptions(store.snapshot.spaces) : []);
const locationCount = (key: string) => store.visibleScreens.filter((screen) => screenMatchesSpace(screen, key, store.snapshot.spaces, spacesAvailable.value)).length;
const locationLabel = (screen: SmartScreen) => `${screenSpaceLabel(screen, store.snapshot.spaces, spacesAvailable.value)} · ${screen.location || '位置未填写'}`;
const allPage = computed(() => store.paged.length > 0 && store.paged.every((s) => store.selectedIds.includes(s.id)));
const somePage = computed(() => store.paged.some((s) => store.selectedIds.includes(s.id)) && !allPage.value);
const activeFilters = computed(() => Boolean(store.filters.keyword || store.filters.space || store.filters.size !== "all" || store.filters.registration !== "all" || store.filters.status !== "all" || store.filters.appVersion !== "all"));
const registrationOptions = [{ label: "全部注册状态", value: "all" }, { label: "平台已注册", value: "platform" }, { label: "平台未注册", value: "local" }, { label: "疑似重复", value: "duplicates" }];
const sizeOptions = [{ label: "全部尺寸", value: "all" }, { label: "10 寸屏", value: "10" }, { label: "4 寸屏", value: "4" }, { label: "尺寸待确认", value: "unknown" }];
const statusOptions = [{ label: "全部平台状态", value: "all" }, { label: "平台在线", value: "online" }, { label: "平台离线", value: "offline" }];
const versionOptions = computed(() => [{ label: "全部应用版本", value: "all" }, { label: "版本未记录", value: "unknown" }, ...[...new Set(store.visibleScreens.map((s) => s.appVersion).filter((v): v is string => Boolean(v)))].map((v) => ({ label: `小新 ${v}`, value: v }))]);
function space(key = "") { store.filters.space = key; store.filterChanged(); }
function summary(key: "all" | "platform" | "local" | "online" | "offline" | "duplicates") {
  store.resetFilters();
  if (key === "platform" || key === "local" || key === "duplicates") store.filters.registration = key;
  if (key === "online" || key === "offline") store.filters.status = key;
}
</script>

<template>
  <div class="summary-strip screen-summary-strip">
    <button class="summary-item" :class="{ active: store.filters.registration === 'all' && store.filters.status === 'all' }" type="button" @click="summary('all')">
      <span class="summary-label summary-label--all"><Monitor :size="14" />管理记录</span>
      <span class="summary-value"><strong>{{ store.stats.total }}</strong><small :title="`${store.stats.large} 台 10 寸 · ${store.stats.small} 台 4 寸`">{{ store.stats.large }} 台 10 寸 · {{ store.stats.small }} 台 4 寸</small></span>
    </button>
    <button class="summary-item" :class="{ active: store.filters.registration === 'platform' }" type="button" :disabled="!store.platformAvailable" @click="summary('platform')">
      <span class="summary-label summary-label--all"><Building2 :size="14" />平台已注册</span>
      <span class="summary-value"><strong>{{ store.platformAvailable ? store.stats.platform : '—' }}</strong><small :title="store.platformAvailable ? '项目平台有效屏记录' : '平台数据未获取'">{{ store.platformAvailable ? store.stats.unknown ? `含 ${store.stats.unknown} 台状态未知` : '平台有效记录' : '平台数据未获取' }}</small></span>
    </button>
    <button class="summary-item" :class="{ active: store.filters.registration === 'local' }" type="button" @click="summary('local')">
      <span class="summary-label summary-label--pending"><Laptop :size="14" />平台未注册</span>
      <span class="summary-value"><strong>{{ store.stats.local }}</strong><small title="先维护，后核对平台关联">先维护，后关联</small></span>
    </button>
    <button class="summary-item" :class="{ active: store.filters.status === 'online' }" type="button" :disabled="!store.platformAvailable" @click="summary('online')">
      <span class="summary-label summary-label--online"><Wifi :size="14" />平台在线</span>
      <span class="summary-value"><strong>{{ store.platformAvailable ? store.stats.online : '—' }}</strong><small title="仅统计平台记录状态">以平台状态为准</small></span>
    </button>
    <button class="summary-item" :class="{ active: store.filters.status === 'offline' }" type="button" :disabled="!store.platformAvailable" @click="summary('offline')">
      <span class="summary-label summary-label--offline"><WifiOff :size="14" />平台离线</span>
      <span class="summary-value"><strong>{{ store.platformAvailable ? store.stats.offline : '—' }}</strong><small title="可主动检查本机可达性">可检查本机可达性</small></span>
    </button>
    <button class="summary-item warning" :class="{ active: store.filters.registration === 'duplicates' }" type="button" :disabled="!store.platformAvailable" @click="summary('duplicates')">
      <span class="summary-label summary-label--conflict"><GitCompareArrows :size="14" />疑似重复</span>
      <span class="summary-value"><strong>{{ store.platformAvailable ? store.stats.duplicates : '—' }}</strong><small :title="store.platformAvailable ? '疑似重复候选组数，需人工核对合并' : '平台数据未获取，暂不能核对重复'">{{ store.platformAvailable ? '组待人工核对' : '待平台数据恢复' }}</small></span>
    </button>
  </div>
  <div class="screen-notices" v-if="store.candidates.length || store.statusDifferences.length">
    <button v-if="store.candidates.length" type="button" class="screen-notice warning" @click="$emit('merge')"><GitCompareArrows :size="15" /><span><b>{{ store.candidates.length }} 组疑似重复</b> · 相同 IP 或 MAC，待人工确认</span><span class="screen-text-link">核对合并 <ArrowUpRight :size="13" /></span></button>
    <button v-if="store.statusDifferences.length" type="button" class="screen-notice" @click="$emit('status')"><Wifi :size="15" /><span><b>{{ store.statusDifferences.length }} 台状态不一致</b> · 本机检查与平台不同</span><span class="screen-text-link">查看差异 <ArrowUpRight :size="13" /></span></button>
  </div>
  <div class="screen-list-layout">
    <aside class="screen-space-panel">
      <div class="screen-section-label">所在空间</div>
      <div class="screen-space-list inx-scroll-area">
      <button type="button" :class="{active:store.filters.space===SCREEN_UNLOCATED_SPACE_KEY}" @click="space(SCREEN_UNLOCATED_SPACE_KEY)"><MapPinOff :size="14" />待定空间<small>{{ locationCount(SCREEN_UNLOCATED_SPACE_KEY) }}</small></button>
      <button type="button" :class="{ active: !store.filters.space }" @click="space()"><Building2 :size="14" />全部空间 <small>{{ store.visibleScreens.length }}</small></button>
      <div v-for="building in buildings" :key="building.value" class="screen-building">
        <button type="button" :class="{ active: store.filters.space === building.value }" @click="space(building.value)"><Building2 :size="14" />{{ building.label }}<small>{{ locationCount(building.value) }}</small></button>
        <button v-for="floor in building.children" :key="floor.value" class="screen-floor" type="button" :class="{ active: store.filters.space === floor.value }" @click="space(floor.value)">{{ floor.label }}<small>{{ locationCount(floor.value) }}</small></button>
      </div>
      <div class="screen-space-note">{{ spacesAvailable ? '楼幢、楼层筛选包含其下全部空间；未选择或关联失效的屏归入待定空间。' : '空间目录暂不可用；已有关联和缓存路径保留，恢复后再核验。' }}</div>
      </div>
    </aside>
    <div class="screen-table-panel">
      <div class="screen-filterbar">
        <n-input v-model:value="store.filters.keyword" size="small" aria-label="搜索屏" placeholder="搜索名称、位置、MAC、IP" title="按名称、位置、MAC、IP 搜索" clearable class="screen-search" @update:value="store.filterChanged"><template #prefix><Search :size="15" /></template></n-input>
        <n-select v-model:value="store.filters.registration" size="small" aria-label="注册状态筛选" :options="registrationOptions" @update:value="store.filterChanged" />
        <n-select v-model:value="store.filters.size" size="small" aria-label="屏尺寸筛选" :options="sizeOptions" @update:value="store.filterChanged" />
        <n-select v-model:value="store.filters.status" size="small" aria-label="平台状态筛选" :options="statusOptions" @update:value="store.filterChanged" />
        <n-select v-model:value="store.filters.appVersion" size="small" aria-label="应用版本筛选" :options="versionOptions" @update:value="store.filterChanged" />
        <n-button size="small" quaternary :disabled="!activeFilters" @click="store.resetFilters">重置</n-button>
      </div>
      <div ref="tableHeader" class="screen-list-table-header table-scroll">
        <table class="screen-table screen-list-table">
          <colgroup><col v-for="(width, index) in columnWidths" :key="index" :style="{ width }" /></colgroup>
          <thead><tr><th class="screen-checkbox"><n-checkbox aria-label="选择当前页" :checked="allPage" :indeterminate="somePage" @update:checked="store.selectPage" /></th><th>屏名称 / 所在位置</th><th>注册 / 平台状态</th><th>尺寸</th><th>IP / MAC</th><th>小新版本</th><th>本机检查</th><th>操作</th></tr></thead>
        </table>
      </div>
      <div class="screen-table-scroll table-scroll" @scroll="syncTableScroll">
        <table class="screen-table screen-list-table">
          <colgroup><col v-for="(width, index) in columnWidths" :key="index" :style="{ width }" /></colgroup>
          <tbody>
            <tr v-for="screen in store.paged" :key="screen.id" :class="{ selected: store.selectedIds.includes(screen.id) }">
              <td><n-checkbox :aria-label="`选择 ${screen.name}`" :checked="store.selectedIds.includes(screen.id)" @update:checked="store.toggleSelection(screen.id, $event)" /></td>
      <td><div class="screen-name-line"><button class="screen-name" type="button" :title="screen.name" @click="$emit('detail', screen.id)">{{ screen.name }}</button><button v-if="store.candidateIds.has(screen.id)" class="screen-duplicate-link" type="button" @click="$emit('merge')"><GitCompareArrows :size="11" />疑似重复</button><button v-if="store.draftIds.has(screen.id)" class="screen-draft-link" type="button" :data-testid="`screen-draft-${screen.id}`" title="核对本机待提交修改并更新平台" @click="$emit('register', screen.id)"><FilePenLine :size="11" />资料有变更</button></div><div class="screen-secondary" :title="locationLabel(screen)">{{ locationLabel(screen) }}</div></td>
              <td><n-tag size="small" :bordered="false" :type="screen.source === 'local' || !store.platformAvailable || screen.platformStatus === 'unknown' ? 'default' : screen.platformStatus === 'online' ? 'success' : 'error'">{{ screen.source === 'local' ? '平台未注册' : screen.platformStatus === 'online' ? '平台在线' : screen.platformStatus === 'offline' ? '平台离线' : '平台未知' }}</n-tag><div class="screen-secondary">{{ screen.source === 'local' ? '本机记录' : store.platformAvailable ? '平台已注册' : '已注册 · 缓存' }}</div></td>
              <td><span class="screen-size">{{ screen.size === 'unknown' ? '待确认' : `${screen.size} 寸` }}</span></td>
              <td class="screen-network"><span>{{ screen.ip }}</span><div class="screen-secondary">{{ effectiveScreenMac(screen) || 'MAC 待采集' }}</div></td>
              <td>{{ screen.appVersion || '未记录' }}<div class="screen-secondary">{{ screen.abi === '尚未检查' ? '架构待检查' : screen.abi }}</div></td>
              <td><span :class="screen.ping === 'online' ? 'screen-success' : screen.ping === 'offline' ? 'screen-warning' : 'screen-muted'">{{ screen.ping === null ? '未检查' : screen.ping === 'online' ? 'IP 可达' : screen.ping === 'offline' ? 'IP 不可达' : '检查结果未知' }}</span><div class="screen-secondary" :title="screen.checkedAt ? formatDisplayDateTime(screen.checkedAt) : ''">{{ screen.checkedAt ? '有检查记录' : '等待采集' }}</div></td>
              <td><n-button text size="tiny" class="screen-detail-link" type="primary" @click="$emit('detail', screen.id)">详情</n-button></td>
            </tr>
          </tbody>
        </table>
        <div v-if="!store.paged.length" class="screen-empty"><Search :size="26" /><strong>没有符合条件的屏</strong><span>调整关键词或筛选条件后重试。</span><n-button @click="store.resetFilters">清空筛选</n-button></div>
      </div>
      <asset-list-footer :selected-count="store.selectedIds.length" @batch="$emit('operate')">
        <template #summary>当前显示 {{ store.paged.length }} / {{ store.filtered.length }} 条</template>
        <template #pagination><n-pagination v-model:page="store.page" v-model:page-size="store.pageSize" :item-count="store.filtered.length" :page-sizes="[10, 20, 50]" :page-slot="5" size="small" show-size-picker @update:page-size="store.page = 1" /></template>
      </asset-list-footer>
    </div>
  </div>
</template>
