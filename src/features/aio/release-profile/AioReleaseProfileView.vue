<script setup lang="ts">
import {
  NAlert,
  NButton,
  NEmpty,
  NInput,
  NInputNumber,
  NTabPane,
  NTabs,
  NTag,
  useDialog,
  useMessage
} from "naive-ui";
import {
  CheckCircle2,
  Code2,
  FileKey2,
  Pencil,
  RefreshCw,
  Save,
  Undo2
} from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { formatDisplayDateTime } from "@/shared/format/dateTime";
import type { ReleaseProfileField } from "@/shared/model/releaseProfile";
import { useProjectStore } from "@/stores/projects";
import { useReleaseProfileStore } from "@/stores/releaseProfile";

const projects = useProjectStore();
const release = useReleaseProfileStore();
const message = useMessage();
const dialog = useDialog();
const dialogs = useSystemDialogAdapter();
const editing = ref(false);
const activeTemplate = ref("env");
const activeTemplateField = computed(() => activeTemplate.value === "env"
  ? "values.envTemplate" as const
  : activeTemplate.value === "compose"
    ? "values.composeTemplate" as const
    : "values.hostInfoTemplate" as const
);
const activeTemplateError = computed(() => release.fieldErrors[activeTemplateField.value]);
const agentScriptDisplay = computed(() => {
  const agent = release.profile?.agentScript;
  if (!agent) return "";
  const source = agent.source === "built_in" ? "内置" : "项目配置";
  return `${agent.fileName} · v${agent.version} · ${source}`;
});
const agentScriptTitle = computed(() => release.profile?.agentScript
  ? `协议 ${release.profile.agentScript.protocolVersion} · SHA-256 ${release.profile.agentScript.sha256}`
  : ""
);

function fieldProps(field: ReleaseProfileField) {
  const error = release.fieldErrors[field];
  return {
    status: error ? "error" as const : undefined,
    inputProps: {
      "aria-invalid": Boolean(error),
      "aria-describedby": error ? `release-error-${field}` : undefined
    }
  };
}

watch([
  () => release.fieldErrors["values.envTemplate"],
  () => release.fieldErrors["values.composeTemplate"],
  () => release.fieldErrors["values.hostInfoTemplate"]
], ([envError, composeError, hostInfoError]) => {
  if (envError) activeTemplate.value = "env";
  else if (composeError) activeTemplate.value = "compose";
  else if (hostInfoError) activeTemplate.value = "host-info";
});

async function load() {
  if (!projects.activeProjectId || !projects.isReady) return;
  try {
    await release.load(projects.activeProjectId);
  } catch (cause) {
    message.error(commandErrorText(cause, "读取发布参数失败"));
  }
}

onMounted(async () => {
  await projects.initialize();
  await load();
});

watch(
  () => projects.activeProjectId,
  async () => {
    editing.value = false;
    await load();
  }
);

function startEdit() {
  release.resetDraft();
  editing.value = true;
}

function cancelEdit() {
  release.resetDraft();
  editing.value = false;
}

async function validate() {
  const expectedProjectId = release.projectId;
  try {
    const result = await release.validate();
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    if (release.validation?.valid) {
      message.success(`配置校验通过，识别 ${result.recognizedPlaceholderCount} 个模板变量`);
    }
  } catch (cause) {
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    message.error(commandErrorText(cause, "配置校验失败"));
  }
}

async function save() {
  const expectedProjectId = release.projectId;
  try {
    const profile = await release.save();
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    editing.value = false;
    message.success(`发布参数已保存为版本 ${profile.version}`);
  } catch (cause) {
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    if (release.conflict) {
      message.error("配置已在其他电脑上更新，请刷新后重新编辑");
      return;
    }
    message.error(commandErrorText(cause, "保存发布参数失败"));
  }
}

async function reloadAfterConflict() {
  await load();
  editing.value = false;
  message.success("已加载最新发布参数");
}

async function chooseAgentScript() {
  dialog.warning({
    title: "确认更换一体机脚本",
    content: "非必要情况下请勿更换一体机脚本。请确认该脚本已经过充分验证，能够正常完成当前工作台支持的部署、升级和回滚操作。",
    positiveText: "确认更换",
    negativeText: "取消",
    maskClosable: false,
    onPositiveClick: () => { void selectAndReplaceAgentScript(); }
  });
}

