import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import type {
  PlatformLoginChallenge,
  PlatformLoginRequest,
  ProjectConnectionTestRequest,
  ProjectConnectionTestResult,
  ProjectInput,
  ProjectOverview,
  ProjectSession,
  WorkbenchSchemaStatus
} from "@/shared/model/project";

const ACTIVE_PROJECT_KEY = "inx.workbench.active-project";

export const useProjectStore = defineStore("projects", () => {
  const projects = ref<ProjectOverview[]>([]);
  const activeProjectId = ref("");
  const initialized = ref(false);
  const loading = ref(false);
  const switching = ref(false);
  const error = ref("");
  const lastConnectionTest = ref<ProjectConnectionTestResult>();
  const schemaStatus = ref<WorkbenchSchemaStatus>();
  const loginChallenge = ref<PlatformLoginChallenge>();

  const activeProject = computed(() =>
    projects.value.find((project) => project.id === activeProjectId.value)
  );
  const session = computed(() => activeProject.value?.session);
  const isReady = computed(() => activeProject.value?.connectionState === "ready");
  const databaseConnected = computed(() => activeProject.value?.databaseState === "connected");
  const businessMenuEnabled = computed(() => isReady.value);

  function replaceProject(project: ProjectOverview) {
    const index = projects.value.findIndex((item) => item.id === project.id);
    if (index >= 0) projects.value[index] = project;
    else projects.value.unshift(project);
  }

  async function initialize() {
    if (initialized.value || loading.value) return;
    loading.value = true;
    error.value = "";
    try {
      projects.value = await useWorkbenchAdapter().listProjects();
      const saved = localStorage.getItem(ACTIVE_PROJECT_KEY);
      const initial = projects.value.find((item) => item.id === saved) ?? projects.value[0];
      if (initial) {
        activeProjectId.value = initial.id;
        try {
          await switchProject(initial.id, false);
        } catch {
          // 保留项目入口和真实错误状态，用户可以编辑或重试。
        }
      }
    } catch (cause) {
      error.value = commandErrorText(cause, "读取本地项目失败");
    } finally {
      initialized.value = true;
      loading.value = false;
    }
  }

  async function refresh() {
    projects.value = await useWorkbenchAdapter().listProjects();
  }

  async function createProject(input: ProjectInput) {
    error.value = "";
    try {
      const project = await useWorkbenchAdapter().createProject(input);
      replaceProject(project);
      activeProjectId.value = project.id;
      localStorage.setItem(ACTIVE_PROJECT_KEY, project.id);
      return project;
    } catch (cause) {
      error.value = commandErrorText(cause, "创建项目失败");
      throw cause;
    }
  }

  async function updateProject(projectId: string, input: ProjectInput) {
    error.value = "";
    try {
      const project = await useWorkbenchAdapter().updateProject(projectId, input);
      replaceProject(project);
      return project;
    } catch (cause) {
      error.value = commandErrorText(cause, "编辑项目失败");
      throw cause;
    }
  }

  async function deleteProject(projectId: string) {
    error.value = "";
    try {
      await useWorkbenchAdapter().deleteProject(projectId);
      projects.value = projects.value.filter((item) => item.id !== projectId);
      if (activeProjectId.value === projectId) {
        activeProjectId.value = projects.value[0]?.id ?? "";
        if (activeProjectId.value) localStorage.setItem(ACTIVE_PROJECT_KEY, activeProjectId.value);
        else localStorage.removeItem(ACTIVE_PROJECT_KEY);
      }
    } catch (cause) {
      error.value = commandErrorText(cause, "删除项目失败");
      throw cause;
    }
  }

  async function testConnection(request: ProjectConnectionTestRequest) {
    error.value = "";
    try {
      lastConnectionTest.value = await useWorkbenchAdapter().testProjectConnection(request);
      return lastConnectionTest.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "项目连接测试失败");
      lastConnectionTest.value = undefined;
      throw cause;
    }
  }

  async function switchProject(projectId: string, persist = true) {
    switching.value = true;
    error.value = "";
    activeProjectId.value = projectId;
    if (persist) localStorage.setItem(ACTIVE_PROJECT_KEY, projectId);
    try {
      const project = await useWorkbenchAdapter().switchProject(projectId);
      replaceProject(project);
      schemaStatus.value = await useWorkbenchAdapter().getWorkbenchSchemaStatus(projectId);
      return project;
    } catch (cause) {
      const project = projects.value.find((item) => item.id === projectId);
      if (project) {
        project.connectionState = "connection_failed";
        project.databaseState = "failed";
        project.statusMessage = commandErrorText(cause, "项目连接失败");
      }
      error.value = commandErrorText(cause, "项目切换失败");
      throw cause;
    } finally {
      switching.value = false;
    }
  }

  async function loadSchemaStatus(projectId = activeProjectId.value) {
    if (!projectId) return undefined;
    schemaStatus.value = await useWorkbenchAdapter().getWorkbenchSchemaStatus(projectId);
    return schemaStatus.value;
  }

  async function upgradeSchema(projectId = activeProjectId.value) {
    if (!projectId) throw new Error("没有活动项目");
    schemaStatus.value = await useWorkbenchAdapter().initializeOrUpgradeWorkbenchSchema(projectId);
    await switchProject(projectId);
    return schemaStatus.value;
  }

  async function createLoginChallenge(projectId = activeProjectId.value) {
    if (!projectId) throw new Error("没有活动项目");
    loginChallenge.value = await useWorkbenchAdapter().createLoginChallenge(projectId);
    return loginChallenge.value;
  }

  async function login(projectId: string, request: PlatformLoginRequest) {
    const nextSession = await useWorkbenchAdapter().loginProject(projectId, request);
    await switchProject(projectId);
    return nextSession;
  }

  async function checkSession(projectId = activeProjectId.value): Promise<ProjectSession | undefined> {
    if (!projectId) return undefined;
    const next = await useWorkbenchAdapter().checkProjectSession(projectId);
    const project = projects.value.find((item) => item.id === projectId);
    if (project) {
      project.session = next.state === "missing" ? undefined : next;
      if (next.state === "expired") project.connectionState = "session_expired";
      if (next.state === "missing" && project.databaseState === "connected") project.connectionState = "login_required";
    }
    return next;
  }

  async function logout(projectId = activeProjectId.value) {
    if (!projectId) return;
    await useWorkbenchAdapter().logoutProject(projectId);
    const project = projects.value.find((item) => item.id === projectId);
    if (project) {
      project.session = undefined;
      project.connectionState = "login_required";
      project.statusMessage = "已退出平台登录";
    }
  }

  return {
    projects,
    activeProjectId,
    activeProject,
    session,
    isReady,
    databaseConnected,
    businessMenuEnabled,
    initialized,
    loading,
    switching,
    error,
    lastConnectionTest,
    schemaStatus,
    loginChallenge,
    initialize,
    refresh,
    createProject,
    updateProject,
    deleteProject,
    testConnection,
    switchProject,
    loadSchemaStatus,
    upgradeSchema,
    createLoginChallenge,
    login,
    checkSession,
    logout
  };
});
