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
  let switchRequest = 0;
  let schemaRequest = 0;
  let sessionRequest = 0;

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
      initialized.value = false;
    } finally {
      loading.value = false;
    }
    if (!error.value) initialized.value = true;
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
        const nextProjectId = projects.value[0]?.id ?? "";
        activeProjectId.value = nextProjectId;
        if (nextProjectId) {
          localStorage.setItem(ACTIVE_PROJECT_KEY, nextProjectId);
          await switchProject(nextProjectId, false);
        } else {
          localStorage.removeItem(ACTIVE_PROJECT_KEY);
          schemaStatus.value = undefined;
        }
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
    const request = ++switchRequest;
    switching.value = true;
    error.value = "";
    activeProjectId.value = projectId;
    if (persist) localStorage.setItem(ACTIVE_PROJECT_KEY, projectId);
    try {
      const project = await useWorkbenchAdapter().switchProject(projectId);
      const schema = await useWorkbenchAdapter().getWorkbenchSchemaStatus(projectId);
      if (request === switchRequest && activeProjectId.value === projectId) {
        replaceProject(project);
        schemaStatus.value = schema;
      }
      return project;
    } catch (cause) {
      if (request === switchRequest && activeProjectId.value === projectId) {
        const project = projects.value.find((item) => item.id === projectId);
        if (project) {
          project.connectionState = "connection_failed";
          project.databaseState = "failed";
          project.statusMessage = commandErrorText(cause, "项目连接失败");
        }
        error.value = commandErrorText(cause, "项目切换失败");
      }
      throw cause;
    } finally {
      if (request === switchRequest) switching.value = false;
    }
  }

  async function loadSchemaStatus(projectId = activeProjectId.value) {
    if (!projectId) return undefined;
    const request = ++schemaRequest;
    const result = await useWorkbenchAdapter().getWorkbenchSchemaStatus(projectId);
    if (request === schemaRequest && activeProjectId.value === projectId) {
      schemaStatus.value = result;
    }
    return result;
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
    const request = ++sessionRequest;
    try {
      const next = await useWorkbenchAdapter().checkProjectSession(projectId);
      if (request === sessionRequest) {
        const project = projects.value.find((item) => item.id === projectId);
        if (project) {
          project.session = next.state === "missing" ? undefined : next;
          if (next.state === "expired") {
            project.connectionState = "session_expired";
            project.statusMessage = "平台会话已失效，请重新登录";
          } else if (next.state === "missing" && project.databaseState === "connected") {
            project.connectionState = "login_required";
            project.statusMessage = "数据库已连接，需要登录平台";
          } else if (next.state === "active" && project.databaseState === "connected") {
            project.connectionState = "ready";
            project.statusMessage = "数据库、Schema与平台会话已就绪";
          }
        }
      }
      return next;
    } catch (cause) {
      if (request === sessionRequest) {
        const project = projects.value.find((item) => item.id === projectId);
        if (project) {
          project.connectionState = "connection_failed";
          project.statusMessage = commandErrorText(cause, "平台会话校验失败，业务入口已关闭");
        }
        error.value = commandErrorText(cause, "平台会话校验失败");
      }
      throw cause;
    }
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
