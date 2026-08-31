<script setup lang="ts">
import { NButton, NEmpty, NInput, NProgress, NSelect, NTag } from "naive-ui";
import { ChevronDown, ListRestart, RotateCw, X } from "lucide-vue-next";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";

import type { ActivityTask, ActivityTaskState } from "@/shared/model/activity";
import { useActivityStore } from "@/stores/activity";
import { useProjectStore } from "@/stores/projects";

const activity = useActivityStore();
const projects = useProjectStore();
const logLines = ref<HTMLElement>();
const followLatest = ref(true);
const orderedTasks = computed(() => [...activity.tasks].sort((a, b) => {
  const rank: Partial<Record<ActivityTaskState, number>> = {
    running: 0,
    cancelling: 1,
    queued: 2,
    finalizing_failed: 3
  };
  return (rank[a.state] ?? 4) - (rank[b.state] ?? 4);
}));
const levelOptions = [
  { label: "INFO", value: "INFO" },
  { label: "WARN", value: "WARN" },
  { label: "ERROR", value: "ERROR" }
];

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

function formatTime(value: string) {
  if (!/^\d{16,}$/.test(value)) return value;
  try {
    const milliseconds = Number(BigInt(value) / 1_000_000n);
    return new Intl.DateTimeFormat("zh-CN", {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hour12: false
    }).format(new Date(milliseconds));
  } catch {
    return value;
  }
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
            <span class="task-name"><strong>{{ task.name }}</strong><small>{{ task.targetCount }} 个目标</small></span>
            <span>{{ task.stage }}</span>
            <span v-if="task.progress !== null" class="task-progress"><n-progress type="line" :percentage="task.progress" :show-indicator="false" :height="4" /><small>{{ task.progress }}%</small></span>
            <span v-else class="task-progress"><small>进度未知</small></span>
            <span>{{ formatTime(task.updatedAt) }}</span>
          </button>
        </div>
        <n-empty v-else :description="activity.error || '暂无任务'" />
      </template>
      <div v-else ref="logLines" class="log-view" @scroll="handleLogScroll">
          <div class="log-scope"><strong>{{ activity.selectedTask?.name ?? '未选择任务' }}</strong><span>完整过程仅保存在当前实例</span><n-button v-if="!followLatest" size="tiny" quaternary @click="returnToLatest">回到最新</n-button><n-select v-model:value="activity.logLevels" class="log-filter-level" size="tiny" multiple clearable :options="levelOptions" placeholder="级别" @update:value="activity.refreshLogs" /><n-input v-model:value="activity.logKeyword" class="log-filter-keyword" size="tiny" clearable placeholder="筛选日志" @update:value="activity.refreshLogs" /><n-button v-if="activity.selectedTask?.cancellable" size="tiny" type="warning" secondary @click="activity.cancelSelectedTask">取消任务</n-button></div>
          <div v-if="activity.logs.length" class="log-lines" role="log">
            <n-button v-if="activity.logHasMore" class="log-load-older" size="tiny" quaternary @click="activity.loadOlderLogs">加载更早日志</n-button>
            <div v-for="entry in activity.logs" :key="entry.id" class="log-line" :class="entry.level.toLowerCase()">
              <time>{{ formatTime(entry.timestamp) }}</time><span class="log-level">{{ entry.level }}</span><span class="log-source">{{ entry.source }}</span><span>{{ entry.message }}</span>
            </div>
          </div>
          <n-empty v-else :description="activity.error || '当前任务暂无日志'" />
      </div>
    </div>
  </section>
</template>