async function selectAndReplaceAgentScript() {
  const expectedProjectId = release.projectId;
  let selected: string | null;
  try {
    selected = await dialogs.selectFile("选择经过验证的一体机脚本", [
      { name: "Shell 脚本", extensions: ["sh"] }
    ]);
  } catch (cause) {
    message.error(commandErrorText(cause, "选择一体机脚本失败"));
    return;
  }
  if (!selected || release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
  try {
    const profile = await release.replaceAgentScript(selected);
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    message.success(`一体机脚本已更换为 v${profile.agentScript.version}`);
  } catch (cause) {
    if (release.projectId !== expectedProjectId || projects.activeProjectId !== expectedProjectId) return;
    message.error(commandErrorText(cause, "更换一体机脚本失败"));
  }
}

async function viewAgentScript() {
  try {
    await release.openAgentScript();
  } catch (cause) {
    message.error(commandErrorText(cause, "打开一体机脚本失败"));
  }
}

</script>

<template>
  <section class="workspace-page release-profile-page" data-testid="release-profile">
    <header class="page-header">
      <div class="release-title-line">
        <h1>发布参数</h1>
        <div class="release-inline-meta">
          <span v-if="release.validation?.valid"><CheckCircle2 :size="14" />配置校验通过</span>
          <span v-if="release.profile">最后修改：{{ release.profile.updatedBy }} · {{ formatDisplayDateTime(release.profile.updatedAt) }}</span>
        </div>
      </div>
      <div class="page-actions">
        <n-tag v-if="release.profile" size="small" :bordered="false" type="info">版本 {{ release.profile.version }}</n-tag>
        <n-button v-if="!editing" size="small" type="primary" data-testid="release-edit" :disabled="!projects.isReady" @click="startEdit"><template #icon><Pencil /></template>{{ release.profile ? "编辑配置" : "新建配置" }}</n-button>
        <template v-else>
          <n-button size="small" @click="cancelEdit"><template #icon><Undo2 /></template>取消</n-button>
          <n-button size="small" secondary data-testid="release-validate" :disabled="release.saving" @click="validate">校验配置</n-button>
          <n-button size="small" type="primary" data-testid="release-save" :loading="release.saving" @click="save"><template #icon><Save /></template>保存配置</n-button>
        </template>
      </div>
    </header>

    <n-alert v-if="!projects.activeProject" type="info" :show-icon="false">请先在顶部新增并连接项目。</n-alert>
    <n-alert v-else-if="!projects.isReady" type="warning" :show-icon="false">
      当前项目尚未就绪：{{ projects.activeProject.statusMessage }}。请先完成项目连接、工作台数据初始化和平台登录。
    </n-alert>
    <n-alert v-if="release.conflict" type="error" :show-icon="false">
      发布配置已在其他电脑上更新，本次保存未覆盖远端数据。
      <n-button size="tiny" text type="primary" @click="reloadAfterConflict"><template #icon><RefreshCw /></template>加载最新版本</n-button>
    </n-alert>
    <n-alert v-if="release.error && !release.conflict" type="error" :show-icon="false">{{ release.error }}</n-alert>
    <n-alert v-if="release.profile?.credentialsResetRequired" type="warning" :show-icon="false" data-testid="release-credentials-reset-required">
      发布凭据加密格式已更新，请重新填写发布参数
    </n-alert>

    <n-empty v-if="projects.isReady && !release.profile && !editing && !release.loading" description="当前项目尚未创建发布参数" class="page-empty" />

    <div v-if="projects.isReady && (release.profile || editing)" class="release-layout">
      <section class="data-panel parameter-panel">
        <header class="panel-heading"><span class="feature-icon info"><FileKey2 :size="19" /></span><div><strong>项目发布参数</strong><small>保存后其他电脑上的工作台读取同一版本</small></div></header>
        <div class="parameter-scroll">
          <div class="form-section"><h3>平台接入</h3><div class="form-grid">
            <label>平台主机<n-input v-model:value="release.draft.values.platformHost" v-bind="fieldProps('values.platformHost')" data-testid="release-platform-host" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['values.platformHost']" id="release-error-values.platformHost" class="field-feedback" role="alert">{{ release.fieldErrors['values.platformHost'] }}</small></label>
            <label>平台 API 端口<n-input-number v-model:value="release.draft.values.platformApiPort" v-bind="fieldProps('values.platformApiPort')" data-testid="release-api-port" size="small" :disabled="!editing || release.saving" :show-button="false" /><small v-if="release.fieldErrors['values.platformApiPort']" id="release-error-values.platformApiPort" class="field-feedback" role="alert">{{ release.fieldErrors['values.platformApiPort'] }}</small></label>
            <label class="span-2">平台 API AuthKey<n-input v-model:value="release.draft.credentials.platformAuthKey" v-bind="fieldProps('credentials.platformAuthKey')" data-testid="release-auth-key" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.platformAuthKey']" id="release-error-credentials.platformAuthKey" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.platformAuthKey'] }}</small></label>
          </div></div>
          <div class="form-section"><h3>MQTT 配置</h3><div class="form-grid">
            <label>平台 MQTT 主机<n-input v-model:value="release.draft.values.platformMqttHost" v-bind="fieldProps('values.platformMqttHost')" data-testid="release-mqtt-host" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['values.platformMqttHost']" id="release-error-values.platformMqttHost" class="field-feedback" role="alert">{{ release.fieldErrors['values.platformMqttHost'] }}</small></label>
            <label>平台 MQTT 端口<n-input-number v-model:value="release.draft.values.platformMqttPort" v-bind="fieldProps('values.platformMqttPort')" data-testid="release-mqtt-port" size="small" :disabled="!editing || release.saving" :show-button="false" /><small v-if="release.fieldErrors['values.platformMqttPort']" id="release-error-values.platformMqttPort" class="field-feedback" role="alert">{{ release.fieldErrors['values.platformMqttPort'] }}</small></label>
            <label>平台 MQTT 账号<n-input v-model:value="release.draft.credentials.platformMqttUser" v-bind="fieldProps('credentials.platformMqttUser')" data-testid="release-mqtt-user" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.platformMqttUser']" id="release-error-credentials.platformMqttUser" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.platformMqttUser'] }}</small></label>
            <label>平台 MQTT 密码<n-input v-model:value="release.draft.credentials.platformMqttPassword" v-bind="fieldProps('credentials.platformMqttPassword')" data-testid="release-mqtt-password" type="password" show-password-on="click" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.platformMqttPassword']" id="release-error-credentials.platformMqttPassword" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.platformMqttPassword'] }}</small></label>
            <label><span>一体机 MQTT <span class="release-label-term">账号</span></span><n-input v-model:value="release.draft.credentials.aioMqttUser" v-bind="fieldProps('credentials.aioMqttUser')" data-testid="release-aio-mqtt-user" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.aioMqttUser']" id="release-error-credentials.aioMqttUser" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.aioMqttUser'] }}</small></label>
            <label><span>一体机 MQTT <span class="release-label-term">密码</span></span><n-input v-model:value="release.draft.credentials.aioMqttPassword" v-bind="fieldProps('credentials.aioMqttPassword')" data-testid="release-aio-mqtt-password" type="password" show-password-on="click" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.aioMqttPassword']" id="release-error-credentials.aioMqttPassword" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.aioMqttPassword'] }}</small></label>
          </div></div>
          <div class="form-section"><h3>SSH 连接</h3><div class="form-grid">
            <label>SSH 用户名<n-input v-model:value="release.draft.credentials.sshUser" v-bind="fieldProps('credentials.sshUser')" data-testid="release-ssh-user" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.sshUser']" id="release-error-credentials.sshUser" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.sshUser'] }}</small></label>
            <label>SSH 端口<n-input-number v-model:value="release.draft.values.sshPort" v-bind="fieldProps('values.sshPort')" data-testid="release-ssh-port" size="small" :disabled="!editing || release.saving" :show-button="false" /><small v-if="release.fieldErrors['values.sshPort']" id="release-error-values.sshPort" class="field-feedback" role="alert">{{ release.fieldErrors['values.sshPort'] }}</small></label>
            <label>连接超时（秒）<n-input-number v-model:value="release.draft.values.sshTimeoutSeconds" v-bind="fieldProps('values.sshTimeoutSeconds')" data-testid="release-ssh-timeout" size="small" :disabled="!editing || release.saving" :show-button="false" /><small v-if="release.fieldErrors['values.sshTimeoutSeconds']" id="release-error-values.sshTimeoutSeconds" class="field-feedback" role="alert">{{ release.fieldErrors['values.sshTimeoutSeconds'] }}</small></label>
            <label>SSH 密码<n-input v-model:value="release.draft.credentials.sshPassword" v-bind="fieldProps('credentials.sshPassword')" data-testid="release-ssh-password" type="password" show-password-on="click" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['credentials.sshPassword']" id="release-error-credentials.sshPassword" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.sshPassword'] }}</small></label>
            <label class="span-2">SSH 私钥（可选，保存时加密）<n-input v-model:value="release.draft.credentials.sshPrivateKey" v-bind="fieldProps('credentials.sshPrivateKey')" data-testid="release-ssh-private-key" type="textarea" size="small" :disabled="!editing || release.saving" placeholder="粘贴无口令的 RSA、Ed25519 或 ECDSA 私钥内容" :autosize="{ minRows: 2, maxRows: 4 }" /><small v-if="release.fieldErrors['credentials.sshPrivateKey']" id="release-error-credentials.sshPrivateKey" class="field-feedback" role="alert">{{ release.fieldErrors['credentials.sshPrivateKey'] }}</small></label>
          </div></div>
          <div class="form-section"><h3>远端目录</h3><div class="form-grid">
            <label>数据根目录<n-input v-model:value="release.draft.values.aioDataRoot" v-bind="fieldProps('values.aioDataRoot')" data-testid="release-data-root" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['values.aioDataRoot']" id="release-error-values.aioDataRoot" class="field-feedback" role="alert">{{ release.fieldErrors['values.aioDataRoot'] }}</small></label>
            <label>部署根目录<n-input v-model:value="release.draft.values.aioDeployRoot" v-bind="fieldProps('values.aioDeployRoot')" data-testid="release-deploy-root" size="small" :disabled="!editing || release.saving" /><small v-if="release.fieldErrors['values.aioDeployRoot']" id="release-error-values.aioDeployRoot" class="field-feedback" role="alert">{{ release.fieldErrors['values.aioDeployRoot'] }}</small></label>
            <div v-if="release.profile" class="agent-script-row span-2">
              <span class="agent-script-label">一体机脚本</span>
              <n-input :value="agentScriptDisplay" data-testid="release-agent-script" size="small" disabled :title="agentScriptTitle" />
              <div class="agent-script-actions" role="group" aria-label="一体机脚本操作">
                <n-button size="small" secondary type="primary" data-testid="release-agent-replace" :disabled="release.saving || release.agentReplacing" :loading="release.agentReplacing" @click="chooseAgentScript">更换</n-button>
                <n-button size="small" secondary type="primary" data-testid="release-agent-view" :disabled="release.saving || release.agentReplacing || release.agentOpening" :loading="release.agentOpening" @click="viewAgentScript">查看</n-button>
              </div>
            </div>
          </div></div>
        </div>
      </section>

      <section class="data-panel template-panel">
        <header class="panel-heading"><span class="feature-icon operation"><Code2 :size="19" /></span><div><strong>部署模板</strong></div></header>
        <n-tabs v-model:value="activeTemplate" type="line" size="small" class="template-tabs">
          <n-tab-pane name="env" tab=".env 模板"><n-input v-model:value="release.draft.values.envTemplate" v-bind="fieldProps('values.envTemplate')" data-testid="release-env-template" type="textarea" :disabled="!editing || release.saving" class="code-editor" :autosize="false" /></n-tab-pane>
          <n-tab-pane name="compose" tab="docker-compose.yml"><n-input v-model:value="release.draft.values.composeTemplate" v-bind="fieldProps('values.composeTemplate')" data-testid="release-compose-template" type="textarea" :disabled="!editing || release.saving" class="code-editor" :autosize="false" /></n-tab-pane>
          <n-tab-pane name="host-info" tab="host-info.json"><n-input v-model:value="release.draft.values.hostInfoTemplate" v-bind="fieldProps('values.hostInfoTemplate')" data-testid="release-host-info-template" type="textarea" :disabled="!editing || release.saving" class="code-editor" :autosize="false" /></n-tab-pane>
        </n-tabs>
        <footer class="template-footer" :class="{ 'has-validation-error': activeTemplateError }">
          <span v-if="activeTemplateError" :id="`release-error-${activeTemplateField}`" role="alert">{{ activeTemplateError }}</span>
          <span v-else-if="release.validation?.valid"><CheckCircle2 :size="15" />校验通过 · {{ release.validation.composeServices.length }} 个服务 · {{ release.validation.recognizedPlaceholderCount }} 个模板变量</span>
          <span v-else>尚未校验当前编辑内容</span>
        </footer>
      </section>
    </div>

  </section>
</template>

<style scoped>
.release-label-term {
  white-space: nowrap;
}

.parameter-panel .form-grid label > .field-feedback {
  grid-column: 2;
  width: 100%;
  color: var(--inx-color-danger);
  font-size: var(--inx-font-size-table);
  font-weight: 400;
  line-height: 1.4;
  text-align: left;
  overflow-wrap: anywhere;
}

.template-footer.has-validation-error {
  color: var(--inx-color-danger);
  overflow-wrap: anywhere;
}

.agent-script-row {
  display: grid;
  width: 100%;
  grid-template-columns: 8em minmax(0, 1fr) auto;
  align-items: center;
  gap: 5px;
  color: var(--inx-color-text-secondary);
  font-size: var(--inx-font-size-base);
  font-weight: 400;
}

.agent-script-label {
  justify-self: end;
  text-align: right;
}

.agent-script-row > .n-input {
  width: 100%;
}

.agent-script-actions {
  display: flex;
  align-items: center;
  gap: 7px;
  min-width: max-content;
  padding-left: 10px;
  border-left: 1px solid var(--inx-color-border);
}

.agent-script-actions .n-button {
  min-width: 48px;
}

</style>
