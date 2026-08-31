<script setup lang="ts">
import {
  NButton,
  NCheckbox,
  NInput,
  NInputNumber,
  NModal,
  NPagination,
  NProgress,
  NRadioButton,
  NRadioGroup,
  NSelect,
  NSpace,
  NTag,
  useMessage
} from "naive-ui";
import {
  Archive,
  ArrowLeft,
  Check,
  CheckCircle2,
  ChevronRight,
  CircleAlert,
  Clock3,
  FileArchive,
  History,
  PackageCheck,
  Play,
  RotateCcw,
  Server,
  ShieldCheck,
  UploadCloud
} from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";

import { useOperationsAdapter } from "@/shared/api/operationsAdapter";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import type { OperationMode } from "@/shared/model/demo";
import type {
  DeploymentTaskTargetView,
  OperationHistoryItem,
  DeploymentPreflightCheck
} from "@/shared/model/deploymentWorkflow";
import type {
  DeploymentPlanInput,
  ReleaseValidation,
  ServiceImageInspection
} from "@/shared/model/release";
import { useActivityStore } from "@/stores/activity";
import { useAioNodesStore } from "@/stores/aioNodes";
import { useProjectStore } from "@/stores/projects";
import { useReleaseProfileStore } from "@/stores/releaseProfile";
import { useDeploymentWorkflowStore } from "@/stores/deploymentWorkflow";

const message = useMessage();
const route = useRoute();
const router = useRouter();
const adapter = useOperationsAdapter();
const dialogs = useSystemDialogAdapter();
const projects = useProjectStore();
const aio = useAioNodesStore();
const release = useReleaseProfileStore();
const activity = useActivityStore();
const workflow = useDeploymentWorkflowStore();
const mode = ref<OperationMode>("full_upgrade");
const step = ref(0);
const selectedMacs = ref<string[]>([]);
const artifactPath = ref("");
const serviceName = ref("device-edge");
const batchSize = ref(4);
const concurrency = ref(2);
const targetSearch = ref("");
const historyOpen = ref(false);
const selectedHistoryId = ref<string>();
const resultTaskId = ref<string>();
const operationProjectId = ref<string>();
const releaseValidation = ref<ReleaseValidation>();
const imageInspection = ref<ServiceImageInspection>();
const checking = ref(false);
const executing = ref(false);
let taskRefreshTimer: number | undefined;
let projectContextMounted = false;
let projectContextRequest = 0;

const steps = [
  { title: "选择", hint: "模式、范围和发布文件" },
  { title: "检查", hint: "配置、连接和执行条件" },
  { title: "部署/升级", hint: "上传、执行和服务检查" },
  { title: "结果", hint: "节点结果和共享记录" }
];

