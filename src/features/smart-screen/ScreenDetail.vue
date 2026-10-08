<script setup lang="ts">
import { computed } from "vue";
import { NButton, NDrawer, NDrawerContent, NTag, useDialog, useMessage } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { effectiveScreenMac, screenActionLabel, screenActions, screenStateLabel } from "@/shared/model/screen";
import type { ScreenAction, SmartScreen } from "@/shared/model/screen";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { screenSpaceLabel } from "@/shared/model/screenSpace";

const props = defineProps<{ screen?: SmartScreen }>();
const emit = defineEmits<{ close: []; edit: [screen: SmartScreen]; operate: [action: ScreenAction]; versions: [] }>();
const store = useSmartScreensStore();
const dialog = useDialog();
const message = useMessage();
const spaceLabel = computed(() => props.screen ? screenSpaceLabel(props.screen, store.snapshot.spaces, store.snapshot.spacesAvailable !== false) : "");
const history = computed(() => store.snapshot.tasks.filter((task) => task.targets.some((t) => props.screen && [props.screen.id, ...props.screen.aliases].includes(t.screenId))));
const adbLabel = computed(() => props.screen?.adbStatus === 'unauthorized' ? 'ADB 未授权' : props.screen?.adbAvailable == null ? '未检查' : props.screen.adbAvailable ? 'ADB 可用' : 'ADB 无法连接');
const switchLabel = (value?: boolean | null) => value == null ? '未检查' : value ? '开启' : '关闭';
const checkedTime = (value?: string | null) => value ? formatDisplayDateTime(value) : '未检查';
function remove() {
  const screen = props.screen;
  if (!screen) return;
  dialog.warning({ title: "从本机列表移除？", content: `移除 ${screen.name} 的本机管理记录。设备和平台不会被修改，操作历史保留。`, positiveText: "移除本机记录", negativeText: "取消", onPositiveClick: async () => {
    try { await useScreenAdapter().removeLocal(store.projectId, screen.id); emit("close"); message.success("已移除本机记录"); }
    catch (cause) { message.error((cause as Error).message); return false; }
  } });
}
</script>

