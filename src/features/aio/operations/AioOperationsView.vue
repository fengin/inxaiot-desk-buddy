<script setup lang="ts">
import {
  NAlert,
  NButton,
  NCheckbox,
  NDropdown,
  NInput,
  NInputNumber,
  NModal,
  NPagination,
  NProgress,
  NRadio,
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
  CircleAlert,
  ChevronDown,
  ChevronRight,
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
import { commandErrorCode, commandErrorText } from "@/shared/api/errors";
import { formatDisplayLocalPath } from "@/shared/format/localPath";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import DeploymentPreflightGroups from "./DeploymentPreflightGroups.vue";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import type { OperationMode } from "@/shared/model/demo";
import type {
  DeploymentTaskTargetView,
  OperationHistoryItem,
  DeploymentPreflightCheck
} from "@/shared/model/deploymentWorkflow";
import type {
  DeploymentPlanInput,
  DeploymentImageInput
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
const preflightHasWarnings = computed(() => workflow.preflight?.checks.some((check) => check.status === "warning") ?? false);
const preflightSnapshotMissing = computed(() =>
  workflow.preflight?.ready === true
  && (!workflow.preflightTaskId || !workflow.preflight.executionSnapshot)
);
const preflightSubmissionReady = computed(() =>
  workflow.preflight?.ready === true
  && Boolean(workflow.preflightTaskId)
  && Boolean(workflow.preflight.executionSnapshot)
);
const mode = ref<OperationMode>("full_upgrade");
const step = ref(0);
const selectedMacs = ref<string[]>([]);
const serviceName = ref("");
const imageSelections = ref<Record<string, {
  filePath: string;
  imageTag: string;
  repoTags: string[];
}>>({});
const batchSize = ref(4);
const concurrency = ref(2);
const targetSearch = ref("");
const historyOpen = ref(false);
const selectedHistoryId = ref<string>();
const resultTaskId = ref<string>();
const operationProjectId = ref<string>();
const checking = ref(false);
const executing = ref(false);
const preflightTaskId = ref<string>();
const preflightProgressCurrent = ref(0);
const preflightProgressTotal = ref(0);
const preflightProgressMessage = ref("正在准备部署检查");
let taskRefreshTimer: number | undefined;
let taskEventRefreshTimer: number | undefined;
let projectContextMounted = false;
let projectContextRequest = 0;
let preflightUiRequest = 0;
let operationSubmissionRequest = 0;

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
const composeServices = computed(() => release.profile?.composeServices ?? []);
const selectedImageCount = computed(() => composeServices.value.filter((service) => imageSelections.value[service.name]?.filePath).length);
const maxBatchSize = computed(() => Math.max(1, Math.min(20, selectedMacs.value.length || 1)));
const maxConcurrency = computed(() => Math.max(1, Math.min(5, batchSize.value, selectedMacs.value.length || 1)));
const estimatedPreflightWorkTotal = computed(() =>
  3 + selectedMacs.value.length * (mode.value === "service_upgrade" ? 1 : 2)
);
const preflightProgressPercent = computed(() => {
  if (preflightProgressTotal.value <= 0) return 0;
  return Math.min(100, Math.round(
    preflightProgressCurrent.value * 100 / preflightProgressTotal.value
  ));
});
const serviceOptions = computed(() => composeServices.value.map((service) => ({
  label: service.name,
  value: service.name
})));
const selectedServiceRows = computed(() => mode.value === "service_upgrade"
  ? composeServices.value.filter((service) => service.name === serviceName.value)
  : composeServices.value
);
const modeHint = computed(() => {
  if (mode.value === "first_deploy") {
    return "一体机还没有部署过软件，第一次部署整体软件包";
  }
  if (mode.value === "full_upgrade") {
    return "一体机已实施部署过软件，调整过 docker-compose 或环境变量等，需要整体升级";
  }
  return "单服务镜像升级，只需要升级对应的服务镜像包";
});
const artifactLabel = computed(() => {
  if (mode.value === "service_upgrade") {
    return imageSelections.value[serviceName.value]?.imageTag || serviceName.value + " 镜像";
  }
  return `${selectedImageCount.value}/${composeServices.value.length} 个服务镜像`;
});

watch(composeServices, (services) => {
  const next: typeof imageSelections.value = {};
  for (const service of services) {
    next[service.name] = imageSelections.value[service.name] ?? {
      filePath: "",
      imageTag: "",
      repoTags: []
    };
  }
  imageSelections.value = next;
  if (!services.some((service) => service.name === serviceName.value)) {
    serviceName.value = services[0]?.name ?? "";
  }
}, { immediate: true });
watch(
  [() => selectedMacs.value.length, () => batchSize.value, () => concurrency.value],
  ([targetCount, currentBatch, currentConcurrency]) => {
    if (!targetCount) return;
    const nextBatch = Math.max(1, Math.min(currentBatch, 20, targetCount));
    if (nextBatch !== batchSize.value) batchSize.value = nextBatch;
    const nextConcurrency = Math.max(1, Math.min(currentConcurrency, 5, nextBatch, targetCount));
    if (nextConcurrency !== concurrency.value) concurrency.value = nextConcurrency;
  }
);
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
    if (["succeeded", "partially_succeeded", "failed", "cancelled", "interrupted", "check_failed", "finalizing_failed"].includes(state ?? "")) {
      step.value = 3;
      const projectId = operationProjectId.value;
      if (projectId && projects.activeProjectId === projectId) void activity.refreshTasks(projectId);
    }
  }
);
watch(
  () => activity.lastEvent,
  (event) => {
    if (!event) return;
    if (checking.value && event.localTaskId === preflightTaskId.value) {
      const total = event.progressTotal;
      const current = event.progressCurrent;
      if (typeof total === "number" && total > 0) {
        preflightProgressTotal.value = total;
        if (typeof current === "number") {
          preflightProgressCurrent.value = Math.max(0, Math.min(current, total));
        }
        if (event.message) preflightProgressMessage.value = event.message;
      }
      activity.selectedTaskId = event.localTaskId;
    }
    if (event.localTaskId !== resultTaskId.value) return;
    if (taskEventRefreshTimer !== undefined) window.clearTimeout(taskEventRefreshTimer);
    taskEventRefreshTimer = window.setTimeout(() => void refreshSubmittedTask(), 80);
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
  if (taskEventRefreshTimer !== undefined) window.clearTimeout(taskEventRefreshTimer);
});

