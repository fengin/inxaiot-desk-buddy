<script setup lang="ts">
import { NButton, NEmpty, NInput, NPopconfirm, NProgress, NSelect, NTag, useMessage } from "naive-ui";
import { ChevronDown, ListRestart, RotateCw, Trash2, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import type { ActivityTask, ActivityTaskState } from "@/shared/model/activity";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import { useActivityStore } from "@/stores/activity";
import { useProjectStore } from "@/stores/projects";

const activity = useActivityStore();
const projects = useProjectStore();
const message = useMessage();
const logLines = ref<HTMLElement>();
const followLatest = ref(true);
const orderedTasks = computed(() => [...activity.tasks].sort((a, b) => {
  const rank: Partial<Record<ActivityTaskState, number>> = {
    checking: 0,
    running: 1,
    cancelling: 2,
    queued: 3,
    finalizing_failed: 4
  };
  return (rank[a.state] ?? 5) - (rank[b.state] ?? 5);
}));
const levelOptions = [
  { label: "INFO", value: "INFO" },
  { label: "WARN", value: "WARN" },
  { label: "ERROR", value: "ERROR" }
];
const clearActionEnabled = computed(() =>
  activity.panelTab === "tasks" ? activity.canClearFinishedTasks : activity.canClearSelectedTaskLogs
);
const clearActionTitle = computed(() =>
  activity.panelTab === "tasks" ? "清空已结束任务记录" : "清空当前任务日志"
);
const clearConfirmation = computed(() =>
  activity.panelTab === "tasks"
    ? "清空已结束的任务记录？正在执行和等待执行的任务不会受影响。"
    : "清空当前任务的全部日志内容？此操作不可恢复。"
);

function tagType(task: ActivityTask) {
  if (task.state === "succeeded") return "success";
  if (task.state === "partially_succeeded" || task.state === "finalizing_failed") return "warning";
  if (task.state === "failed" || task.state === "interrupted") return "error";
  return "info";
}

function stateLabel(task: ActivityTask) {
  const labels: Record<ActivityTaskState, string> = {
    draft: "草稿",
    checking: "检查中",
    check_failed: "检查失败",
    ready: "等待入队",
    queued: "等待执行",
    running: "执行中",
    cancelling: "取消中",
    cancelled: "已取消",
    succeeded: "已成功",
    partially_succeeded: "部分成功",
    failed: "已失败",
    interrupted: "已中断",
    finalizing_failed: "结果待补写"
  };
  return labels[task.state];
}

function taskStageLabel(task: ActivityTask) {
  if (task.state === "queued") return "排队中";
  if (task.state === "failed") return "执行失败";
  if (task.state === "cancelled") return "已取消";
  if (task.state === "interrupted") return "已中断";
  if (task.state === "succeeded" || task.state === "partially_succeeded") return "已完成";
  if (task.state === "finalizing_failed") return "正在补写结果";
  const labels: Record<string, string> = {
    draft: "准备任务",
    checking: "检查中",
    check_failed: "检查失败",
    ready: "等待执行",
    queued: "排队中",
    prepare_config: "读取发布配置",
    prepare_artifact: "准备镜像文件",
    prepare_release: "生成发布包",
    prepare_target: "准备一体机配置",
    prepare_lease: "确认任务可执行",
    prepare_lease_failed: "任务执行条件确认失败",
    prepare_local: "准备发布文件",
    prepare_remote: "准备一体机",
    lease_acquired: "已确认可以执行",
    ssh_connect: "连接一体机",
    ssh_connected: "连接完成",
    upload: "上传发布文件",
    prepare_agent: "准备执行脚本",
    precheck: "检查运行环境",
    backup: "备份现有数据",
    install: "安装发布包",
    register: "确认平台注册",
    health: "检查服务状态",
    service_check: "检查目标服务",
    service_inspection: "读取服务和镜像",
    inspect_services: "读取服务和镜像",
    service_upgrade: "升级目标服务",
    service_health: "检查目标服务状态",
    remote_complete: "一体机操作完成",
    finalizing: "保存执行结果",
    completed: "已完成",
    cancelling: "正在取消",
    cancelled: "已取消",
    failed: "执行失败",
    preparation_failed: "准备失败",
    submission_failed: "任务提交失败",
    handler_failed: "任务处理失败",
    needs_reconcile: "等待结果确认",
    interrupted: "执行中断",
    preflight_target: "检查一体机"
  };
  return labels[task.stage] ?? (/[㐀-鿿]/u.test(task.stage) ? task.stage : "处理中");
}

function handleLogScroll() {
  const element = logLines.value;
  if (!element) return;
  followLatest.value = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
}

async function returnToLatest() {
  followLatest.value = true;
  await nextTick();
  if (logLines.value) logLines.value.scrollTop = logLines.value.scrollHeight;
}

async function clearCurrentPanel() {
  try {
    if (activity.panelTab === "tasks") {
      const cleared = await activity.clearFinishedTasks();
      message.success(cleared ? `已清空 ${cleared} 条已结束任务记录` : "没有可清空的任务记录");
      return;
    }
    if (await activity.clearSelectedTaskLogs()) message.success("已清空当前任务日志");
  } catch (cause) {
    message.error(cause instanceof Error ? cause.message : "清空失败");
  }
}

onMounted(async () => {
  await projects.initialize();
  if (projects.activeProjectId) await activity.start(projects.activeProjectId);
});
onBeforeUnmount(() => activity.dispose());
watch(() => projects.activeProjectId, (projectId) => void activity.start(projectId));
watch(
  () => activity.logs.at(-1)?.sequence,
  async () => {
    if (followLatest.value) await returnToLatest();
  }
);
</script>

<template>
  <section v-if="activity.panelOpen" class="activity-panel" data-testid="activity-panel">
    <header class="activity-panel__header">
      <div class="activity-panel__title"><ListRestart :size="16" /><button type="button" :class="{ active: activity.panelTab === 'tasks' }" @click="activity.panelTab = 'tasks'">任务</button><button type="button" :class="{ active: activity.panelTab === 'logs' }" @click="activity.panelTab = 'logs'">日志</button><span v-if="activity.activeTaskCount">{{ activity.activeTaskCount }} 个活动任务</span></div>
      <div class="activity-panel__actions">
        <n-popconfirm positive-text="确认清空" negative-text="取消" @positive-click="clearCurrentPanel">
          <template #trigger><button type="button" :title="clearActionTitle" :disabled="activity.loading || !clearActionEnabled"><Trash2 :size="15" /></button></template>
          {{ clearConfirmation }}
        </n-popconfirm>
        <button type="button" title="刷新" :disabled="activity.loading" @click="activity.refreshTasks()"><RotateCw :size="15" /></button>
        <button type="button" title="收起" @click="activity.panelOpen = false"><ChevronDown :size="16" /></button>
        <button type="button" title="关闭" @click="activity.panelOpen = false"><X :size="15" /></button>
      </div>
    </header>
    <div class="activity-panel__content">
      <template v-if="activity.panelTab === 'tasks'">
        <div v-if="orderedTasks.length" class="task-table">
          <div class="task-table__head"><span>状态</span><span>任务</span><span>当前阶段</span><span>进度</span><span>更新时间</span></div>
          <button
            v-for="task in orderedTasks"
            :key="task.id"
            type="button"
            class="task-row"
            :class="{ selected: activity.selectedTaskId === task.id }"
            @click="activity.selectTask(task.id); activity.panelTab = 'logs'"
          >
            <n-tag size="small" :bordered="false" :type="tagType(task)">{{ stateLabel(task) }}</n-tag>
            <span class="task-name"><strong>{{ task.name }}</strong><small class="task-target-count">{{ task.targetCount }} 个目标</small></span>
            <span>{{ taskStageLabel(task) }}</span>
            <span v-if="task.progress !== null" class="task-progress"><small class="task-progress-value">{{ task.progress }}%</small><n-progress type="line" :percentage="task.progress" :show-indicator="false" :height="4" /></span>
            <span v-else class="task-progress task-progress--unknown"><small>进度未知</small></span>
            <time class="task-time">{{ formatDisplayDateTime(task.updatedAt) }}</time>
          </button>
        </div>
        <n-empty v-else :description="activity.error || '暂无任务'" />
      </template>
      <div v-else ref="logLines" class="log-view" @scroll="handleLogScroll">
          <div class="log-scope"><strong>{{ activity.selectedTask?.name ?? '未选择任务' }}</strong><span>完整过程仅保存在当前电脑</span><n-button v-if="!followLatest" size="tiny" quaternary @click="returnToLatest">回到最新</n-button><n-select v-model:value="activity.logLevels" class="log-filter-level" size="tiny" multiple clearable :options="levelOptions" placeholder="级别" @update:value="activity.refreshLogs" /><n-input v-model:value="activity.logKeyword" class="log-filter-keyword" size="tiny" clearable placeholder="筛选日志" @update:value="activity.refreshLogs" /><n-button v-if="activity.selectedTask?.cancellable" size="tiny" type="warning" secondary @click="activity.cancelSelectedTask">取消任务</n-button></div>
          <div v-if="activity.logs.length" class="log-lines" role="log">
            <n-button v-if="activity.logHasMore" class="log-load-older" size="tiny" quaternary @click="activity.loadOlderLogs">加载更早日志</n-button>
            <div v-for="entry in activity.logs" :key="entry.id" class="log-line" :class="entry.level.toLowerCase()">
              <time class="log-time">{{ formatDisplayDateTime(entry.timestamp) }}</time><span class="log-level">{{ entry.level }}</span><span class="log-source">{{ entry.source }}</span><span>{{ entry.message }}</span>
            </div>
          </div>
          <n-empty v-else :description="activity.error || '当前任务暂无日志'" />
      </div>
    </div>
  </section>
</template>