<template>
  <n-drawer :show="Boolean(screen)" width="min(var(--inx-detail-drawer-width), 94vw)" class="screen-dialog screen-detail-drawer device-detail-drawer" @update:show="!$event && $emit('close')">
    <n-drawer-content v-if="screen" :title="screen.name" body-content-class="inx-scroll-area" closable>
      <div class="screen-detail-tags"><n-tag :bordered="false" :type="screen.source === 'local' ? 'default' : 'info'">{{ screen.source === 'local' ? '平台未注册 · 本机管理' : '平台已注册' }}</n-tag><n-tag :bordered="false">{{ screen.size === 'unknown' ? '尺寸待确认' : `${screen.size} 寸屏` }}</n-tag><n-tag v-if="screen.aliases.length" type="success" :bordered="false">已关联本机历史</n-tag><n-tag v-if="screen.source === 'platform' && !store.platformAvailable" :bordered="false">缓存资料</n-tag></div>
      <div v-if="store.draftIds.has(screen.id)" class="screen-draft-notice" data-testid="screen-detail-draft"><span>资料有变更，尚未更新平台</span><n-button size="tiny" text type="primary" @click="$emit('operate', 'register')">核对并更新</n-button></div>
      <div v-if="screen.source === 'platform' && !store.platformAvailable" class="screen-secondary">缓存读取时间：{{ store.snapshot.platformReadAt ? formatDisplayDateTime(store.snapshot.platformReadAt) : '未记录' }}，恢复连接后刷新。</div>
      <h3 class="device-detail-section-title">资产信息</h3>
      <dl class="device-detail-facts">
        <div><dt>设备 IP</dt><dd :title="screen.ip">{{ screen.ip }}</dd></div><div><dt>所在空间</dt><dd :title="spaceLabel">{{ spaceLabel }}</dd></div>
        <div class="device-detail-full-row"><dt>安装位置</dt><dd :title="screen.location || '未填写'">{{ screen.location || '未填写' }}</dd></div>
        <div><dt>平台 MAC</dt><dd :title="screen.source === 'local' ? '无平台记录' : screen.mac || '平台未填写'">{{ screen.source === 'local' ? '无平台记录' : screen.mac || '平台未填写' }}</dd></div><div><dt>采集 MAC</dt><dd :title="screen.observedMac || '未采集'">{{ screen.observedMac || '未采集' }}</dd></div>
        <div><dt>平台在线状态</dt><dd>{{ screen.source === 'local' ? '无平台记录' : screen.platformStatus === 'online' ? '在线' : screen.platformStatus === 'offline' ? '离线' : '未知' }}</dd></div><div><dt>本机 IP 检查</dt><dd>{{ screen.ping === null ? '未检查' : screen.ping === 'online' ? '可达' : screen.ping === 'offline' ? '不可达' : '结果未知' }}</dd></div>
        <div><dt>MAC 来源</dt><dd>{{ screen.macSource || '未采集' }}</dd></div><div><dt>MAC 采集时间</dt><dd>{{ checkedTime(screen.macCheckedAt) }}</dd></div>
      </dl>
      <div v-if="!effectiveScreenMac(screen)" class="screen-callout">MAC 尚未采集，可执行“获取/核对 MAC”。应用设备标识与网卡 MAC 分别管理。</div>
      <h3 class="device-detail-section-title">设备与小新应用 <small v-if="store.snapshot.mode !== 'real'">模拟观测</small></h3>
      <dl class="device-detail-facts">
        <div><dt>系统 / 架构</dt><dd :title="`${screen.android} · ${screen.abi}`">{{ screen.android }} · {{ screen.abi }}</dd></div><div><dt>{{ screen.source === 'platform' ? '平台小新版本' : '本机小新版本' }}</dt><dd :title="screen.appVersion || '未记录'">{{ screen.appVersion || '未记录' }}</dd></div>
        <div v-if="screen.observedAppVersion && screen.observedAppVersion !== screen.appVersion"><dt>本机观测版本</dt><dd :title="`${screen.observedAppVersion}（未覆盖平台）`">{{ screen.observedAppVersion }}（未覆盖平台）</dd></div>
        <div><dt>管理连接</dt><dd :class="screen.adbAvailable == null ? 'screen-muted' : screen.adbAvailable ? 'screen-success' : 'screen-warning'">{{ adbLabel }}</dd></div><div><dt>可用空间</dt><dd>{{ screen.freeSpaceMb == null ? '未检查' : `${screen.freeSpaceMb} MB` }}</dd></div>
        <div><dt>小新安装状态</dt><dd>{{ screen.appInstalled == null ? '未检查' : screen.appInstalled ? '已安装' : '未安装' }}</dd></div><div><dt>小新运行状态</dt><dd>{{ screen.appInstalled === false ? '未安装' : screen.appRunning == null ? '未检查' : screen.appRunning ? '运行中' : '未运行' }}</dd></div>
        <div><dt>时间偏差</dt><dd>{{ screen.clockOffsetSeconds == null ? '未检查' : `${screen.clockOffsetSeconds} 秒 · 保留设备时区` }}</dd></div><div><dt>5555持久配置</dt><dd>{{ screen.size !== '10' ? '不适用' : screen.persistentAdb == null ? '未检查' : screen.persistentAdb ? '已读到5555配置' : '未读到5555配置' }}</dd></div>
        <div class="device-detail-full-row"><dt>最近检查</dt><dd :title="screen.checkedAt ? formatDisplayDateTime(screen.checkedAt) : '无检查记录'">{{ screen.checkedAt ? formatDisplayDateTime(screen.checkedAt) : '无检查记录' }}</dd></div>
      </dl>
      <details class="screen-inspection-more">
        <summary>更多检查信息</summary>
        <dl class="device-detail-facts">
          <div><dt>设备型号</dt><dd>{{ screen.deviceModel || '未检查' }}</dd></div><div><dt>ADB 检查时间</dt><dd>{{ checkedTime(screen.adbCheckedAt) }}</dd></div>
          <div class="device-detail-full-row"><dt>设备固件</dt><dd>{{ screen.firmware || '未检查' }}</dd></div>
          <div><dt>设备时间</dt><dd>{{ checkedTime(screen.deviceTime) }}</dd></div><div><dt>读取时电脑时间</dt><dd>{{ checkedTime(screen.computerTime) }}</dd></div>
          <div><dt>设备时区</dt><dd>{{ screen.timezone || '未检查' }}</dd></div><div><dt>自动校时</dt><dd>{{ switchLabel(screen.automaticTime) }}</dd></div>
          <div><dt>自动时区</dt><dd>{{ switchLabel(screen.automaticTimezone) }}</dd></div><div><dt>版本读取时间</dt><dd>{{ checkedTime(screen.versionCheckedAt) }}</dd></div>
          <div v-if="screen.macCandidates?.length" class="device-detail-full-row"><dt>网卡采集结果</dt><dd>{{ screen.macCandidates.join('；') }}</dd></div>
          <div v-if="screen.source === 'platform'"><dt>平台读取时间</dt><dd>{{ checkedTime(store.snapshot.platformReadAt) }}</dd></div><div v-if="screen.source === 'platform'"><dt>平台检查时间</dt><dd>平台未提供</dd></div>
        </dl>
        <div v-if="screen.inspectionErrors?.length" class="screen-callout"><div v-for="item in screen.inspectionErrors" :key="item">{{ item }}</div></div>
      </details>
      <h3 class="device-detail-section-title">设备运维</h3>
      <div class="screen-detail-actions"><n-button v-for="action in screenActions" :key="action.value" size="small" :disabled="action.value === 'adb' && screen.size !== '10'" @click="$emit('operate', action.value)">{{ action.label }}</n-button><n-button size="small" title="重新核对设备应用版本，无需重复安装" @click="$emit('versions')">核对版本并同步平台</n-button></div>
      <p class="screen-muted">“修改小新配置”读取屏端当前设置；修改运行中的环境配置后，会按需重启小新应用。</p>
      <h3 class="device-detail-section-title">最近操作 <small>{{ history.length }} 条</small></h3>
      <div v-if="!history.length" class="screen-callout">暂无操作记录。检查、安装和维护的结果会保留在这里。</div>
      <div v-for="task in history" :key="task.id" class="screen-history-item"><div><b>{{ screenActionLabel(task.action) }}</b><n-tag size="small" :bordered="false">{{ screenStateLabel[task.state] }}</n-tag></div><small>{{ formatDisplayDateTime(task.createdAt) }}<template v-if="task.mode !== 'real'"> · 原型模拟</template></small><p>{{ task.targets.find((t) => [screen!.id, ...screen!.aliases].includes(t.screenId))?.message }}</p></div>
      <template #footer><div class="screen-dialog-footer"><n-button v-if="screen.source === 'local'" type="error" quaternary @click="remove">移除本机记录</n-button><span class="screen-spacer"></span><n-button @click="$emit('close')">关闭</n-button><n-button type="primary" @click="$emit('edit', screen)">{{ screen.source === 'local' ? '编辑本机信息' : '编辑资料' }}</n-button></div></template>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.device-detail-facts {
  grid-template-columns: max-content minmax(0, .85fr) max-content minmax(0, 1.15fr);
  column-gap: 8px;
}
.device-detail-facts > div {
  grid-column: span 2;
  grid-template-columns: subgrid;
  align-items: baseline;
  column-gap: 8px;
}
.device-detail-facts > .device-detail-full-row { grid-column: 1 / -1; }
.device-detail-full-row > dd { grid-column: 2 / -1; }
.device-detail-facts dt,
.device-detail-facts dd {
  font-size: 11px;
  font-weight: 400;
  line-height: 1.5;
}
.screen-inspection-more { margin-top: 10px; }
.screen-inspection-more summary { cursor: pointer; color: var(--inx-color-text-secondary); font-size: 11px; }
.screen-inspection-more .device-detail-facts { margin-top: 8px; }
.device-detail-facts dd {
  overflow: visible;
  text-overflow: clip;
  white-space: normal;
  overflow-wrap: anywhere;
}
</style>
