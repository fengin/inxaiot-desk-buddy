<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NButton, NCheckbox, NInput, NModal, NSpin } from "naive-ui";
import { Monitor } from "lucide-vue-next";
import { useScreenRemoteAdapter } from "@/shared/api/screenRemoteAdapter";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { createScreenRemoteSession, SCREEN_REMOTE_REFRESH_INTERVAL_MS } from "./screenRemoteSession";
import { screenRemoteTextError, screenTapPoint } from "./screenRemoteInput";
import type { ScreenRemoteFrameInfo, ScreenRemoteKey } from "@/shared/model/screenRemote";

const props = defineProps<{ show: boolean; projectId: string; screenId: string; name: string; ip: string; targetRevision?: string }>();
const emit = defineEmits<{ close: [] }>();
const remote = createScreenRemoteSession(useScreenRemoteAdapter());
const state = remote.state;
const inputText = ref("");
const passwordMode = ref(true);
const viewingBusy = computed(() => state.connecting || state.capturing || state.decoding);
const inputBusy = computed(() => state.connecting || state.controlChanging || state.acting);
const busy = computed(() => viewingBusy.value || inputBusy.value);
const imageLayers = computed(() => {
  const layers: { url: string; frame: ScreenRemoteFrameInfo; current: boolean }[] = [];
  if (state.imageUrl && state.frame) layers.push({ url: state.imageUrl, frame: state.frame, current: true });
  if (state.pendingImageUrl && state.pendingFrame) layers.push({ url: state.pendingImageUrl, frame: state.pendingFrame, current: false });
  return layers;
});
const canOperate = computed(() => remote.canAct());
const canWake = computed(() => remote.canWake());
const wakeOnly = computed(() => state.controlMode === "wake_only");
const textError = computed(() => inputText.value ? screenRemoteTextError(inputText.value, state.maxTextLength) : "");
const canSendText = computed(() => canOperate.value && Boolean(inputText.value.length) && !textError.value);
const inputHint = computed(() => wakeOnly.value ? "屏幕已休眠，请点击唤醒" : `${state.textInputBehavior === 'replace' ? '发送内容会替换屏端当前输入框的全部内容' : '内容插入当前光标处，屏端原内容请先清除'}；支持英文、数字和常用符号（最多${state.maxTextLength}个字符），发送后清空本机输入。`);
const title = computed(() => `${state.session?.name || props.name} · 查看屏幕`);
const captureTime = computed(() => state.frame ? formatDisplayDateTime(state.frame.capturedAt) : "尚未读取");
const feedbackMessage = computed(() => {
  if (state.operationUnconfirmed && state.operationMessage) return state.operationMessage;
  if (state.acting) return "正在发送操作…";
  if (state.controlChanging) return "正在切换辅助操作…";
  if (state.connecting) return "正在连接屏幕…";
  if (state.capturing || state.decoding) return "正在更新屏幕画面…";
  return state.operationMessage;
});
function reconnect() {
  inputText.value = "";
  if (props.show && props.projectId && props.screenId) void remote.open({ projectId: props.projectId, screenId: props.screenId });
}
function close() { inputText.value = ""; void remote.close(); emit("close"); }
function imageFailed(event: Event) { remote.imageFailed((event.target as HTMLImageElement).src); }
function imageLoaded(event: Event) {
  const image = event.target as HTMLImageElement;
  const frame = remote.frameForImage(image.src);
  if (!frame) return;
  if (image.naturalWidth !== frame.width || image.naturalHeight !== frame.height) {
    remote.imageFailed(image.src);
    return;
  }
  remote.imageLoaded(image.src);
}
function tap(event: MouseEvent) {
  const frame = state.frame;
  if (event.button !== 0 || !frame || !state.controlActive) return;
  if (!remote.canAct()) { remote.explainUnavailableAction(); return; }
  const image = event.currentTarget as HTMLImageElement;
  if (image.src !== state.imageUrl) return;
  const point = screenTapPoint(event.clientX, event.clientY, image.getBoundingClientRect(), frame.width, frame.height);
  if (point) void remote.perform({ kind: "tap", ...point });
}
function sendText() {
  if (!canSendText.value) return;
  const text = inputText.value;
  // 点击发送即移除输入框内容；命令失败或超时也不保留供自动重发。
  inputText.value = "";
  void remote.perform({ kind: "text", text });
}
function sendKey(key: ScreenRemoteKey) { void remote.perform({ kind: "key", key }); }
function endControl() { inputText.value = ""; void remote.endControl(); }
watch(() => state.controlActive, (active) => {
  if (!active) { inputText.value = ""; passwordMode.value = true; }
});
watch(() => [props.show, props.projectId, props.screenId, props.ip, props.targetRevision] as const, () => {
  if (props.show && props.projectId && props.screenId) reconnect();
  else void remote.close();
}, { immediate: true });
onBeforeUnmount(() => { inputText.value = ""; void remote.close(); });
</script>

