<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NButton, NPagination, NProgress, NTag, NModal, NCard, useMessage } from "naive-ui";
import { FileText, RefreshCw } from "lucide-vue-next";
import { useActivityStore } from "@/stores/activity";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { screenActionLabel, screenStateLabel } from "@/shared/model/screen";
import type { ScreenTask, ScreenTaskTarget } from "@/shared/model/screen";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { appConfigFieldLabel } from "@/shared/model/screenAppConfig";
import { ntpServerDisplay, ntpStageLabel } from "@/shared/model/screenNtp";
import type { ScreenNtpEvidence } from "@/shared/model/screenNtp";

const props = defineProps<{ task: ScreenTask }>();
const emit = defineEmits<{ retry: [ids: string[]]; restart: [ids: string[]]; status: []; logsOpened: []; versions: [ids: string[]]; detail: [id: string] }>();
const activity = useActivityStore();
const message = useMessage();
let logOpenRequest = 0;
onBeforeUnmount(() => { ++logOpenRequest; });
const page = ref(1), pageSize = ref(50), resultBody = ref<HTMLElement>();
const isConfig = computed(() => props.task.action === "app_config");
const isNtp = computed(() => props.task.action === "ntp");
const isPing = computed(() => props.task.action === "ping");
const showScreenDetail = computed(() => ["ping", "inspect", "mac"].includes(props.task.action));
const visibleTargets = computed(() => props.task.targets.slice((page.value - 1) * pageSize.value, page.value * pageSize.value));
const columns = computed(() => [
  { key: "name", label: "目标屏" }, { key: "ip", label: "IP 地址" }, { key: "state", label: isPing.value ? "在线状态" : "状态" },
  ...(isConfig.value ? [{ key: "save", label: "保存" }, { key: "restart", label: "重启" }, { key: "readback", label: "回读" }] : []),
  ...(isNtp.value ? [{ key: "save", label: "保存" }, { key: "activation", label: "生效" }, { key: "sync", label: "授时" }] : []),
  { key: "message", label: unverifiedInstall.value ? "演示结果 / 处理建议" : "实际结果 / 处理建议" },
  ...(showScreenDetail.value ? [{ key: "detail", label: "查看屏详情" }] : [])
]);
watch(() => props.task.id, () => { page.value = 1; ++logOpenRequest; });
watch(pageSize, () => { page.value = 1; });
watch(() => props.task.targets.length, count => { page.value = Math.min(page.value, Math.max(1, Math.ceil(count / pageSize.value))); });
watch([page, pageSize], () => { if (resultBody.value) resultBody.value.scrollTop = 0; });
const diagnosticText=ref(""),diagnosticOpen=ref(false),diagnosticBusy=ref(false);
async function diagnostics(exportFile=false){diagnosticBusy.value=true;try{const adapter=useScreenAdapter();if(exportFile){const path=await adapter.exportDiagnostics?.(props.task.projectId,props.task.id);if(path)message.success(`诊断已导出：${path}`);}else{diagnosticText.value=await adapter.readDiagnostics?.(props.task.projectId,props.task.id)??"";diagnosticOpen.value=true;}}catch(cause){message.error((cause as Error).message);}finally{diagnosticBusy.value=false;}}
const retryIds = computed(() => props.task.targets.filter((t) => ["failed", "cancelled"].includes(t.state) && !(props.task.action === "install" && (t.result?.device === "succeeded" || ["installed", "verified"].includes(String(t.result?.evidence?.phase)))) && !(props.task.action==='app_config'&&['saved','verified','unchanged'].includes(String(configResult(t)?.save)))).map((t) => t.screenId));
const resultLabels: Record<string,string> = { not_required:"无需更新",pending:"待保存",succeeded:"已完成",failed:"未完成",unknown:"待核实",skipped:"无需执行",cancelled:"已取消" };
function configResult(target:ScreenTaskTarget){return target.result?.evidence?.config as Record<string,unknown>|undefined;}
function ntpResult(target: ScreenTaskTarget) { return target.result?.evidence?.ntp as ScreenNtpEvidence | undefined; }
const ntpStages = ["save", "activation", "sync"] as const;
function ntpResultLabel(target: ScreenTaskTarget, stage: typeof ntpStages[number]) {
  const value = ntpResult(target)?.[stage];
  return value === "succeeded" ? stage === "save" ? "已保存" : stage === "activation" ? "已生效" : "授时已确认" : ntpStageLabel(value);
}
function ntpSummary(target: ScreenTaskTarget) {
  const value = ntpResult(target);
  if (!value) return "";
  const confirmed = value.sync === "succeeded" && value.syncEvidence?.sourceConfirmedBy && value.syncEvidence.sourceConfirmedBy !== "unconfirmed";
  const source = value.syncEvidence?.sourceType === "firmware_default" ? "固件默认" : value.syncEvidence?.server || "未记录";
  return [`服务器：${ntpServerDisplay(value.before?.server)} → ${ntpServerDisplay(value.targetServer)}`, value.after ? `自动校时：${value.after.autoTime ? "开启" : "关闭"}` : "", confirmed ? `已确认授时源：${source}${value.syncEvidence!.clockOffsetSeconds == null ? '' : ` · 偏差 ${value.syncEvidence!.clockOffsetSeconds} 秒`}` : "尚未取得授时证据"].filter(Boolean).join(" · ");
}
const configRestartIds=computed(()=>props.task.action==='app_config'?props.task.targets.filter(target=>{
  const result=configResult(target);return target.state!=='needs_review'&&result&&['saved','verified'].includes(String(result.save))&&result.restartRequired===true&&result.restart!=='succeeded';
}).map(target=>target.screenId):[]);
const configLabels:Record<string,string>={pending:'等待处理',unknown:'待核实',saved:'已保存',verified:'已回读确认',partial:'部分保存',failed:'未完成',unchanged:'值已相同',not_required:'无需重启',not_started:'未重启',not_confirmed:'未确认重启',succeeded:'已完成',different:'与本次设置不同'};
const configStages = ["save", "restart", "readback"] as const;
function configStageLabel(target: ScreenTaskTarget, stage: typeof configStages[number]) {
  const result = configResult(target);
  if (!result) return stage === "restart" ? "按配置判断" : "等待处理";
  const value = String(result[stage] ?? "pending");
  return value === "not_required" && stage !== "restart" ? "无需执行" : configLabels[value] ?? "未开始";
}
function configStageClass(target: ScreenTaskTarget, stage: typeof configStages[number]) {
  const state = String(configResult(target)?.[stage] ?? "pending");
  return ["failed", "different"].includes(state) ? "screen-danger" : ["partial", "unknown", "not_confirmed"].includes(state) ? "screen-warning" : "";
}
function failedFields(target: ScreenTaskTarget) {
  const fields = configResult(target)?.failedFields;
  return Array.isArray(fields) ? fields.filter((field): field is string => typeof field === "string").map(appConfigFieldLabel).join("、") : "";
}
function deviceResultLabel(target:ScreenTaskTarget){
  if(target.result&&['pending','unknown'].includes(target.result.device)){
    if(target.state==='queued')return '等待执行';
    if(target.state==='running')return '执行中';
  }
  return target.result?resultLabels[target.result.device]:'';
}
function pingResult(target: ScreenTaskTarget) {
  // 只使用本次任务保存的实测；当前资产状态可能来自之后的检查。
  const observation = target.result?.observation;
  if (!observation || observation.observedIp !== target.ip) return null;
  return observation.ping === "online" || observation.ping === "offline" ? observation.ping : null;
}
function targetStateLabel(target: ScreenTaskTarget) {
  if (!isPing.value) return displayState(target.state);
  const ping = pingResult(target);
  if (ping) return ping === "online" ? "在线" : "离线";
  if (target.state === "running") return "检查中";
  if (["queued", "cancelled", "skipped"].includes(target.state)) return "未检查";
  return "未知";
}
function targetStateType(target: ScreenTaskTarget): "success" | "error" | "info" | "warning" | "default" {
  if (!isPing.value) return target.state === "succeeded" ? "success" : target.state === "failed" ? "error" : "default";
  const ping = pingResult(target);
  if (ping) return ping === "online" ? "success" : "error";
  if (target.state === "running") return "info";
  return ["queued", "cancelled", "skipped"].includes(target.state) ? "default" : "warning";
}
function targetMessage(target: ScreenTaskTarget) {
  if (!isPing.value || pingResult(target) || ["queued", "running", "cancelled", "skipped"].includes(target.state)) return target.message;
  if (target.result?.observation && target.result.observation.observedIp !== target.ip) return "检查地址与本次目标不一致，请重新检查。";
  const errors = target.result?.observation?.errors.filter(Boolean).join("；");
  if (errors) return [...new Set([target.message, errors].filter(Boolean))].join("；");
  if (target.state === "succeeded") return "未记录本次在线检查结果，请重新检查。";
  return target.message || "未取得在线检查结果，请查看日志后重试。";
}
function resultSummary(target: ScreenTaskTarget) {
  const result = target.result;
  if (!result) return "";
  return [
    ...(!isPing.value && result.device !== "not_required" ? [`设备：${deviceResultLabel(target)}`] : []),
    ...(result.business !== "not_required" ? [`平台数据：${resultLabels[result.business]}`] : []),
    `操作记录：${result.shared === "not_required" ? "保存在本机" : resultLabels[result.shared]}`
  ].join(" · ");
}
const completed = computed(() => props.task.targets.filter((t) => !["queued", "running"].includes(t.state)).length);
const progress = computed(() => Math.round(props.task.targets.reduce((sum, t) => sum + (t.state === "cancelled" ? 100 : t.progress), 0) / Math.max(1, props.task.targets.length)));
const running = computed(() => ["running", "cancelling"].includes(props.task.state));
const unverifiedInstall = computed(() => props.task.mode !== "real" && props.task.action === "install" && Boolean(props.task.input?.apk) && !props.task.input?.appVersion);
function displayState(state: keyof typeof screenStateLabel) {
  if (isPing.value && state === "succeeded") return "检查完成";
  return unverifiedInstall.value && state === "succeeded" ? "演示完成" : screenStateLabel[state];
}
async function cancel() { try { await useScreenAdapter().cancel(props.task.projectId, props.task.id); } catch (cause) { message.error((cause as Error).message); } }
async function verify() { try { await useScreenAdapter().verify(props.task.projectId, props.task.id); message.success(useScreenAdapter().mode === "prototype" ? (unverifiedInstall.value ? "已继续模拟核实，未执行真实安装或更新版本记录" : "已模拟回读核实，未重复执行设备操作") : "已核实并保存结果，未重复执行设备操作"); } catch (cause) { message.error((cause as Error).message); } }
async function openGlobalLogs() {
  const request = ++logOpenRequest, projectId = props.task.projectId, taskId = props.task.id;
  const current = () => request === logOpenRequest && props.task.projectId === projectId && props.task.id === taskId;
  activity.logKeyword = ""; activity.logLevels = [];
  await activity.refreshTasks(projectId, taskId);
  if (!current() || (activity.selectedTaskId && activity.selectedTaskId !== taskId)) return;
  await activity.selectTask(taskId);
  if (!current()) return;
  activity.openPanel("logs"); emit("logsOpened");
}
</script>

