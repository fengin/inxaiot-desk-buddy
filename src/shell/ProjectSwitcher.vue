<script setup lang="ts">
import {
  NAlert,
  NButton,
  NForm,
  NFormItem,
  NInput,
  NInputNumber,
  NModal,
  NPopconfirm,
  NPopover,
  NSpace,
  NTag,
  useMessage
} from "naive-ui";
import {
  Check,
  ChevronDown,
  Database,
  LogIn,
  LogOut,
  Pencil,
  Plus,
  RefreshCw,
  Server,
  Trash2
} from "lucide-vue-next";
import { onMounted, reactive, ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import type { ProjectInput, ProjectOverview } from "@/shared/model/project";
import { useProjectStore } from "@/stores/projects";

const projects = useProjectStore();
const message = useMessage();
const defaultWorkbenchDatabase = import.meta.env.VITE_STAGE75_WORKBENCH_DB || "inxaiot_desk_buddy";
const popoverOpen = ref(false);
const loginOpen = ref(false);
const projectDialogOpen = ref(false);
const savingProject = ref(false);
const connectionTesting = ref(false);
const loginLoading = ref(false);
const schemaLoading = ref(false);
const editingProjectId = ref<string>();
const loginProjectId = ref<string>();
const loginForm = reactive({ username: "", password: "", sessionUuid: "", imageCode: "" });
const projectForm = reactive<ProjectInput>({
  name: "",
  platformUrl: "",
  dbHost: "",
  dbPort: 3306,
  dbUser: "",
  dbPassword: "",
  businessDb: "",
  workbenchDb: defaultWorkbenchDatabase
});

onMounted(() => void projects.initialize());

async function retryInitialize() {
  await projects.initialize();
  if (projects.error) message.error(projects.error);
}

function connectionTag(project: ProjectOverview) {
  if (project.connectionState === "ready") return { type: "success" as const, label: "已就绪" };
  if (project.connectionState === "login_required") return { type: "warning" as const, label: "需登录" };
  if (project.connectionState === "session_expired") return { type: "warning" as const, label: "会话过期" };
  if (project.connectionState === "schema_required") return { type: "warning" as const, label: "需升级结构" };
  if (project.connectionState === "connecting") return { type: "info" as const, label: "连接中" };
  if (project.connectionState === "disconnected") return { type: "default" as const, label: "未连接" };
  return { type: "error" as const, label: "连接失败" };
}

function connectionDotClass(project?: ProjectOverview) {
  if (project?.connectionState === "ready") return "ready";
  if (["login_required", "session_expired", "schema_required"].includes(project?.connectionState ?? "")) return "login_required";
  return "offline";
}

function projectInitials(project: ProjectOverview) {
  return project.name.trim().slice(0, 2).toUpperCase() || "IN";
}

function schemaTag() {
  const schema = projects.schemaStatus;
  if (!schema) return { type: "default" as const, label: "未检查" };
  if (schema.state === "ready") return { type: "success" as const, label: "结构已就绪" };
  if (["uninitialized", "upgrade_required"].includes(schema.state)) {
    return { type: "warning" as const, label: "需要初始化/升级" };
  }
  return { type: "error" as const, label: "结构不兼容" };
}

async function chooseProject(project: ProjectOverview) {
  popoverOpen.value = false;
  try {
    const current = await projects.switchProject(project.id);
    if (["login_required", "session_expired"].includes(current.connectionState)) {
      await openLogin(project.id);
      return;
    }
    if (current.connectionState === "schema_required") {
      message.warning("工作台数据库结构尚未就绪，请编辑项目并显式初始化/升级");
      openEditProject();
      return;
    }
    message.success(`已切换到 ${project.name}`);
  } catch (cause) {
    message.error(commandErrorText(cause, "项目连接失败"));
  }
}

async function openLogin(projectId = projects.activeProjectId) {
  if (!projectId) return;
  loginProjectId.value = projectId;
  Object.assign(loginForm, { username: "", password: "", sessionUuid: "", imageCode: "" });
  loginOpen.value = true;
  try {
    const challenge = await projects.createLoginChallenge(projectId);
    loginForm.sessionUuid = challenge.sessionUuid;
  } catch (cause) {
    message.error(commandErrorText(cause, "获取登录验证码失败"));
  }
}

async function refreshChallenge() {
  if (!loginProjectId.value) return;
  try {
    const challenge = await projects.createLoginChallenge(loginProjectId.value);
    loginForm.sessionUuid = challenge.sessionUuid;
    loginForm.imageCode = "";
  } catch (cause) {
    message.error(commandErrorText(cause, "刷新登录验证码失败"));
  }
}

async function submitLogin() {
  if (!loginProjectId.value) return;
  loginLoading.value = true;
  try {
    await projects.login(loginProjectId.value, { ...loginForm });
    loginOpen.value = false;
    message.success("项目平台登录成功");
  } catch (cause) {
    message.error(commandErrorText(cause, "项目平台登录失败"));
    await refreshChallenge();
  } finally {
    loginLoading.value = false;
  }
}

async function logout() {
  await projects.logout();
  message.success("已退出当前项目平台登录");
}

function resetProjectForm() {
  Object.assign(projectForm, {
    name: "",
    platformUrl: "",
    dbHost: "",
    dbPort: 3306,
    dbUser: "",
    dbPassword: "",
    businessDb: "",
    workbenchDb: defaultWorkbenchDatabase
  });
  projects.lastConnectionTest = undefined;
  projects.schemaStatus = undefined;
}

function openCreateProject() {
  editingProjectId.value = undefined;
  resetProjectForm();
  projectDialogOpen.value = true;
  popoverOpen.value = false;
}

function openEditProject() {
  const project = projects.activeProject;
  if (!project) return;
  editingProjectId.value = project.id;
  Object.assign(projectForm, {
    name: project.name,
    platformUrl: project.platformUrl,
    dbHost: project.dbHost,
    dbPort: project.dbPort,
    dbUser: project.dbUser,
    dbPassword: "",
    businessDb: project.businessDb,
    workbenchDb: project.workbenchDb
  });
  projects.lastConnectionTest = undefined;
  projectDialogOpen.value = true;
  void loadSchemaStatus(project.id);
  popoverOpen.value = false;
}

async function testConnection() {
  connectionTesting.value = true;
  try {
    const result = await projects.testConnection({
      existingProjectId: editingProjectId.value,
      project: { ...projectForm }
    });
    if (result.successful) message.success("双数据库连接与平台 Schema 探测通过");
    else message.error(result.message);
    return result;
  } catch (cause) {
    message.error(commandErrorText(cause, "连接测试失败"));
    return undefined;
  } finally {
    connectionTesting.value = false;
  }
}

async function saveProject() {
  savingProject.value = true;
  try {
    const test = await testConnection();
    if (!test?.successful) return;
    const input = { ...projectForm };
    if (!input.dbPassword) input.dbPassword = undefined;
    const project = editingProjectId.value
      ? await projects.updateProject(editingProjectId.value, input)
      : await projects.createProject(input);
    projectDialogOpen.value = false;
    const opened = await projects.switchProject(project.id);
    message.success(editingProjectId.value ? "项目入口已更新" : "项目入口已创建");
    if (opened.connectionState === "schema_required") {
      editingProjectId.value = project.id;
      projectDialogOpen.value = true;
      await loadSchemaStatus(project.id);
      message.warning("数据库连接通过；请显式初始化/升级工作台 Schema 后继续登录");
    } else if (["login_required", "session_expired"].includes(opened.connectionState)) {
      await openLogin(project.id);
    }
  } catch (cause) {
    message.error(commandErrorText(cause, "保存项目失败"));
  } finally {
    savingProject.value = false;
  }
}

async function deleteCurrentProject() {
  if (!projects.activeProjectId) return;
  try {
    await projects.deleteProject(projects.activeProjectId);
    projectDialogOpen.value = false;
    message.success("本地项目入口已删除，远端数据库数据未删除");
  } catch (cause) {
    message.error(commandErrorText(cause, "删除项目失败"));
  }
}

async function loadSchemaStatus(projectId = editingProjectId.value) {
  if (!projectId) return;
  schemaLoading.value = true;
  try {
    await projects.loadSchemaStatus(projectId);
  } catch (cause) {
    message.error(commandErrorText(cause, "数据库结构检查失败"));
  } finally {
    schemaLoading.value = false;
  }
}

async function upgradeSchema() {
  if (!editingProjectId.value) return;
  schemaLoading.value = true;
  try {
    await projects.upgradeSchema(editingProjectId.value);
    message.success("工作台数据库初始化/升级完成");
    if (["login_required", "session_expired"].includes(projects.activeProject?.connectionState ?? "")) {
      projectDialogOpen.value = false;
      await openLogin(editingProjectId.value);
    }
  } catch (cause) {
    message.error(commandErrorText(cause, "数据库结构升级失败"));
  } finally {
    schemaLoading.value = false;
  }
}
</script>

<template>
  <n-popover v-model:show="popoverOpen" trigger="click" placement="bottom-start" :width="380">
    <template #trigger>
      <button class="project-trigger" type="button" data-testid="project-switcher" data-action-owner="popover">
        <span class="project-trigger__icon"><Database :size="17" /></span>
        <span class="project-trigger__copy">
          <strong>{{ projects.activeProject?.name ?? "选择或新增项目" }}</strong>
        </span>
        <span class="connection-dot" :class="connectionDotClass(projects.activeProject)"></span>
        <ChevronDown :size="15" />
      </button>
    </template>

    <div class="project-popover">
      <div class="popover-heading">
        <div><strong>切换项目</strong><small>每个项目使用独立连接和登录会话</small></div>
        <n-button size="tiny" quaternary @click="openCreateProject"><template #icon><Plus /></template>新增</n-button>
      </div>
      <div v-if="projects.loading" class="project-empty">正在读取本地项目…</div>
      <div v-else-if="projects.error && !projects.initialized" class="project-empty">
        <span>{{ projects.error }}</span>
        <n-button size="tiny" text type="primary" data-testid="project-initialize-retry" @click="retryInitialize">重新读取</n-button>
      </div>
      <div v-else-if="!projects.projects.length" class="project-empty">还没有项目，请先新增项目入口</div>
      <button
        v-for="project in projects.projects"
        :key="project.id"
        type="button"
        class="project-option"
        :class="{ active: project.id === projects.activeProjectId }"
        @click="chooseProject(project)"
      >
        <span class="project-avatar">{{ projectInitials(project) }}</span>
        <span class="project-option__copy">
          <strong>{{ project.name }}</strong>
          <small>{{ project.dbHost }}:{{ project.dbPort }} · {{ project.businessDb }}</small>
        </span>
        <n-tag size="small" :bordered="false" :type="connectionTag(project).type">
          {{ connectionTag(project).label }}
        </n-tag>
        <Check v-if="project.id === projects.activeProjectId" :size="16" class="project-check" />
      </button>
      <div class="project-popover__footer">
        <n-button size="small" secondary :disabled="!projects.activeProject" @click="openEditProject"><template #icon><Pencil /></template>编辑当前项目</n-button>
        <n-button v-if="projects.session?.state === 'active'" size="small" quaternary @click="logout"><template #icon><LogOut /></template>退出登录</n-button>
      </div>
    </div>
  </n-popover>

  <n-modal v-model:show="loginOpen" preset="card" title="登录项目平台" class="compact-modal login-modal" :bordered="false">
    <div class="login-project-summary">
      <span class="feature-icon info"><LogIn :size="20" /></span>
      <div><strong>{{ projects.projects.find((item) => item.id === loginProjectId)?.name }}</strong><small>使用项目平台后台管理的同一套账号密码</small></div>
    </div>
    <n-form label-placement="left" label-width="82" size="small">
      <n-form-item label="账号"><n-input v-model:value="loginForm.username" data-testid="login-username" autocomplete="username" /></n-form-item>
      <n-form-item label="密码"><n-input v-model:value="loginForm.password" data-testid="login-password" type="password" show-password-on="click" autocomplete="current-password" /></n-form-item>
      <n-form-item label="验证码">
        <div class="captcha-row">
          <n-input v-model:value="loginForm.imageCode" data-testid="login-image-code" />
          <button class="captcha-image" type="button" title="刷新验证码" @click="refreshChallenge">
            <img v-if="projects.loginChallenge?.captchaImageDataUrl" :src="projects.loginChallenge.captchaImageDataUrl" alt="平台验证码" />
            <RefreshCw v-else :size="17" />
          </button>
        </div>
      </n-form-item>
      <n-form-item label="会话标识"><n-input v-model:value="loginForm.sessionUuid" data-testid="login-session-uuid" placeholder="验证码会话 UUID" /></n-form-item>
    </n-form>
    <n-alert v-if="!projects.loginChallenge?.captchaImageDataUrl" type="warning" :show-icon="false">
      平台未返回验证码图片；开发环境可按现场平台配置填写验证码与会话标识。
    </n-alert>
    <template #footer>
      <n-space justify="end"><n-button size="small" @click="loginOpen = false">取消</n-button><n-button size="small" type="primary" data-testid="login-submit" :loading="loginLoading" @click="submitLogin">登录并进入</n-button></n-space>
    </template>
  </n-modal>

  <n-modal v-model:show="projectDialogOpen" preset="card" :title="editingProjectId ? '编辑项目入口' : '新增项目入口'" class="project-editor-modal" :bordered="false">
    <p class="modal-description">本地只保存项目连接入口；共享配置和最终资产位于项目侧工作台数据库。</p>
    <n-form label-placement="left" label-width="108" size="small">
      <n-form-item label="项目名称"><n-input v-model:value="projectForm.name" data-testid="project-name" placeholder="例如：深圳湾智慧园区" /></n-form-item>
      <n-form-item label="平台访问地址"><n-input v-model:value="projectForm.platformUrl" data-testid="project-platform-url" placeholder="http://192.168.3.6:8055" /></n-form-item>
      <n-form-item label="数据库主机"><n-input v-model:value="projectForm.dbHost" data-testid="project-db-host" placeholder="192.168.3.6" /></n-form-item>
      <n-form-item label="数据库端口"><n-input-number v-model:value="projectForm.dbPort" data-testid="project-db-port" :show-button="false" :min="1" :max="65535" /></n-form-item>
      <n-form-item label="数据库账号"><n-input v-model:value="projectForm.dbUser" data-testid="project-db-user" /></n-form-item>
      <n-form-item label="数据库密码"><n-input v-model:value="projectForm.dbPassword" data-testid="project-db-password" type="password" show-password-on="click" :placeholder="editingProjectId ? '留空表示保持原密码' : '请输入数据库密码'" /></n-form-item>
      <n-form-item label="平台业务库"><n-input v-model:value="projectForm.businessDb" data-testid="project-business-db" placeholder="inxvision_iot_dev" /></n-form-item>
      <n-form-item label="工作台库"><n-input v-model:value="projectForm.workbenchDb" data-testid="project-workbench-db" readonly /></n-form-item>
    </n-form>
    <div class="connection-preview"><Server :size="16" /><span>连接测试只读探测平台库；不会初始化结构或修改平台业务数据</span></div>
    <n-alert v-if="projects.lastConnectionTest" :type="projects.lastConnectionTest.successful ? 'success' : 'error'" :show-icon="false">
      {{ projects.lastConnectionTest.message }} · MySQL {{ projects.lastConnectionTest.mysqlVersion }} · {{ projects.lastConnectionTest.connectionEncrypted ? '连接已加密' : '连接未加密' }}
    </n-alert>
    <div v-if="editingProjectId" class="schema-maintenance">
      <div class="schema-maintenance__copy">
        <strong>工作台数据库结构</strong>
        <span>{{ projects.schemaStatus?.message ?? "尚未检查" }}</span>
        <small v-if="projects.schemaStatus">当前版本 {{ projects.schemaStatus.currentVersion ?? 0 }} · 可用版本 {{ projects.schemaStatus.latestAvailableVersion }}</small>
      </div>
      <n-tag size="small" :bordered="false" :type="schemaTag().type">{{ schemaTag().label }}</n-tag>
      <n-button size="small" secondary :loading="schemaLoading" @click="loadSchemaStatus()">检查结构</n-button>
      <n-popconfirm
        v-if="projects.schemaStatus && ['uninitialized', 'upgrade_required'].includes(projects.schemaStatus.state)"
        positive-text="确认执行"
        negative-text="取消"
        @positive-click="upgradeSchema"
      >
        <template #trigger><n-button size="small" type="primary" data-testid="schema-upgrade" data-action-owner="popconfirm" :loading="schemaLoading">初始化/升级</n-button></template>
        将只修改项目侧工作台数据库，不会修改平台业务库。确认继续？
      </n-popconfirm>
    </div>
    <template #footer>
      <div class="project-editor-actions">
        <n-popconfirm v-if="editingProjectId" positive-text="仅删除本地入口" negative-text="取消" @positive-click="deleteCurrentProject">
          <template #trigger><n-button size="small" type="error" secondary data-action-owner="popconfirm"><template #icon><Trash2 /></template>删除项目</n-button></template>
          只删除当前电脑上的项目入口和会话，不删除任何远端数据库数据。继续？
        </n-popconfirm>
        <n-space justify="end">
          <n-button size="small" @click="projectDialogOpen = false">取消</n-button>
          <n-button size="small" secondary data-testid="project-test-connection" :loading="connectionTesting" @click="testConnection">测试连接</n-button>
          <n-button size="small" type="primary" data-testid="project-save" :loading="savingProject" @click="saveProject">保存项目</n-button>
        </n-space>
      </div>
    </template>
  </n-modal>
</template>