<template>
  <n-modal :show="show" preset="card" :title="title" class="screen-remote-dialog" :bordered="false" :mask-closable="false" @close="close" @update:show="!$event && close()">
    <div class="screen-remote-toolbar">
      <div class="screen-remote-identity"><b>连接IP：{{ state.session?.ip || ip }}</b><span>采集时间：{{ captureTime }}</span></div>
      <div class="screen-remote-controls">
        <n-checkbox :checked="state.autoRefresh" :disabled="!state.session || Boolean(state.error)" @update:checked="remote.setAutoRefresh">自动刷新（{{ SCREEN_REMOTE_REFRESH_INTERVAL_MS / 1000 }}秒）</n-checkbox>
        <n-button class="screen-remote-refresh-button" size="small" :loading="viewingBusy" :disabled="!state.session || viewingBusy" @click="remote.refresh">刷新画面</n-button>
        <n-button v-if="!state.controlActive" class="screen-remote-control-button" size="small" type="primary" :loading="state.controlChanging" :disabled="!state.session || inputBusy || Boolean(state.error)" @click="remote.beginControl()">开启辅助操作</n-button>
        <n-button v-else class="screen-remote-control-button" size="small" :loading="state.controlChanging" :disabled="state.controlChanging" @click="endControl">停止辅助操作</n-button>
      </div>
    </div>
    <div class="screen-remote-viewport" :class="{ 'is-stale': state.stale, 'can-operate': canOperate, 'control-blocked': state.controlActive && !canOperate }" :aria-busy="busy">
      <img v-for="layer in imageLayers" :key="layer.frame.frameId" :class="layer.current ? 'screen-remote-image-current' : 'screen-remote-image-pending'" :src="layer.url" :width="layer.frame.width" :height="layer.frame.height" :alt="layer.current ? `${state.session?.name || name}的当前屏幕` : ''" :aria-hidden="!layer.current" draggable="false" @error="imageFailed" @load="imageLoaded" @click="tap" />
      <div v-if="!state.imageUrl" class="screen-remote-empty">
        <n-spin v-if="busy" size="medium" />
        <Monitor v-else :size="42" :stroke-width="1.5" />
        <span>{{ busy ? '正在读取屏幕…' : '暂无屏幕画面' }}</span>
      </div>
      <span v-if="state.imageUrl && (state.capturing || state.decoding || state.acting)" class="screen-remote-reading">{{ state.acting ? '正在发送操作…' : '正在更新…' }}</span>
      <span v-else-if="state.controlActive && state.waitingForInputFrame" class="screen-remote-reading">正在等待操作后的新画面</span>
      <span v-else-if="state.controlActive && state.frameExpired" class="screen-remote-reading">画面已过期，请刷新后操作</span>
    </div>
    <div class="screen-remote-assist-slot" data-layout-region="assist">
      <div v-if="state.controlActive || state.acting" class="screen-remote-assist">
      <div class="screen-remote-input-row">
        <n-checkbox v-model:checked="passwordMode" :disabled="state.acting || wakeOnly">密码</n-checkbox>
        <n-input v-model:value="inputText" class="screen-remote-text" data-testid="remote-screen-text" size="small" :type="passwordMode ? 'password' : 'text'" show-password-on="mousedown" :input-props="{ autocomplete: 'off', autocapitalize: 'off', autocorrect: 'off', spellcheck: false }" placeholder="先点选屏上的输入框，再输入内容" :disabled="state.acting || state.controlChanging || wakeOnly" @keydown.enter.prevent="sendText" />
        <n-button size="small" type="primary" :disabled="!canSendText" @click="sendText">发送输入</n-button>
        <div class="screen-remote-keys">
          <n-button v-if="state.textInputBehavior === 'insert'" size="small" :disabled="!canOperate" @click="sendKey('backspace')">退格</n-button>
          <n-button v-else size="small" :disabled="!canOperate" @click="sendKey('clear_text')">清空输入框</n-button>
          <n-button size="small" :disabled="!canOperate" @click="sendKey('back')">返回</n-button>
          <n-button size="small" :disabled="!canWake" @click="sendKey('wake_up')">唤醒</n-button>
        </div>
      </div>
      <div class="screen-remote-input-hint" :class="{ 'has-error': Boolean(textError) }">{{ textError || inputHint }}</div>
      </div>
      <div v-else class="screen-remote-assist-idle">辅助操作已关闭，当前仅查看屏幕</div>
    </div>
    <div class="screen-remote-footer">
      <div class="screen-remote-feedback" data-layout-region="feedback">
        <div v-if="state.error" class="screen-remote-error" role="alert" tabindex="0" :title="`${state.error}${state.stale ? ' 当前显示上一次画面。' : ''}`">{{ state.error }}<span v-if="state.stale">当前显示上一次画面。</span></div>
        <div v-else class="screen-remote-operation-message" :class="{ 'is-warning': state.operationUnconfirmed }" role="status" :tabindex="feedbackMessage ? 0 : -1" :aria-hidden="!feedbackMessage" :title="feedbackMessage">{{ feedbackMessage }}</div>
        <n-button v-if="state.error" class="screen-remote-reconnect-button" size="small" :disabled="busy" @click="reconnect">重新连接</n-button>
      </div>
      <n-button size="small" @click="close">关闭</n-button>
    </div>
  </n-modal>