<template>
  <section class="screen-task-results" :class="{ 'screen-task-results-config': isConfig || isNtp }">
    <div v-if="task.input?.retryOfOperationId" class="screen-muted">重试来源：{{task.input.retryOfOperationId}}</div>
    <div class="screen-result-header"><div class="screen-result-title"><h3>{{ screenActionLabel(task.action) }}</h3><span class="screen-muted">{{ formatDisplayDateTime(task.createdAt) }} · {{ task.targets.length }} 台<template v-if="task.mode !== 'real'"> · 原型模拟</template></span></div><n-tag :bordered="false" :type="task.state === 'succeeded' ? 'success' : running ? 'info' : 'warning'">{{ displayState(task.state) }}</n-tag></div>
    <div v-if="unverifiedInstall" class="screen-callout" data-testid="screen-install-preview-notice"><strong v-if="task.state === 'succeeded'">安装流程演示完成。</strong>APK 元数据未解析；未执行真实安装，未更新版本记录。</div>
    <div class="screen-task-progress"><n-progress type="line" :percentage="progress" :show-indicator="false" :status="task.state === 'failed' ? 'error' : task.state === 'succeeded' ? 'success' : 'default'" /><b>{{ progress }}%</b><span>{{ completed }} / {{ task.targets.length }} 台已处理</span></div>
    <div class="screen-inline-actions screen-result-actions">
      <n-button v-if="running" size="small" :disabled="task.state === 'cancelling'" @click="cancel">{{ task.state === 'cancelling' ? '正在停止后续操作' : '取消后续执行' }}</n-button>
      <n-button v-if="task.state === 'needs_review'" size="small" type="warning" @click="verify">继续核实结果</n-button>
      <n-button v-if="!running&&configRestartIds.length" size="small" @click="$emit('restart',configRestartIds)">只重启小新 {{configRestartIds.length}} 台</n-button>
      <n-button v-if="!running && retryIds.length && !['status','version_sync','merge'].includes(task.action)" size="small" @click="$emit('retry', retryIds)"><template #icon><RefreshCw :size="14" /></template>重试 {{ retryIds.length }} 台失败 / 未执行设备</n-button>
      <n-button v-if="!running && ['install', 'register', 'inspect', 'version_sync'].includes(task.action)" size="small" type="primary" secondary :disabled="task.state === 'needs_review'" @click="$emit('versions', task.targets.map(target => target.screenId))">核对版本并同步平台</n-button>
      <n-button v-if="task.action === 'ping' || task.action === 'status'" size="small" type="primary" secondary :disabled="running" @click="$emit('status')">查看平台状态差异</n-button>
      <template v-if="task.mode==='real'&&task.action==='diagnostics'&&!running"><n-button size="small" :loading="diagnosticBusy" @click="diagnostics()">查看诊断文件</n-button><n-button size="small" :loading="diagnosticBusy" @click="diagnostics(true)">导出诊断</n-button></template>
      <span class="screen-spacer"></span><n-button size="small" text type="primary" title="查看当前任务日志" @click="openGlobalLogs"><template #icon><FileText :size="14" /></template>任务与日志面板</n-button>
    </div>
    <div v-if="task.state === 'needs_review'" class="screen-callout">{{ unverifiedInstall ? '本次演示结果待核实，请先继续核实结果，不重复提交安装。' : '回读中断不等于动作失败。先核实已有结果，不重复提交、重装或再次重启。' }}</div>
    <div class="screen-result-table-wrap">
      <div class="screen-result-table-header"><table class="screen-table screen-small-table screen-result-table" aria-label="结果列表表头"><colgroup><col v-for="column in columns" :key="column.key" :class="`result-col-${column.key}`" /></colgroup><thead><tr><th v-for="column in columns" :key="column.key">{{ column.label }}</th></tr></thead></table></div>
      <div ref="resultBody" class="screen-table-scroll screen-result-table-body inx-scroll-area"><table class="screen-table screen-small-table screen-result-table" aria-label="智能屏操作结果"><colgroup><col v-for="column in columns" :key="column.key" :class="`result-col-${column.key}`" /></colgroup><tbody><tr v-for="target in visibleTargets" :key="target.screenId" :data-screen-id="target.screenId">
        <td :title="target.name">{{ target.name }}</td><td :title="target.ip">{{ target.ip }}</td>
        <td><n-tag size="small" :bordered="false" :type="targetStateType(target)">{{ targetStateLabel(target) }}</n-tag></td>
        <template v-if="isConfig"><td v-for="stage in configStages" :key="stage" :class="configStageClass(target, stage)" :title="configStageLabel(target, stage)">{{ configStageLabel(target, stage) }}</td></template>
        <template v-if="isNtp"><td v-for="stage in ntpStages" :key="stage" :class="ntpResult(target)?.[stage] === 'failed' ? 'screen-danger' : ntpResult(target)?.[stage] === 'unknown' ? 'screen-warning' : ''" :title="ntpResultLabel(target, stage)">{{ ntpResultLabel(target, stage) }}</td></template>
        <td><div class="screen-result-message" :title="targetMessage(target)">{{ targetMessage(target) }}</div><div v-if="isNtp" class="screen-secondary" :title="ntpSummary(target)">{{ ntpSummary(target) }}</div><div v-if="isConfig && failedFields(target)" class="screen-result-message screen-warning" :title="`未保存字段：${failedFields(target)}`">未保存字段：{{ failedFields(target) }}</div><div v-if="target.result" class="screen-secondary" :title="resultSummary(target)">{{ resultSummary(target) }}</div></td>
        <td v-if="showScreenDetail"><n-button size="tiny" text type="primary" title="查看本机保存的最新屏详情" @click="emit('detail', target.screenId)">查看屏详情</n-button></td>
      </tr><tr v-if="!task.targets.length"><td :colspan="columns.length" class="screen-muted">暂无设备结果</td></tr></tbody></table></div>
    </div>
    <div class="screen-result-pagination"><span class="screen-muted">共 {{ task.targets.length }} 台 · 已处理 {{ completed }} 台</span><n-pagination v-model:page="page" v-model:page-size="pageSize" :item-count="task.targets.length" :page-sizes="[20,50,100]" :page-slot="5" size="small" show-size-picker /></div>
  </section>
  <n-modal v-model:show="diagnosticOpen"><n-card title="本机智能屏诊断" closable style="width:min(1000px,94vw)" @close="diagnosticOpen=false"><pre style="max-height:70vh;overflow:auto;white-space:pre-wrap;word-break:break-word">{{diagnosticText}}</pre></n-card></n-modal>
