import { defineStore } from "pinia";
import { computed, ref } from "vue";

import {
  demoHistory,
  demoLogs,
  demoNodes,
  demoProjects,
  demoReleaseProfile,
  demoTasks
} from "@/shared/fixtures/demoData";
import type { DemoLogEntry, DemoProject, DemoTask, EdgeNode, OperationMode, ReleaseProfile } from "@/shared/model/demo";

const nowText = () => new Intl.DateTimeFormat("zh-CN", { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false }).format(new Date());

export const useDemoStore = defineStore("demo", () => {
  const projects = ref<DemoProject[]>(structuredClone(demoProjects));
  const activeProjectId = ref(projects.value[0]?.id ?? "");
  const nodes = ref<EdgeNode[]>(structuredClone(demoNodes));
  const releaseProfile = ref<ReleaseProfile>(structuredClone(demoReleaseProfile));
  const tasks = ref<DemoTask[]>(structuredClone(demoTasks));
  const logs = ref<DemoLogEntry[]>(structuredClone(demoLogs));
  const history = ref(structuredClone(demoHistory));
  const selectedTaskId = ref(tasks.value[0]?.id ?? "");
  const activityOpen = ref(false);
  const activityTab = ref<"tasks" | "logs">("tasks");

  const activeProject = computed(() => projects.value.find((project) => project.id === activeProjectId.value) ?? projects.value[0]);
  const activeTaskCount = computed(() => tasks.value.filter((task) => task.state === "queued" || task.state === "running").length);
  const selectedTask = computed(() => tasks.value.find((task) => task.id === selectedTaskId.value) ?? tasks.value[0]);
  const selectedTaskLogs = computed(() => logs.value.filter((entry) => entry.taskId === selectedTaskId.value));

  function setActiveProject(id: string) {
    activeProjectId.value = id;
  }

  function loginProject(id: string, username: string) {
    const project = projects.value.find((item) => item.id === id);
    if (!project) return;
    project.connectionState = "ready";
    project.username = username || "实施管理员";
    activeProjectId.value = id;
  }

  function addProject(project: Omit<DemoProject, "id" | "connectionState">) {
    const next: DemoProject = {
      ...project,
      id: `project-${crypto.randomUUID()}`,
      connectionState: "login_required"
    };
    projects.value.push(next);
    activeProjectId.value = next.id;
    return next;
  }

  function updateProject(id: string, patch: Partial<DemoProject>) {
    const project = projects.value.find((item) => item.id === id);
    if (project) Object.assign(project, patch);
  }

  function applyMockImport() {
    const exists = nodes.value.some((node) => node.mac === "00:0C:29:3B:B9:39");
    if (!exists) {
      nodes.value.unshift({
        mac: "00:0C:29:3B:B9:39",
        name: "AIO-C栋-1F",
        ip: "10.20.14.21",
        location: "C栋 1F 弱电间",
        managementState: "pending",
        deployLabel: "待实施",
        platformState: "unknown",
        platformUpdatedAt: "尚未注册",
        serviceState: "unknown",
        serviceLabel: "待检查",
        lastOperation: "导入清单 · 刚刚",
        versions: []
      });
    }
  }

  function saveReleaseProfile(next: ReleaseProfile) {
    releaseProfile.value = {
      ...structuredClone(next),
      version: releaseProfile.value.version + 1,
      updatedBy: activeProject.value?.username ?? "实施管理员",
      updatedAt: new Intl.DateTimeFormat("zh-CN", { dateStyle: "short", timeStyle: "short" }).format(new Date())
    };
  }

  function appendLog(taskId: string, level: DemoLogEntry["level"], source: string, message: string) {
    logs.value.push({ id: crypto.randomUUID(), taskId, time: nowText(), level, source, message });
  }

  function startMockTask(mode: OperationMode, targetMacs: string[], artifact: string) {
    const id = crypto.randomUUID();
    const modeLabel = mode === "first_deploy" ? "首次部署" : mode === "full_upgrade" ? "整包升级" : "单服升级";
    const task: DemoTask = {
      id,
      projectId: activeProjectId.value,
      name: `${modeLabel} · ${artifact}`,
      mode,
      state: "running",
      stage: "准备连接",
      progress: 6,
      targetCount: targetMacs.length,
      completedCount: 0,
      updatedAt: "刚刚"
    };
    tasks.value.unshift(task);
    selectedTaskId.value = id;
    activityOpen.value = true;
    activityTab.value = "logs";
    appendLog(id, "INFO", "任务", `开始${modeLabel}，目标 ${targetMacs.length} 台`);

    const stages = [
      ["检查 SSH 与运行环境", 20],
      ["上传发布文件", 42],
      ["执行远端部署", 68],
      ["检查服务与版本", 88],
      ["写入最终结果", 100]
    ] as const;
    let cursor = 0;
    const timer = window.setInterval(() => {
      const current = stages[cursor];
      if (!current) {
        window.clearInterval(timer);
        return;
      }
      task.stage = current[0];
      task.progress = current[1];
      task.updatedAt = "刚刚";
      appendLog(id, "INFO", cursor < targetMacs.length ? targetMacs[cursor] ?? "批次" : "批次", current[0]);
      cursor += 1;
      if (cursor === stages.length) {
        task.state = "succeeded";
        task.completedCount = targetMacs.length;
        task.stage = "已完成";
        targetMacs.forEach((mac) => {
          const node = nodes.value.find((item) => item.mac === mac);
          if (node) {
            node.deployLabel = mode === "first_deploy" ? "已部署" : "已升级";
            node.managementState = "managed";
            node.serviceState = "healthy";
            node.serviceLabel = "4项正常";
            node.lastOperation = `${modeLabel} · 刚刚`;
          }
        });
        history.value.unshift({
          id: `OP-${new Date().getTime()}`,
          type: modeLabel,
          operator: activeProject.value?.username ?? "实施管理员",
          targetSummary: `${targetMacs.length} 台`,
          artifact,
          result: "成功",
          finishedAt: "刚刚"
        });
        appendLog(id, "INFO", "项目侧", "操作摘要和节点最终结果已记录");
        window.clearInterval(timer);
      }
    }, 700);
    return id;
  }

  function openActivity(tab: "tasks" | "logs" = "tasks") {
    activityTab.value = tab;
    activityOpen.value = true;
  }

  return {
    projects,
    activeProjectId,
    activeProject,
    nodes,
    releaseProfile,
    tasks,
    logs,
    history,
    selectedTaskId,
    selectedTask,
    selectedTaskLogs,
    activeTaskCount,
    activityOpen,
    activityTab,
    setActiveProject,
    loginProject,
    addProject,
    updateProject,
    applyMockImport,
    saveReleaseProfile,
    startMockTask,
    openActivity
  };
});