</template>

<style>
.screen-remote-dialog { width: min(1040px, 94vw); height: min(920px, calc(100vh - 32px)); max-height: calc(100vh - 32px); }
.screen-remote-dialog > .n-card-header { flex: 0 0 52px; min-height: 52px; box-sizing: border-box; padding: 12px 20px; }
.screen-remote-dialog .n-card-header__main { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.screen-remote-dialog > .n-card-content { display: flex; flex: 1; flex-direction: column; min-height: 0; padding: 0 20px 16px; overflow-y: auto; scrollbar-gutter: stable; }
.screen-remote-toolbar, .screen-remote-controls, .screen-remote-footer { display: flex; align-items: center; gap: 12px; }
.screen-remote-toolbar { flex: 0 0 44px; min-height: 44px; justify-content: space-between; margin-bottom: 8px; }
.screen-remote-identity { display: flex; min-width: 0; flex-direction: column; align-items: flex-start; gap: 2px; white-space: nowrap; }
.screen-remote-identity span, .screen-remote-footer { color: var(--inx-color-text-secondary); font-size: 12px; }
.screen-remote-controls { flex: 0 0 auto; flex-wrap: nowrap; gap: 8px; }
.screen-remote-refresh-button { width: 104px; }
.screen-remote-control-button { width: 146px; }
.screen-remote-error { box-sizing: border-box; padding: 4px 8px; border-radius: 6px; color: var(--inx-color-warning); background: rgba(240, 160, 32, .09); }
.screen-remote-error span { margin-left: 8px; }
.screen-remote-viewport { position: relative; display: flex; flex: 1 1 0; justify-content: center; align-items: center; min-height: 160px; overflow: hidden; border-radius: 8px; background: #101824; }
.screen-remote-viewport img { position: absolute; inset: 0; display: block; width: 100%; height: 100%; object-fit: contain; user-select: none; }
.screen-remote-viewport .screen-remote-image-pending { opacity: 0; pointer-events: none; }
.screen-remote-viewport.can-operate img { cursor: crosshair; }
.screen-remote-viewport.control-blocked img { cursor: not-allowed; }
.screen-remote-viewport.is-stale { outline: 1px solid var(--inx-color-warning); }
.screen-remote-empty { display: flex; align-items: center; flex-direction: column; gap: 14px; color: #abb8c9; }
.screen-remote-reading { position: absolute; z-index: 1; right: 10px; bottom: 10px; padding: 4px 9px; border-radius: 4px; color: #fff; background: rgba(0,0,0,.65); font-size: 12px; pointer-events: none; }
.screen-remote-footer { flex: 0 0 28px; min-height: 28px; margin-top: 8px; gap: 8px; }
.screen-remote-footer > .n-button { flex: 0 0 auto; }
.screen-remote-assist-slot { flex: 0 0 56px; height: 56px; min-height: 56px; margin-top: 8px; overflow-y: auto; scrollbar-gutter: stable; }
.screen-remote-assist-idle { display: flex; height: 100%; align-items: center; justify-content: center; border-radius: 6px; background: var(--inx-color-surface, #f7f9fb); color: var(--inx-color-text-secondary); font-size: 12px; }
.screen-remote-input-row, .screen-remote-keys { display: flex; align-items: center; gap: 8px; }
.screen-remote-input-row > .n-checkbox { flex: 0 0 auto; }
.screen-remote-text { flex: 1; min-width: 140px; }
.screen-remote-keys { flex: 0 0 auto; }
.screen-remote-input-hint { margin-top: 6px; font-size: 12px; line-height: 18px; color: var(--inx-color-text-secondary); }
.screen-remote-feedback { display: flex; flex: 1 1 0; min-width: 0; height: 28px; min-height: 28px; align-items: center; gap: 8px; }
.screen-remote-feedback > [role] { flex: 1; min-width: 0; max-height: 100%; overflow-y: auto; overflow-wrap: anywhere; font-size: 12px; line-height: 16px; scrollbar-gutter: stable; }
.screen-remote-operation-message { color: var(--inx-color-text-secondary); }
.screen-remote-reconnect-button { flex: 0 0 auto; }
.screen-remote-input-hint.has-error, .screen-remote-operation-message.is-warning { color: var(--inx-color-warning); }
@media (max-width: 760px) {
  .screen-remote-dialog > .n-card-content { padding: 0 14px 14px; }
  .screen-remote-dialog > .n-card-header { padding: 12px 14px; }
  .screen-remote-toolbar { flex: 0 0 80px; min-height: 80px; align-items: flex-start; flex-direction: column; justify-content: center; gap: 8px; }
  .screen-remote-controls { width: 100%; justify-content: flex-end; }
  .screen-remote-input-row { flex-wrap: wrap; }
  .screen-remote-keys { margin-left: auto; }
  .screen-remote-assist-slot { flex-basis: 112px; height: 112px; min-height: 112px; }
}
</style>