const currentTask = computed(() => {
  return workflow.currentTask?.id === resultTaskId.value
    && workflow.currentTaskProjectId === operationProjectId.value
    ? workflow.currentTask
    : undefined;
});
const history = computed(() => workflow.history.items);
const selectedHistory = computed(() => workflow.historyDetail?.operation);
const eligibleNodes = computed(() =>
  (aio.selectionNodes.length ? aio.selectionNodes : aio.nodes)
    .filter((node) => node.managementState !== "conflict")
);
const visibleEligibleNodes = computed(() => {
  const keyword = targetSearch.value.trim().toLocaleLowerCase();
  if (!keyword) return eligibleNodes.value;
  return eligibleNodes.value.filter((node) =>
    [node.name, node.ip, node.mac, node.location]
      .some((value) => value.toLocaleLowerCase().includes(keyword))
  );
});
const modeLabel = computed(() => mode.value === "first_deploy" ? "首次部署" : mode.value === "full_upgrade" ? "整包升级" : "单服升级");
const artifactLabel = computed(() => {
  if (mode.value === "service_upgrade") {
    return imageInspection.value?.archive.repoTags[0] ?? serviceName.value + " 镜像";
  }
  return releaseValidation.value?.manifest
    ? "Release " + releaseValidation.value.manifest.version
    : "未选择Release";
});
const artifactCheckSummary = computed(() => {
  if (mode.value === "service_upgrade") {
    return (imageInspection.value?.archive.repoTags.length ?? 0) + " 个RepoTag";
  }
  return (releaseValidation.value?.images.length ?? 0) + " 个镜像声明一致";
});
const resultTargets = computed<DeploymentTaskTargetView[]>(() => {
  if (currentTask.value?.targets.length) return currentTask.value.targets;
  return [];
});
const resultSuccessCount = computed(
  () => currentTask.value?.successCount ?? 0
);
const resultFailureCount = computed(
  () => currentTask.value?.failureCount ?? 0
);
const resultCancelledCount = computed(
  () => currentTask.value?.cancelledCount ?? 0
);
const resultTargetCount = computed(() => currentTask.value?.targetCount ?? resultTargets.value.length);
const resultState = computed(() => {
  if (currentTask.value?.state) return currentTask.value.state;
  if (resultFailureCount.value > 0 && resultSuccessCount.value > 0) return "partially_succeeded";
  if (resultFailureCount.value > 0) return "failed";
  if (resultCancelledCount.value > 0 && resultSuccessCount.value === 0) return "cancelled";
  return resultSuccessCount.value > 0 ? "succeeded" : "unknown";
});

watch(
  () => currentTask.value?.state,
  (state) => {
    if (["succeeded", "partially_succeeded", "failed", "cancelled", "interrupted", "check_failed", "finalizing_failed"].includes(state ?? "")) step.value = 3;
  }
);

onMounted(async () => {
  await projects.initialize();
  projectContextMounted = true;
  await activateProjectContext(projects.activeProjectId);
});
watch(
  () => projects.activeProjectId,
  (projectId, previous) => {
    if (!projectContextMounted || projectId === previous) return;
    void activateProjectContext(projectId);
  }
);
onBeforeUnmount(() => {
  projectContextMounted = false;
  projectContextRequest += 1;
  if (taskRefreshTimer !== undefined) window.clearTimeout(taskRefreshTimer);
});

async function activateProjectContext(projectId?: string) {
  const request = ++projectContextRequest;
  workflow.bindProject(projectId);
  selectedHistoryId.value = undefined;
  historyOpen.value = false;
  releaseValidation.value = undefined;
  imageInspection.value = undefined;
  selectedMacs.value = [];
  targetSearch.value = "";
  if (!projectId || !projects.isReady) return;
  await Promise.all([
    aio.refresh(projectId),
    aio.loadSelectionNodes(projectId),
    release.load(projectId),
    activity.start(projectId)
  ]);
  if (request !== projectContextRequest || projectId !== projects.activeProjectId) return;
  const routedTarget = typeof route.query.target === "string" ? route.query.target : "";
  const routedNode = eligibleNodes.value.find(
    (node) => node.mac === routedTarget && node.managementState !== "conflict"
  );
  selectedMacs.value = routedNode
    ? [routedNode.mac]
    : eligibleNodes.value
        .filter((node) => node.managementState === "managed" && node.platformState === "online")
        .slice(0, 4)
        .map((node) => node.mac);
}

function nodeName(mac: string) {
  return eligibleNodes.value.find((node) => node.mac === mac)?.name
    ?? aio.nodes.find((node) => node.mac === mac)?.name
    ?? mac;
}

function toggleNode(mac: string, checked: boolean) {
  if (checked) selectedMacs.value = [...new Set([...selectedMacs.value, mac])];
  else selectedMacs.value = selectedMacs.value.filter((item) => item !== mac);
}

async function chooseArtifact() {
  if (!adapter.real) {
    message.info("浏览器 Fixture 不读取本机文件；Tauri 模式使用真实文件选择器");
    return;
  }
  const selected = mode.value === "service_upgrade"
    ? await dialogs.selectFile("选择Docker镜像归档", [
        { name: "Docker镜像归档", extensions: ["tar"] }
      ])
    : await dialogs.selectDirectory("选择本地Release目录");
  if (!selected) return;
  artifactPath.value = selected;
  releaseValidation.value = undefined;
  imageInspection.value = undefined;
}