</template>

<style scoped>
.screen-task-results{display:flex;flex:1;flex-direction:column;gap:8px;min-width:0;min-height:0;overflow:hidden}
.screen-result-header{flex:none;gap:8px}.screen-result-title{display:flex;align-items:baseline;gap:12px;min-width:0;flex-wrap:wrap}.screen-result-header h3{margin:0;font-size:14px}.screen-result-header .screen-muted{font-size:11px}
.screen-task-progress{flex:none;gap:10px;margin:0;min-height:24px}.screen-task-progress b{font-size:12px;min-width:3ch;text-align:right}
.screen-task-results .screen-result-actions{flex:none;gap:8px;margin:0;min-height:28px}
.screen-task-results>.screen-callout{flex:none;margin:0;padding:6px 10px;font-size:12px}
.screen-result-table-wrap{display:flex;flex:1;flex-direction:column;min-width:0;min-height:0;border:1px solid var(--inx-color-border);border-radius:6px;overflow:hidden}
.screen-result-table-header{flex:none;padding-right:4px;background:var(--inx-color-table-header)}
.screen-task-results .screen-result-table-body{flex:1;min-height:0;max-height:none;border:0;border-radius:0;overflow-y:auto;overflow-x:hidden}
.screen-result-table{width:100%;table-layout:fixed}.screen-result-table .result-col-name{width:20%}.screen-result-table .result-col-ip{width:118px}.screen-result-table .result-col-state{width:100px}
.screen-task-results-config .result-col-name{width:15%}.screen-task-results-config .result-col-ip{width:110px}
.result-col-save{width:74px}.result-col-restart{width:84px}.result-col-readback{width:104px}
.result-col-activation{width:74px}.result-col-sync{width:100px}
.result-col-detail{width:96px}
.screen-result-table th{padding:6px 8px;font-size:10px;line-height:19px}
.screen-result-table td{height:var(--inx-table-row-height);padding:4px 8px;line-height:20px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;vertical-align:middle}
.screen-result-table .screen-secondary{margin-top:1px;max-width:none;font-size:10px;line-height:15px}
.screen-result-message{overflow:hidden;white-space:nowrap;text-overflow:ellipsis}.screen-result-message.screen-warning{font-size:11px;line-height:16px}
.screen-result-pagination{display:flex;flex:none;align-items:center;justify-content:space-between;gap:8px;min-height:28px;font-size:11px}
</style>