async function activateProjectContext(projectId?: string) {
  const request = ++projectContextRequest;
  preflightUiRequest += 1;
  operationSubmissionRequest += 1;
  checking.value = false;
  executing.value = false;
  workflow.bindProject(projectId);
  selectedHistoryId.value = undefined;
  historyOpen.value = false;
  imageSelections.value = {};
  selectedMacs.value = [];
  targetSearch.value = "";
  preflightTaskId.value = undefined;
  preflightProgressCurrent.value = 0;
  preflightProgressTotal.value = 0;
  preflightProgressMessage.value = "正在准备部署检查";
  if (operationProjectId.value !== projectId) {
    step.value = 0;
    resultTaskId.value = undefined;
    operationProjectId.value = undefined;
    workflow.clearTask();
  }
  if (!projectId || !projects.isReady) return;
  await Promise.all([
    aio.refresh(projectId),
    aio.loadSelectionNodes(projectId),
    release.load(projectId),
    activity.start(projectId)
  ]);
  if (request !== projectContextRequest || projectId !== projects.activeProjectId) return;
  if (await restoreActiveDeployment(projectId)) return;
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

async function restoreActiveDeployment(projectId: string) {
  const task = activity.tasks.find((item) =>
    item.domainType === "aio"
    && ["first_deploy", "full_upgrade", "service_upgrade"].includes(item.operationType)
    && ["queued", "running", "cancelling", "finalizing_failed"].includes(item.state)
  );
  if (!task) return false;
  resultTaskId.value = task.id;
  operationProjectId.value = projectId;
  mode.value = task.operationType as OperationMode;
  try {
    const restored = await workflow.loadTask(projectId, task.id);
    step.value = restored.state === "finalizing_failed" ? 3 : 2;
    if (step.value === 2) scheduleTaskRefresh();
    message.info(`已恢复本项目活动任务：${task.id}`);
    return true;
  } catch {
    resultTaskId.value = undefined;
    operationProjectId.value = undefined;
    return false;
  }
}

function nodeName(mac: string) {
  const normalized = mac.replace(/[:\s-]/g, "").toUpperCase();
  return eligibleNodes.value.find((node) => node.macNormalized === normalized)?.name
    ?? aio.nodes.find((node) => node.macNormalized === normalized)?.name
    ?? mac;
}

function stageLabel(stage?: string) {
  if (!stage) return "正在准备";
  const labels: Record<string, string> = {
    queued: "排队中", checking: "检查中", ready: "待执行",
    ssh_connect: "连接一体机", ssh_connected: "连接完成",
    prepare_config: "读取发布配置", prepare_artifact: "准备镜像文件",
    prepare_release: "生成发布包", prepare_target: "准备一体机配置",
    prepare_lease: "确认任务可执行", prepare_lease_failed: "任务执行条件确认失败",
    prepare_remote: "准备一体机", prepare_local: "准备发布文件",
    upload: "上传发布文件", precheck: "检查运行环境", backup: "备份现有数据",
    install: "安装发布包", health: "检查服务状态", register: "确认平台注册",
    service_check: "检查目标服务", service_upgrade: "升级目标服务", service_health: "检查目标服务状态",
    remote_complete: "一体机操作完成", finalizing: "保存执行结果", completed: "已完成",
    cancelling: "正在取消", cancelled: "已取消", failed: "执行失败",
    preparation_failed: "准备失败", interrupted: "执行中断"
  };
  return labels[stage] ?? (/[㐀-鿿]/u.test(stage) ? stage : "处理中");
}

function targetResultText(target: DeploymentTaskTargetView) {
  if (target.message) return target.message;
  return target.state === "succeeded" ? stageLabel(target.stage) : historyResultLabel(target.state);
}

function toggleNode(mac: string, checked: boolean) {
  if (checked) selectedMacs.value = [...new Set([...selectedMacs.value, mac])];
  else selectedMacs.value = selectedMacs.value.filter((item) => item !== mac);
}

function selectParsedImageTag(service: string, event: FocusEvent) {
  if ((imageSelections.value[service]?.repoTags.length ?? 0) > 0
    && event.target instanceof HTMLInputElement) {
    event.target.select();
  }
}

function chooseImageTag(service: string, key: string | number) {
  const tag = String(key);
  const selection = imageSelections.value[service];
  if (selection?.repoTags.includes(tag)) selection.imageTag = tag;
}

async function chooseImage(service: string) {
  if (!adapter.real) {
    const current = imageSelections.value[service] ?? { filePath: "", imageTag: "", repoTags: [] };
    const path = current.filePath || `C:/fixture/${service}.tar`;
    const inspection = await adapter.inspectImage(path, current.imageTag || undefined);
    imageSelections.value[service] = {
      filePath: path,
      imageTag: current.imageTag || inspection.archive.repoTags[0] || "",
      repoTags: inspection.archive.repoTags
    };
    return;
  }
  const selected = await dialogs.selectFile(`选择 ${service} 的Docker镜像归档`, [
    { name: "Docker镜像归档", extensions: ["tar"] }
  ]);
  if (!selected) return;
  try {
    const inspection = await adapter.inspectImage(selected);
    const current = imageSelections.value[service];
    imageSelections.value[service] = {
      filePath: inspection.archive.path,
      imageTag: current?.imageTag && inspection.archive.repoTags.includes(current.imageTag)
        ? current.imageTag
        : inspection.archive.repoTags[0] ?? "",
      repoTags: inspection.archive.repoTags
    };
  } catch (error) {
    message.error(commandErrorText(error, "镜像文件检查失败"));
  }
}

async function runCheck() {
  if (checking.value) return;
  const projectId = projects.activeProjectId;
  if (!projectId) {
    message.warning("请先选择项目");
    return;
  }
  if (!projects.isReady) {
    message.warning("请先完成项目连接和登录");
    return;
  }
  if (!selectedMacs.value.length) {
    message.warning("请至少选择一台一体机");
    return;
  }
  if (!release.profile || !composeServices.value.length) {
    message.warning("请先在发布参数中配置并保存 docker-compose.yml");
    return;
  }
  const invalid = selectedServiceRows.value.find((service) => {
    const selected = imageSelections.value[service.name];
    return !selected?.filePath || !selected.imageTag;
  });
  if (invalid) {
    message.warning(`请为服务 ${invalid.name} 选择镜像文件并填写镜像标签`);
    return;
  }
  const taskId = crypto.randomUUID();
  const request = ++preflightUiRequest;
  preflightTaskId.value = taskId;
  preflightProgressCurrent.value = 0;
  preflightProgressTotal.value = estimatedPreflightWorkTotal.value;
  preflightProgressMessage.value = "正在创建部署检查任务";
  activity.selectedTaskId = taskId;
  checking.value = true;
  try {
    preflightProgressMessage.value = "正在检查公共发布配置";
    const report = await workflow.runPreflight(
      projectId,
      taskId,
      buildPlanInput()
    );
    if (request !== preflightUiRequest || projectId !== projects.activeProjectId) return;
    step.value = 1;
    if (report.ready && report.executionSnapshot) {
      message.success("部署检查通过");
    } else if (report.ready) {
      message.warning("本次检查结果不完整，请重新检查");
    } else {
      const blockers = report.checks.filter((check) => check.blocking && check.status === "failed");
      message.warning("检查发现" + blockers.length + "处问题，请处理后重试");
    }
  } catch (error) {
    if (request !== preflightUiRequest || projectId !== projects.activeProjectId) return;
    message.error(commandErrorText(error, "部署检查失败"));
  } finally {
    if (request === preflightUiRequest && projectId === projects.activeProjectId) {
      await activity.refreshTasks(projectId);
      if (activity.tasks.some((task) => task.id === taskId)) {
        activity.selectedTaskId = taskId;
        await activity.refreshLogs();
      }
      checking.value = false;
    }
  }
}

async function startOperation() {
  if (executing.value) return;
  const projectId = projects.activeProjectId;
  if (!projectId || workflow.preflightProjectId !== projectId) {
    message.warning("项目已切换，请在当前项目重新执行预检");
    return;
  }
  const checkedTaskId = workflow.preflightTaskId;
  const executionSnapshot = workflow.preflight?.executionSnapshot;
  if (!workflow.preflight?.ready || !checkedTaskId || !executionSnapshot) {
    message.warning("本次检查结果不完整，请重新检查");
    return;
  }
  const request = ++operationSubmissionRequest;
  executing.value = true;
  try {
    const submission = await workflow.submit(
      projectId,
      checkedTaskId,
      executionSnapshot
    );
    if (request !== operationSubmissionRequest || projects.activeProjectId !== projectId) return;
    operationProjectId.value = projectId;
    resultTaskId.value = submission.taskId;
    step.value = 2;
    await activity.refreshTasks(projectId);
    if (request !== operationSubmissionRequest || projects.activeProjectId !== projectId) return;
    if (activity.tasks.some((task) => task.id === submission.taskId)) {
      await activity.selectTask(submission.taskId);
    }
    scheduleTaskRefresh();
    message.success("部署任务已提交：" + submission.taskId);
  } catch (error) {
    if (request !== operationSubmissionRequest || projects.activeProjectId !== projectId) return;
    const errorText = commandErrorText(error, "部署任务提交失败");
    message.error(errorText);
    await activity.refreshTasks(projectId);
    if (request !== operationSubmissionRequest || projects.activeProjectId !== projectId) return;
    if (activity.tasks.some((task) => task.id === checkedTaskId)) {
      await activity.selectTask(checkedTaskId);
      activity.openPanel("logs");
    }
    if (commandErrorCode(error) === "NOT_FOUND") {
      workflow.clearPreflight();
      preflightTaskId.value = undefined;
      step.value = 0;
      message.warning("原检查记录已不存在，请重新检查后提交");
    } else {
      step.value = 1;
    }
  } finally {
    if (request === operationSubmissionRequest) executing.value = false;
  }
}

function buildPlanInput(): DeploymentPlanInput {
  const imageFiles: DeploymentImageInput[] = selectedServiceRows.value.map((service) => ({
    serviceName: service.name,
    filePath: imageSelections.value[service.name]?.filePath ?? "",
    imageTag: imageSelections.value[service.name]?.imageTag ?? ""
  }));
  return {
    mode: mode.value,
    targetMacs: [...selectedMacs.value],
    imageFiles,
    batchSize: batchSize.value ?? 1,
    concurrency: concurrency.value ?? 1
  };
}

async function handleRemediation(check: DeploymentPreflightCheck) {
  const remediation = check.remediation;
  if (!remediation) return;
  if (remediation.action === "select_artifact") {
    step.value = 0;
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
  preflightTaskId.value = undefined;
}

function scheduleTaskRefresh() {
  if (taskRefreshTimer !== undefined) window.clearTimeout(taskRefreshTimer);
  taskRefreshTimer = window.setTimeout(async () => {
    await refreshSubmittedTask();
    if (step.value === 2) scheduleTaskRefresh();
  }, 5_000);
}

function resetFlow() {
  step.value = 0;
  resultTaskId.value = undefined;
  operationProjectId.value = undefined;
  workflow.clearTask();
  workflow.clearPreflight();
  preflightTaskId.value = undefined;
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
    draft: "准备中",
    checking: "检查中",
    check_failed: "检查失败",
    ready: "待执行",
    queued: "排队中",
    pending: "待执行",
    running: "执行中",
    cancelling: "取消中",
    succeeded: "成功",
    partially_succeeded: "部分成功",
    failed: "失败",
    cancelled: "已取消",
    interrupted: "已中断",
    finalizing_failed: "结果待补写",
    unknown: "状态未知",
    skipped: "已跳过"
  }[state] ?? "状态未知";
}

function stateTone(state: string): "success" | "warning" | "error" | "info" {
  if (state === "succeeded") return "success";
  if (state === "partially_succeeded" || state === "cancelling" || state === "cancelled" || state === "interrupted") return "warning";
  if (state === "check_failed" || state === "failed" || state === "unknown" || state === "finalizing_failed") return "error";
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
      <div class="operation-flow-bar">
        <header class="operation-mode-bar">
          <strong>部署模式</strong>
          <n-radio-group v-model:value="mode" class="operation-mode-options" :disabled="step > 0">
            <n-radio class="operation-mode-option" value="first_deploy" data-testid="operation-mode-first">首次部署</n-radio>
            <n-radio class="operation-mode-option" value="full_upgrade" data-testid="operation-mode-full">整包升级</n-radio>
            <n-radio class="operation-mode-option" value="service_upgrade" data-testid="operation-mode-service">单服升级</n-radio>
          </n-radio-group>
        </header>

        <div class="process-stepper">
          <template v-for="(item, index) in steps" :key="item.title">
            <button type="button" class="process-step" data-action-owner="self" :class="{ active: step === index, done: step > index, 'result-success': index === steps.length - 1 && step === index && resultState === 'succeeded', 'result-warning': index === steps.length - 1 && step === index && resultState !== 'succeeded' }" :disabled="index > step" @click="index < step && (step = index)">
              <b><Check v-if="step > index || (index === steps.length - 1 && step === index && resultState === 'succeeded')" :size="14" /><CircleAlert v-else-if="index === steps.length - 1 && step === index" :size="14" /><span v-else>{{ index + 1 }}</span></b>
              <span class="process-step-copy">
                <span class="process-step-heading"><strong>{{ item.title }}</strong><i v-if="index < steps.length - 1" class="step-line" :class="{ done: step > index }"></i></span>
                <small>{{ item.hint }}</small>
              </span>
            </button>
          </template>
        </div>
      </div>

      <div class="operation-content" :class="{ 'selection-scroll-owner': step === 0 }">
        <section v-if="step === 0" class="operation-stage select-stage">
          <div class="stage-main">
            <header class="stage-heading"><div><span class="feature-icon info"><PackageCheck :size="20" /></span><span><strong>选择{{ modeLabel }}范围</strong><small class="operation-mode-hint">{{ modeHint }}</small></span></div></header>
            <div class="selection-toolbar"><span>目标一体机</span><b>已选 <strong class="metric-number metric-info" data-testid="selected-node-count">{{ selectedMacs.length }}</strong> / 可选 <strong class="metric-number metric-operation" data-testid="eligible-node-count">{{ eligibleNodes.length }}</strong> 台</b><n-input v-model:value="targetSearch" class="target-search" size="tiny" clearable placeholder="搜索全部节点" /><n-button size="tiny" quaternary @click="selectedMacs = visibleEligibleNodes.map((node) => node.mac)">选择全部匹配</n-button><n-button size="tiny" quaternary @click="selectedMacs = visibleEligibleNodes.filter((node) => node.platformState === 'online').map((node) => node.mac)">选择匹配在线</n-button><n-button size="tiny" quaternary @click="selectedMacs = []">清空</n-button></div>
            <div v-if="aio.selectionLoading" class="empty-state">正在读取项目完整节点选择集…</div>
            <div class="node-selection-list">
              <label v-for="node in visibleEligibleNodes" :key="node.mac" :class="{ selected: selectedMacs.includes(node.mac) }">
                <n-checkbox :checked="selectedMacs.includes(node.mac)" @update:checked="toggleNode(node.mac, $event)" />
                <strong class="node-selection-name" :title="node.name">{{ node.name }}</strong>
                <span class="node-selection-ip" :title="node.ip">{{ node.ip }}</span>
                <span class="node-selection-location" :title="node.location">{{ node.location || '—' }}</span>
                <n-tag size="small" :bordered="false" :type="node.platformState === 'online' ? 'success' : node.platformState === 'offline' ? 'error' : 'default'">{{ node.platformState === 'online' ? '在线' : node.platformState === 'offline' ? '离线' : '未知' }}</n-tag>
              </label>
            </div>
          </div>
          <aside class="stage-settings">
            <h3>本次执行设置</h3>
            <n-alert v-if="!release.profile || !composeServices.length" type="warning" :show-icon="true" class="compose-missing-alert">
              请先在发布参数中配置并保存 docker-compose.yml
              <template #action><n-button size="tiny" @click="router.push('/aio/release')">前往配置</n-button></template>
            </n-alert>
            <label v-if="mode === 'service_upgrade'">目标服务<n-select v-model:value="serviceName" size="small" :options="serviceOptions" :disabled="!serviceOptions.length" /></label>
            <div v-if="selectedServiceRows.length" class="service-image-list" data-testid="operation-service-images">
              <div v-for="service in selectedServiceRows" :key="service.name" class="service-image-row">
                <strong>{{ service.name }}</strong>
                <div class="service-image-fields">
                  <div class="image-file-picker">
                    <n-input :value="formatDisplayLocalPath(imageSelections[service.name]?.filePath)" size="tiny" readonly placeholder="请选择镜像文件" :title="formatDisplayLocalPath(imageSelections[service.name]?.filePath)" />
                    <n-button size="tiny" secondary :data-testid="`operation-image-select-${service.name}`" @click="chooseImage(service.name)">选择</n-button>
                  </div>
                  <div class="image-tag-picker">
                    <n-input
                      v-model:value="imageSelections[service.name].imageTag"
                      size="tiny"
                      :readonly="!imageSelections[service.name]?.filePath || (imageSelections[service.name]?.repoTags.length ?? 0) > 0"
                      :placeholder="imageSelections[service.name]?.filePath ? '未解析到标签，请手工填写' : '选择镜像后自动解析标签'"
                      :title="imageSelections[service.name]?.imageTag"
                      :data-testid="`operation-image-tag-${service.name}`"
                      @focus="selectParsedImageTag(service.name, $event)"
                    />
                    <n-dropdown
                      v-if="(imageSelections[service.name]?.repoTags.length ?? 0) > 1"
                      trigger="click"
                      :options="imageSelections[service.name].repoTags.map((tag) => ({ label: tag, key: tag }))"
                      @select="chooseImageTag(service.name, $event)"
                    >
                      <n-button size="tiny" secondary data-action-owner="dropdown" :aria-label="`切换 ${service.name} 镜像标签`"><template #icon><ChevronDown /></template></n-button>
                    </n-dropdown>
                  </div>
                </div>
              </div>
            </div>
            <div class="form-grid compact execution-tuning"><label><span>一批台数</span><n-input-number v-model:value="batchSize" data-testid="operation-batch-size" size="small" :min="1" :max="maxBatchSize" :disabled="!selectedMacs.length" /></label><label><span>并发数</span><n-input-number v-model:value="concurrency" data-testid="operation-concurrency" size="small" :min="1" :max="maxConcurrency" :disabled="!selectedMacs.length" /></label></div>
            <div class="execution-summary">
              <span><Server :size="16" /><strong class="metric-number metric-info" data-testid="summary-target-count">{{ selectedMacs.length }}</strong> 台目标</span>
              <span v-if="mode === 'service_upgrade'"><Archive :size="16" />{{ artifactLabel }}</span>
              <span v-else><Archive :size="16" /><strong class="metric-number" :class="selectedImageCount === composeServices.length && composeServices.length ? 'metric-operation' : 'metric-warning'" data-testid="summary-image-count">{{ selectedImageCount }}/{{ composeServices.length }}</strong> 个服务镜像</span>
              <span><Clock3 :size="16" />预计 <strong class="metric-number metric-warning" data-testid="summary-estimate">8～12</strong> 分钟</span>
            </div>
            <div v-if="checking" class="preflight-progress" data-testid="operation-preflight-progress" aria-live="polite">
              <div><span>{{ preflightProgressMessage }}</span><strong data-testid="operation-preflight-progress-count">已完成 {{ preflightProgressCurrent }}/{{ preflightProgressTotal }} 项</strong></div>
              <n-progress type="line" :percentage="preflightProgressPercent" :show-indicator="false" :height="5" processing />
            </div>
            <n-button class="full-button" type="primary" data-testid="operation-preflight" :loading="checking" :disabled="checking || !selectedMacs.length" @click="runCheck"><template #icon><ShieldCheck /></template>检查执行条件</n-button>
          </aside>
        </section>

        <section v-else-if="step === 1" class="operation-stage check-stage" data-testid="operation-preflight-report">
          <header class="stage-heading"><div><span class="feature-icon operation"><ShieldCheck :size="20" /></span><span><strong>部署检查</strong><small>先检查公共发布配置，再逐台检查连接和运行环境</small></span></div><n-tag size="small" class="preflight-status-tag" :type="!workflow.preflight?.ready ? 'error' : preflightHasWarnings ? 'warning' : 'success'" :bordered="false"><span class="preflight-status-label">{{ !workflow.preflight?.ready ? '有问题待处理' : preflightHasWarnings ? '可继续（有提示）' : '全部通过' }}</span></n-tag></header>
          <DeploymentPreflightGroups :report="workflow.preflight" :nodes="eligibleNodes" @remediate="handleRemediation" />
          <div class="render-preview"><span><FileArchive :size="20" /></span><div><strong>本次发布</strong><small class="render-preview-summary"><template v-if="workflow.preflight?.normalizedPlan.mode !== 'service_upgrade'"><strong class="metric-number metric-operation" data-testid="preflight-summary-image-count">{{ workflow.preflight?.normalizedPlan.imageFiles.length ?? 0 }}</strong> 个服务镜像</template><template v-else>{{ workflow.preflight?.normalizedPlan.artifactName }}</template> {{ workflow.preflight?.normalizedPlan.artifactVersion }} · <strong class="metric-number metric-info" data-testid="preflight-summary-target-count">{{ workflow.preflight?.normalizedPlan.targetMacs.length ?? 0 }}</strong> 台一体机 · 并发 <strong class="metric-number metric-warning" data-testid="preflight-summary-concurrency">{{ workflow.preflight?.normalizedPlan.concurrency ?? concurrency }}</strong> 台</small></div></div>
          <footer class="stage-footer"><span v-if="preflightSnapshotMissing" class="preflight-submit-warning" data-testid="operation-preflight-snapshot-warning">本次检查结果不完整，请返回重新检查</span><n-button size="small" @click="step = 0">返回调整</n-button><n-button size="small" class="operation-button" data-testid="operation-submit" :loading="executing" :disabled="executing || !preflightSubmissionReady" @click="startOperation"><template #icon><Play /></template>{{ executing ? "正在创建部署任务" : "提交部署任务" }}</n-button></footer>
        </section>

        <section v-else-if="step === 2" class="operation-stage execute-stage" data-testid="operation-execution">
          <div class="execution-overview">
            <div class="execution-overview__heading">
              <div class="execution-overview__identity"><span class="feature-icon operation pulse"><UploadCloud :size="20" /></span><span><strong :title="`正在${modeLabel}`">正在{{ modeLabel }}</strong><small :title="`${stageLabel(currentTask?.stage)} · 完整过程保存在当前电脑`">{{ stageLabel(currentTask?.stage) }} · 完整过程保存在当前电脑</small><small data-testid="operation-task-id" :title="resultTaskId">{{ resultTaskId }}</small></span></div>
              <div class="execution-overview__meter">
                <span class="execution-overview__result-count" data-testid="operation-execution-result-count">{{ currentTask?.completedCount ?? 0 }}/{{ currentTask?.targetCount ?? 0 }} 台形成最终结果</span>
                <strong class="execution-overview__percentage" data-testid="operation-execution-percentage">{{ currentTask?.progress ?? 0 }}%</strong>
                <div class="execution-overview__progress" data-testid="operation-execution-progress"><n-progress type="line" :percentage="currentTask?.progress ?? 0" :show-indicator="false" :height="6" processing /></div>
              </div>
              <n-tag class="execution-task-state" :type="stateTone(currentTask?.state ?? 'running')" :bordered="false">{{ historyResultLabel(currentTask?.state ?? 'running') }}</n-tag>
            </div>
          </div>
          <div v-if="currentTask?.targets.length" class="execution-nodes"><div v-for="(target, index) in currentTask.targets" :key="target.mac" :class="{ active: target.state === 'running', done: target.state === 'succeeded' }"><span><b>{{ index + 1 }}</b><strong>{{ nodeName(target.mac) }}</strong><small>{{ target.mac }}</small></span><span>{{ stageLabel(target.stage) }} · {{ target.progress }}%</span><n-tag class="execution-target-state" size="small" :bordered="false" :type="stateTone(target.state)">{{ historyResultLabel(target.state) }}</n-tag></div></div>
          <div v-else class="empty-state">任务已提交，正在读取本地任务记录和执行进度。</div>
          <footer class="stage-footer"><span>切换项目不会停止当前任务，可在底部任务与日志面板持续查看。</span><n-button v-if="currentTask?.cancellable" size="small" type="error" secondary data-testid="operation-cancel" @click="cancelCurrentTask">请求取消</n-button><n-button size="small" secondary @click="openCurrentLogs">打开完整日志</n-button></footer>
        </section>

        <section v-else class="operation-stage result-stage" data-testid="operation-result">
          <div class="result-hero" :class="{ 'result-not-success': resultState !== 'succeeded' }"><span class="result-icon" :class="resultState === 'succeeded' ? 'success' : 'warning'"><CheckCircle2 v-if="resultState === 'succeeded'" :size="32" /><CircleAlert v-else :size="32" /></span><div><strong class="result-hero-title">{{ modeLabel }}已形成最终结果</strong><p class="result-hero-summary">成功 {{ resultSuccessCount }} 台，失败 {{ resultFailureCount }} 台，取消 {{ resultCancelledCount }} 台。</p></div><n-tag :type="stateTone(resultState)" :bordered="false">{{ historyResultLabel(resultState) }}</n-tag></div>
          <div class="result-metrics"><span><small>目标数量</small><strong>{{ resultTargetCount }}</strong></span><span><small>成功</small><strong class="success-text">{{ resultSuccessCount }}</strong></span><span><small>失败/异常</small><strong>{{ resultFailureCount }}</strong></span><span><small>发布版本</small><strong :title="artifactLabel">{{ artifactLabel }}</strong></span></div>
          <div v-if="resultTargets.length" class="result-list"><div v-for="target in resultTargets" :key="target.mac" :class="{ 'result-not-success': target.state !== 'succeeded' }"><CheckCircle2 v-if="target.state === 'succeeded'" :size="17" /><CircleAlert v-else :size="17" /><span><strong>{{ nodeName(target.mac) }}</strong><small>{{ target.mac }}</small></span><span>{{ targetResultText(target) }}</span><n-tag size="small" :type="stateTone(target.state)" :bordered="false">{{ historyResultLabel(target.state) }}</n-tag></div></div>
          <div v-else class="empty-state">没有可验证的节点最终结果，未按成功处理。</div>
          <p v-if="resultState === 'finalizing_failed'" class="modal-description">任务执行已结束，但结果记录尚未完整保存；工作台已保留本次任务数据，重新启动后会自动补写。</p>
          <footer class="stage-footer"><n-button size="small" secondary @click="openHistory">查看操作记录</n-button><n-button v-if="['failed', 'cancelled', 'interrupted', 'check_failed'].includes(resultState)" size="small" secondary @click="retryOperation"><template #icon><RotateCcw /></template>按当前参数重新检查</n-button><n-button size="small" type="primary" :disabled="resultState === 'finalizing_failed'" @click="resetFlow"><template #icon><RotateCcw /></template>创建下一次任务</n-button></footer>
        </section>
      </div>
    </section>

    <n-modal :show="historyOpen" preset="card" :title="selectedHistory ? '部署升级详情' : '部署升级历史'" class="history-modal" data-testid="operation-history" :bordered="false" @update:show="!$event && closeHistory()">
      <template v-if="!selectedHistory">
        <p class="modal-description">其他电脑可以查看操作结果；完整过程和日志仅保存在发起任务的电脑。</p>
        <div v-if="history.length" class="history-list">
          <button v-for="record in history" :key="record.id" type="button" @click="selectHistory(record)">
            <span class="history-icon" :class="record.state === 'succeeded' ? 'success' : 'warning'"><History :size="17" /></span>
            <span class="history-main"><strong>{{ record.operationName }} · {{ historyArtifact(record) }}</strong><small>{{ record.id }} · {{ record.operatorName }} · 目标 {{ record.targetCount }} / 成功 {{ record.successCount }} / 失败 {{ record.failureCount }}</small></span>
            <n-tag size="small" :bordered="false" :type="stateTone(record.state)">{{ historyResultLabel(record.state) }}</n-tag>
            <time>{{ formatDisplayDateTime(record.endedAt ?? record.startedAt) }}</time><ChevronRight :size="16" />
          </button>
        </div>
        <n-pagination v-if="workflow.history.total > workflow.history.pageSize" :page="workflow.history.page" :page-size="workflow.history.pageSize" :item-count="workflow.history.total" size="small" @update:page="changeHistoryPage" />
        <div v-if="!history.length" class="empty-state">{{ workflow.historyLoading ? '正在读取共享操作历史…' : '当前项目没有共享部署操作记录。' }}</div>
      </template>
      <div v-else class="history-detail">
        <n-button size="tiny" quaternary @click="backHistory"><template #icon><ArrowLeft /></template>返回历史列表</n-button>
        <div class="history-detail__summary">
          <span class="history-icon" :class="selectedHistory.state === 'succeeded' ? 'success' : 'warning'"><History :size="19" /></span>
          <div><strong>{{ selectedHistory.operationName }} · {{ historyArtifact(selectedHistory) }}</strong><small>{{ selectedHistory.id }}</small></div>
          <n-tag :bordered="false" :type="stateTone(selectedHistory.state)">{{ historyResultLabel(selectedHistory.state) }}</n-tag>
        </div>
        <dl class="history-detail__facts"><dt>操作人员</dt><dd>{{ selectedHistory.operatorName }}</dd><dt>发起电脑</dt><dd>{{ selectedHistory.instanceId }}</dd><dt>完成时间</dt><dd>{{ formatDisplayDateTime(selectedHistory.endedAt, '尚未完成') }}</dd><dt>执行范围</dt><dd>目标 {{ selectedHistory.targetCount }}，成功 {{ selectedHistory.successCount }}，失败 {{ selectedHistory.failureCount }}，取消 {{ selectedHistory.cancelledCount }}</dd><dt>发布内容</dt><dd>{{ historyArtifact(selectedHistory) }}</dd></dl>
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

<style scoped>
.compose-missing-alert {
  margin-bottom: 10px;
}

.service-image-list {
  display: grid;
  gap: 8px;
}

.service-image-row {
  display: grid;
  gap: 5px;
  padding: 8px;
  border: 1px solid var(--inx-color-border);
  border-radius: var(--inx-radius-sm);
  background: var(--inx-color-surface-subtle);
}

.service-image-row > strong {
  font-size: 12px;
}

.service-image-fields {
  display: grid;
  min-width: 0;
  grid-template-columns: minmax(0, .9fr) minmax(0, 1.1fr);
  gap: 6px;
}

.image-file-picker {
  display: grid;
  min-width: 0;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 6px;
}

.image-tag-picker {
  display: grid;
  min-width: 0;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 4px;
}

.execution-tuning label {
  display: grid;
  min-width: 0;
  grid-template-columns: auto minmax(0, 1fr);
  align-items: center;
  gap: 8px;
}

.execution-tuning label > span {
  white-space: nowrap;
}

.metric-number {
  font-style: normal;
  font-variant-numeric: tabular-nums;
  font-weight: 700;
}

.metric-info {
  color: var(--inx-color-info);
}

.metric-operation {
  color: var(--inx-color-operation);
}

.metric-warning {
  color: var(--inx-color-warning);
}

.preflight-status-tag {
  display: inline-flex;
  height: 26px;
  min-height: 26px;
  align-items: center;
  justify-content: center;
  padding-block: 0;
  line-height: 1;
  vertical-align: middle;
}

.preflight-submit-warning {
  margin-right: auto;
  color: var(--inx-color-warning);
  font-size: 12px;
}

.preflight-status-tag :deep(.n-tag__content) {
  display: inline-flex;
  min-height: 24px;
  align-self: stretch;
  align-items: center;
  justify-content: center;
  line-height: 1;
}

.preflight-status-label {
  display: inline-flex;
  min-height: 24px;
  align-items: center;
  justify-content: center;
  box-sizing: border-box;
  padding-top: 1px;
  line-height: 1;
}

</style>