async function runCheck() {
  const projectId = projects.activeProjectId;
  if (!projectId) {
    message.warning("请先选择项目");
    return;
  }
  if (!selectedMacs.value.length) {
    message.warning("请至少选择一台一体机");
    return;
  }
  if (!artifactPath.value) {
    message.warning("请先选择本地发布文件");
    return;
  }
  checking.value = true;
  try {
    const inspection = await adapter.inspectArtifact(mode.value, artifactPath.value);
    releaseValidation.value = inspection.releaseValidation;
    imageInspection.value = inspection.imageInspection;
    if (releaseValidation.value && !releaseValidation.value.valid) {
      throw new Error(releaseValidation.value.errors.join("；"));
    }
    const report = await workflow.runPreflight(
      projectId,
      buildPlanInput()
    );
    if (projectId !== projects.activeProjectId) return;
    step.value = 1;
    if (report.ready) {
      message.success("所有真实执行条件检查通过");
    } else {
      const blockers = report.checks.filter((check) => check.blocking && check.status === "failed");
      message.warning("预检发现" + blockers.length + "项阻断，请按整改入口处理");
    }
  } catch (error) {
    message.error(error instanceof Error ? error.message : "发布文件校验失败");
  } finally {
    checking.value = false;
  }
}

async function startOperation() {
  const projectId = projects.activeProjectId;
  if (!projectId || workflow.preflightProjectId !== projectId) {
    message.warning("项目已切换，请在当前项目重新执行预检");
    return;
  }
  executing.value = true;
  step.value = 2;
  try {
    const submission = await workflow.submit(
      projectId,
      workflow.preflight?.normalizedPlan ?? buildPlanInput()
    );
    operationProjectId.value = projectId;
    resultTaskId.value = submission.taskId;
    await activity.refreshTasks(projectId);
    scheduleTaskRefresh();
    message.success("部署任务已提交：" + submission.taskId);
  } catch (error) {
    step.value = 1;
    message.error(error instanceof Error ? error.message : "部署任务提交失败");
  } finally {
    executing.value = false;
  }
}

function buildPlanInput(): DeploymentPlanInput {
  return {
    mode: mode.value,
    targetMacs: selectedMacs.value,
    artifactPath: artifactPath.value,
    artifactName: artifactLabel.value,
    artifactVersion: releaseValidation.value?.manifest?.version ?? "",
    serviceName: mode.value === "service_upgrade" ? serviceName.value : undefined,
    imageName: imageInspection.value?.archive.repoTags[0],
    batchSize: batchSize.value ?? 1,
    concurrency: concurrency.value ?? 1
  };
}

async function handleRemediation(check: DeploymentPreflightCheck) {
  const remediation = check.remediation;
  if (!remediation) return;
  if (remediation.action === "select_artifact") {
    step.value = 0;
    await chooseArtifact();
    return;
  }
  if (remediation.route) {
    await router.push({
      path: remediation.route,
      query: remediation.target ? { target: remediation.target } : undefined
    });
    return;
  }
  message.warning(remediation.label + "：" + check.message);
}

async function refreshSubmittedTask() {
  const projectId = operationProjectId.value;
  if (!projectId) return;
  if (projects.activeProjectId === projectId) await activity.refreshTasks(projectId);
  if (!resultTaskId.value) return;
  try {
    const task = await workflow.loadTask(projectId, resultTaskId.value);
    if (["succeeded", "partially_succeeded", "failed", "cancelled", "interrupted", "check_failed", "finalizing_failed"].includes(task.state)) {
      if (taskRefreshTimer !== undefined) window.clearTimeout(taskRefreshTimer);
      taskRefreshTimer = undefined;
    }
  } catch {
    if (workflow.error) message.error(workflow.error);
  }
}

async function cancelCurrentTask() {
  if (!resultTaskId.value || !operationProjectId.value) return;
  await activity.refreshTasks(operationProjectId.value);
  await activity.selectTask(resultTaskId.value);
  await activity.cancelSelectedTask();
  await refreshSubmittedTask();
  if (activity.error) message.error(activity.error);
  else message.warning("已请求取消，等待当前安全步骤收敛");
}

async function openCurrentLogs() {
  if (resultTaskId.value && operationProjectId.value) {
    await activity.refreshTasks(operationProjectId.value);
    await activity.selectTask(resultTaskId.value);
  }
  activity.openPanel("logs");
}

function retryOperation() {
  step.value = 0;
  resultTaskId.value = undefined;
  operationProjectId.value = undefined;
  workflow.clearTask();
  workflow.clearPreflight();
}

function scheduleTaskRefresh() {
  if (taskRefreshTimer !== undefined) window.clearTimeout(taskRefreshTimer);
  taskRefreshTimer = window.setTimeout(async () => {
    await refreshSubmittedTask();
    if (step.value === 2) scheduleTaskRefresh();
  }, 500);
}

function resetFlow() {
  step.value = 0;
  resultTaskId.value = undefined;
  operationProjectId.value = undefined;
  workflow.clearTask();
  workflow.clearPreflight();
}

async function openHistory() {
  selectedHistoryId.value = undefined;
  workflow.clearHistoryDetail();
  historyOpen.value = true;
  const projectId = projects.activeProjectId;
  if (!projectId) return;
  try {
    await workflow.loadHistory(projectId);
  } catch {
    if (workflow.error) message.error(workflow.error);
  }
}

async function changeHistoryPage(page: number) {
  const projectId = projects.activeProjectId;
  if (!projectId) return;
  try {
    await workflow.loadHistory(projectId, { page, pageSize: workflow.history.pageSize });
  } catch {
    if (workflow.error) message.error(workflow.error);
  }
}

async function selectHistory(record: OperationHistoryItem) {
  selectedHistoryId.value = record.id;
  const projectId = projects.activeProjectId;
  if (!projectId) return;
  try {
    await workflow.loadHistoryDetail(projectId, record.id);
  } catch {
    selectedHistoryId.value = undefined;
    if (workflow.error) message.error(workflow.error);
  }
}

function closeHistory() {
  historyOpen.value = false;
  selectedHistoryId.value = undefined;
  workflow.clearHistoryDetail();
}

function backHistory() {
  selectedHistoryId.value = undefined;
  workflow.clearHistoryDetail();
}

function historyResultLabel(state: string) {
  return {
    succeeded: "成功",
    partially_succeeded: "部分成功",
    failed: "失败",
    cancelled: "已取消",
    interrupted: "已中断",
    finalizing_failed: "最终化待重试",
    running: "执行中"
  }[state] ?? state;
}

function stateTone(state: string): "success" | "warning" | "error" | "info" {
  if (state === "succeeded") return "success";
  if (state === "partially_succeeded" || state === "cancelled" || state === "interrupted") return "warning";
  if (state === "failed" || state === "unknown" || state === "finalizing_failed") return "error";
  return "info";
}

function historyArtifact(record: OperationHistoryItem) {
  return [record.artifactName, record.artifactVersion].filter(Boolean).join(" ") || "未记录发布物";
}
</script>

<template>
  <section class="workspace-page operations-page" data-testid="aio-operations">
    <header class="page-header">
      <div><h1>部署升级</h1></div>
      <div class="page-actions"><n-button size="small" secondary @click="openHistory"><template #icon><History /></template>查看历史记录</n-button></div>
    </header>

    <section class="operation-shell">
      <header class="operation-mode-bar">
        <div><strong>操作模式</strong><span>选择本次要完成的实施动作</span></div>
        <n-radio-group v-model:value="mode" size="small" :disabled="step > 0">
          <n-radio-button value="first_deploy" data-testid="operation-mode-first">首次部署</n-radio-button>
          <n-radio-button value="full_upgrade" data-testid="operation-mode-full">整包升级</n-radio-button>
          <n-radio-button value="service_upgrade" data-testid="operation-mode-service">单服升级</n-radio-button>
        </n-radio-group>
      </header>

      <div class="process-stepper">
        <template v-for="(item, index) in steps" :key="item.title">
          <button type="button" class="process-step" data-action-owner="self" :class="{ active: step === index, done: step > index }" :disabled="index > step" @click="index < step && (step = index)">
            <b><Check v-if="step > index" :size="14" /><span v-else>{{ index + 1 }}</span></b><span><strong>{{ item.title }}</strong><small>{{ item.hint }}</small></span>
          </button>
          <i v-if="index < steps.length - 1" class="step-line" :class="{ done: step > index }"></i>
        </template>
      </div>

      <div class="operation-content">
        <section v-if="step === 0" class="operation-stage select-stage">
          <div class="stage-main">
            <header class="stage-heading"><div><span class="feature-icon info"><PackageCheck :size="20" /></span><span><strong>选择{{ modeLabel }}范围</strong><small>当前选择只形成本机任务，执行后记录最终结果</small></span></div></header>
            <div class="selection-toolbar"><span>目标一体机</span><b>已选 {{ selectedMacs.length }} / 可选 {{ eligibleNodes.length }} 台</b><n-input v-model:value="targetSearch" size="tiny" clearable placeholder="搜索全部节点" /><n-button size="tiny" quaternary @click="selectedMacs = visibleEligibleNodes.map((node) => node.mac)">选择全部匹配</n-button><n-button size="tiny" quaternary @click="selectedMacs = visibleEligibleNodes.filter((node) => node.platformState === 'online').map((node) => node.mac)">选择匹配在线</n-button><n-button size="tiny" quaternary @click="selectedMacs = []">清空</n-button></div>
            <div v-if="aio.selectionLoading" class="empty-state">正在读取项目完整节点选择集…</div>
            <div class="node-selection-list">
              <label v-for="node in visibleEligibleNodes" :key="node.mac" :class="{ selected: selectedMacs.includes(node.mac) }">
                <n-checkbox :checked="selectedMacs.includes(node.mac)" @update:checked="toggleNode(node.mac, $event)" />
                <span><strong>{{ node.name }}</strong><small>{{ node.ip }} · {{ node.location }}</small></span>
                <n-tag size="small" :bordered="false" :type="node.platformState === 'online' ? 'success' : node.platformState === 'offline' ? 'error' : 'default'">{{ node.platformState === 'online' ? '在线' : node.platformState === 'offline' ? '离线' : '未知' }}</n-tag>
              </label>
            </div>
          </div>
          <aside class="stage-settings">
            <h3>本次执行设置</h3>
            <label v-if="mode === 'service_upgrade'">目标服务<n-select v-model:value="serviceName" size="small" :options="[{ label: 'device-edge', value: 'device-edge' }, { label: 'rule-engine', value: 'rule-engine' }, { label: 'device-edge-web', value: 'device-edge-web' }]" /></label>
            <label>{{ mode === 'service_upgrade' ? '本地镜像文件' : '本地Release目录' }}<div class="path-input"><n-input v-model:value="artifactPath" data-testid="operation-artifact-path" size="small" /><n-button size="small" secondary @click="chooseArtifact">选择</n-button></div></label>
            <div class="form-grid compact"><label>一批台数<n-input-number v-model:value="batchSize" size="small" :min="1" :max="20" /></label><label>并发数<n-input-number v-model:value="concurrency" size="small" :min="1" :max="5" /></label></div>
            <div class="execution-summary"><span><Server :size="16" />{{ selectedMacs.length }} 台目标</span><span><Archive :size="16" />{{ artifactLabel }}</span><span><Clock3 :size="16" />预计 8～12 分钟</span></div>
            <n-button class="full-button" type="primary" data-testid="operation-preflight" :loading="checking" :disabled="!selectedMacs.length" @click="runCheck"><template #icon><ShieldCheck /></template>检查执行条件</n-button>
          </aside>
        </section>

        <section v-else-if="step === 1" class="operation-stage check-stage" data-testid="operation-preflight-report">
          <header class="stage-heading"><div><span class="feature-icon operation"><ShieldCheck :size="20" /></span><span><strong>执行条件检查</strong><small>项目、制品、资产、租约、HostKey和远端门禁统一返回真实状态</small></span></div><n-tag :type="workflow.preflight?.ready ? 'success' : 'error'" :bordered="false">{{ workflow.preflight?.ready ? '全部通过' : '存在阻断' }}</n-tag></header>
          <div class="check-grid">
            <div v-for="check in workflow.preflight?.checks ?? []" :key="check.code + ':' + (check.targetMac ?? '')" class="check-item" :class="check.status === 'passed' ? 'success' : check.status === 'warning' ? 'warning' : 'error'">
              <CheckCircle2 v-if="check.status === 'passed'" :size="18" /><CircleAlert v-else :size="18" />
              <span><strong>{{ check.label }}<em v-if="check.targetMac"> · {{ check.targetMac }}</em></strong><small>{{ check.message }}</small></span>
              <n-button v-if="check.remediation" size="tiny" quaternary @click="handleRemediation(check)">{{ check.remediation.label }}</n-button>
              <b v-else>{{ check.status === 'passed' ? '通过' : check.status === 'warning' ? '提示' : '阻断' }}</b>
            </div>
            <template v-if="!workflow.preflight">
            <div class="check-item" :class="release.profile ? 'success' : 'error'"><CheckCircle2 :size="18" /><span><strong>项目发布参数</strong><small>{{ release.profile ? `版本 ${release.profile.version} · 已从项目库读取` : '当前项目尚未创建发布参数' }}</small></span><b>{{ release.profile ? '通过' : '阻断' }}</b></div>
            <div class="check-item warning"><CircleAlert :size="18" /><span><strong>资源租约</strong><small>提交任务时从工作台数据库原子获取，冲突会明确阻断</small></span><b>待执行</b></div>
            <div class="check-item warning"><CircleAlert :size="18" /><span><strong>SSH与HostKey</strong><small>提交任务后使用已确认指纹执行真实认证，不预设成功</small></span><b>待执行</b></div>
            <div class="check-item warning"><CircleAlert :size="18" /><span><strong>Docker与Compose</strong><small>提交任务后由远端Agent返回实际结果</small></span><b>待执行</b></div>
            <div class="check-item success"><CheckCircle2 :size="18" /><span><strong>本地发布文件</strong><small>{{ artifactLabel }} · {{ artifactCheckSummary }}</small></span><b>通过</b></div>
            <div class="check-item" :class="projects.isReady ? 'success' : 'error'"><CheckCircle2 :size="18" /><span><strong>项目上下文</strong><small>{{ projects.activeProject?.statusMessage ?? '当前项目未就绪' }}</small></span><b>{{ projects.isReady ? '通过' : '阻断' }}</b></div>
            </template>
          </div>
          <div class="render-preview"><span><FileArchive :size="20" /></span><div><strong>归一化执行计划</strong><small>{{ workflow.preflight?.normalizedPlan.artifactName }} {{ workflow.preflight?.normalizedPlan.artifactVersion }} · 发布参数版本 {{ workflow.preflight?.profileVersion ?? '—' }}</small></div></div>
          <footer class="stage-footer"><n-button size="small" @click="step = 0">返回调整</n-button><n-button size="small" class="operation-button" data-testid="operation-submit" :loading="executing" :disabled="!workflow.preflight?.ready" @click="startOperation"><template #icon><Play /></template>提交部署任务</n-button></footer>
        </section>

        <section v-else-if="step === 2" class="operation-stage execute-stage" data-testid="operation-execution">
          <div class="execution-overview">
            <div class="execution-overview__heading">
              <div class="execution-overview__identity"><span class="feature-icon operation pulse"><UploadCloud :size="20" /></span><span><strong>正在{{ modeLabel }}</strong><small>{{ currentTask?.stage ?? '等待真实Task建立' }} · 完整过程保存在当前实例</small><small data-testid="operation-task-id">{{ resultTaskId }}</small></span></div>
              <div class="execution-overview__progress"><div><strong>{{ currentTask?.progress ?? 0 }}%</strong><span>{{ currentTask?.completedCount ?? 0 }}/{{ currentTask?.targetCount ?? 0 }} 台形成最终结果</span></div><n-progress type="line" :percentage="currentTask?.progress ?? 0" :show-indicator="false" :height="6" /></div>
              <n-tag :type="stateTone(currentTask?.state ?? 'running')" :bordered="false">{{ historyResultLabel(currentTask?.state ?? 'running') }}</n-tag>
            </div>
          </div>
          <div v-if="currentTask?.targets.length" class="execution-nodes"><div v-for="(target, index) in currentTask.targets" :key="target.mac" :class="{ active: target.state === 'running', done: target.state === 'succeeded' }"><span><b>{{ index + 1 }}</b><strong>{{ nodeName(target.mac) }}</strong><small>{{ target.mac }}</small></span><span>{{ target.stage }} · {{ target.progress }}%</span><n-tag size="small" :bordered="false" :type="stateTone(target.state)">{{ historyResultLabel(target.state) }}</n-tag></div></div>
          <div v-else class="empty-state">任务已提交，正在等待本地Task记录和目标进度。</div>
          <footer class="stage-footer"><span>切换项目不会停止当前任务，可在底部任务与日志面板持续查看。</span><n-button v-if="currentTask?.cancellable" size="small" type="error" secondary data-testid="operation-cancel" @click="cancelCurrentTask">请求取消</n-button><n-button size="small" secondary @click="openCurrentLogs">打开完整日志</n-button></footer>
        </section>

        <section v-else class="operation-stage result-stage" data-testid="operation-result">
          <div class="result-hero"><span class="result-icon" :class="resultState === 'succeeded' ? 'success' : 'warning'"><CheckCircle2 :size="32" /></span><div><strong>{{ modeLabel }}已形成最终结果</strong><p>成功 {{ resultSuccessCount }} 台，失败 {{ resultFailureCount }} 台，取消 {{ resultCancelledCount }} 台；以下统计只来自真实Task或执行摘要。</p></div><n-tag :type="stateTone(resultState)" :bordered="false">{{ historyResultLabel(resultState) }}</n-tag></div>
          <div class="result-metrics"><span><small>目标数量</small><strong>{{ resultTargetCount }}</strong></span><span><small>成功</small><strong class="success-text">{{ resultSuccessCount }}</strong></span><span><small>失败/异常</small><strong>{{ resultFailureCount }}</strong></span><span><small>发布版本</small><strong>{{ artifactLabel }}</strong></span></div>
          <div v-if="resultTargets.length" class="result-list"><div v-for="target in resultTargets" :key="target.mac"><CheckCircle2 :size="17" /><span><strong>{{ nodeName(target.mac) }}</strong><small>{{ target.mac }}</small></span><span>{{ target.message ?? target.stage }}</span><n-tag size="small" :type="stateTone(target.state)" :bordered="false">{{ historyResultLabel(target.state) }}</n-tag></div></div>
          <div v-else class="empty-state">没有可验证的节点最终结果，未按成功处理。</div>
          <p v-if="resultState === 'finalizing_failed'" class="modal-description">项目侧原子最终化或本地投影尚未安全收敛，任务制品已保留；重启工作台会在确认共享操作终态后自动重试本地投影。</p>
          <footer class="stage-footer"><n-button size="small" secondary @click="openHistory">查看操作记录</n-button><n-button v-if="['failed', 'cancelled', 'interrupted', 'check_failed'].includes(resultState)" size="small" secondary @click="retryOperation"><template #icon><RotateCcw /></template>按当前参数重新检查</n-button><n-button size="small" type="primary" :disabled="resultState === 'finalizing_failed'" @click="resetFlow"><template #icon><RotateCcw /></template>创建下一次任务</n-button></footer>
        </section>
      </div>
    </section>

    <n-modal :show="historyOpen" preset="card" :title="selectedHistory ? '部署升级详情' : '部署升级历史'" class="history-modal" data-testid="operation-history" :bordered="false" @update:show="!$event && closeHistory()">
      <template v-if="!selectedHistory">
        <p class="modal-description">项目侧共享操作记录保存摘要和节点最终结果；完整步骤、输出和日志仅保留在发起实例。</p>
        <div v-if="history.length" class="history-list">
          <button v-for="record in history" :key="record.id" type="button" @click="selectHistory(record)">
            <span class="history-icon" :class="record.state === 'succeeded' ? 'success' : 'warning'"><History :size="17" /></span>
            <span class="history-main"><strong>{{ record.operationName }} · {{ historyArtifact(record) }}</strong><small>{{ record.id }} · {{ record.operatorName }} · 目标 {{ record.targetCount }} / 成功 {{ record.successCount }} / 失败 {{ record.failureCount }}</small></span>
            <n-tag size="small" :bordered="false" :type="stateTone(record.state)">{{ historyResultLabel(record.state) }}</n-tag>
            <time>{{ record.endedAt ?? record.startedAt }}</time><ChevronRight :size="16" />
          </button>
        </div>
        <n-pagination v-if="workflow.history.total > workflow.history.pageSize" :page="workflow.history.page" :page-size="workflow.history.pageSize" :item-count="workflow.history.total" size="small" @update:page="changeHistoryPage" />
        <div v-else class="empty-state">{{ workflow.historyLoading ? '正在读取共享操作历史…' : '当前项目没有共享部署操作记录。' }}</div>
      </template>
      <div v-else class="history-detail">
        <n-button size="tiny" quaternary @click="backHistory"><template #icon><ArrowLeft /></template>返回历史列表</n-button>
        <div class="history-detail__summary">
          <span class="history-icon" :class="selectedHistory.state === 'succeeded' ? 'success' : 'warning'"><History :size="19" /></span>
          <div><strong>{{ selectedHistory.operationName }} · {{ historyArtifact(selectedHistory) }}</strong><small>{{ selectedHistory.id }}</small></div>
          <n-tag :bordered="false" :type="stateTone(selectedHistory.state)">{{ historyResultLabel(selectedHistory.state) }}</n-tag>
        </div>
        <dl class="history-detail__facts"><dt>操作人员</dt><dd>{{ selectedHistory.operatorName }}</dd><dt>发起实例</dt><dd>{{ selectedHistory.instanceId }}</dd><dt>完成时间</dt><dd>{{ selectedHistory.endedAt ?? '尚未完成' }}</dd><dt>执行范围</dt><dd>目标 {{ selectedHistory.targetCount }}，成功 {{ selectedHistory.successCount }}，失败 {{ selectedHistory.failureCount }}，取消 {{ selectedHistory.cancelledCount }}</dd><dt>发布内容</dt><dd>{{ historyArtifact(selectedHistory) }}</dd></dl>
        <div class="history-detail__nodes">
          <header><strong>节点最终结果</strong><span>项目侧记录</span></header>
          <div v-for="target in workflow.historyDetail?.targets ?? []" :key="target.resourceType + ':' + target.resourceKey">
            <span><strong>{{ nodeName(target.resourceKey) }}</strong><small>{{ target.resourceKey }}</small></span><span>{{ target.resultSummary ?? target.errorSummary ?? '未记录摘要' }}</span><n-tag size="small" :bordered="false" :type="stateTone(target.state)">{{ historyResultLabel(target.state) }}</n-tag>
          </div>
          <div v-if="!(workflow.historyDetail?.targets.length)" class="empty-state">此操作没有节点最终结果记录。</div>
        </div>
      </div>
      <template #footer><n-space justify="end"><n-button size="small" @click="closeHistory">关闭</n-button></n-space></template>
    </n-modal>
  </section>
</template>
