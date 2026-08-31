<script setup lang="ts">
import {
  NAlert,
  NButton,
  NEmpty,
  NInput,
  NInputNumber,
  NModal,
  NPopconfirm,
  NSpace,
  NTabPane,
  NTabs,
  NTag,
  useMessage
} from "naive-ui";
import {
  CheckCircle2,
  Code2,
  Download,
  Eye,
  FileKey2,
  KeyRound,
  Pencil,
  RefreshCw,
  RotateCw,
  Save,
  ShieldCheck,
  Upload,
  Undo2
} from "lucide-vue-next";
import { onMounted, reactive, ref, watch } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { useProjectStore } from "@/stores/projects";
import { useReleaseProfileStore } from "@/stores/releaseProfile";

const projects = useProjectStore();
const release = useReleaseProfileStore();
const message = useMessage();
const dialogs = useSystemDialogAdapter();
const editing = ref(false);
const activeTemplate = ref("env");
const hostKeyOpen = ref(false);
const keyManagementOpen = ref(false);
const keyPassphrase = ref("");
const keyPassphraseConfirmation = ref("");
const hostKeyForm = reactive({ host: "", port: 22 });

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
    keyManagementOpen.value = false;
    clearKeyPassphrases();
    await load();
  }
);

function startEdit() {
  release.resetDraft();
  editing.value = true;
}

function clearKeyPassphrases() {
  keyPassphrase.value = "";
  keyPassphraseConfirmation.value = "";
}

function checkedPassphrase(requireConfirmation: boolean) {
  if (keyPassphrase.value.length < 12) {
    throw new Error("密钥包口令至少需要12个字节");
  }
  if (requireConfirmation && keyPassphrase.value !== keyPassphraseConfirmation.value) {
    throw new Error("两次输入的密钥包口令不一致");
  }
  return keyPassphrase.value;
}

async function exportMasterKey() {
  try {
    const passphrase = checkedPassphrase(true);
    const safeProjectName = (projects.activeProject?.name ?? "inxaiot-project")
      .replace(/[<>:"/\\|?*]/g, "_");
    const filePath = await dialogs.saveFile(
      "导出项目主密钥",
      [{ name: "INX 项目主密钥包", extensions: ["inxkey"] }],
      `${safeProjectName}-release-master-key.inxkey`
    );
    if (!filePath) return;
    const result = await release.exportMasterKey(filePath, passphrase);
    clearKeyPassphrases();
    message.success(result.message);
  } catch (cause) {
    message.error(commandErrorText(cause, "导出项目主密钥失败"));
  }
}

async function importMasterKey() {
  try {
    const passphrase = checkedPassphrase(false);
    const filePath = await dialogs.selectFile("导入项目主密钥", [
      { name: "INX 项目主密钥包", extensions: ["inxkey"] }
    ]);
    if (!filePath) return;
    const result = await release.importMasterKey(filePath, passphrase);
    clearKeyPassphrases();
    message.success(result.message);
  } catch (cause) {
    message.error(commandErrorText(cause, "导入项目主密钥失败"));
  }
}

async function rotateMasterKey() {
  try {
    const result = await release.rotateMasterKey();
    clearKeyPassphrases();
    message.warning(`${result.message}。请立即设置口令并导出新密钥包。`, { duration: 8000 });
  } catch (cause) {
    message.error(commandErrorText(cause, "轮换项目主密钥失败"));
  }
}

function cancelEdit() {
  release.resetDraft();
  editing.value = false;
}

async function validate() {
  try {
    const result = await release.validate();
    message.success(`配置校验通过，识别 ${result.recognizedPlaceholderCount} 个模板变量`);
  } catch (cause) {
    message.error(commandErrorText(cause, "配置校验失败"));
  }
}

async function save() {
  try {
    const profile = await release.save();
    editing.value = false;
    message.success(`发布参数已保存为版本 ${profile.version}`);
  } catch (cause) {
    if (release.conflict) {
      message.error("配置已被其他实例更新，请刷新后重新编辑");
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

async function captureHostKey() {
  try {
    const observation = await release.captureHostKey(hostKeyForm.host, hostKeyForm.port);
    if (observation.state === "confirmed") message.success("主机密钥与已确认记录一致");
    else if (observation.state === "changed") message.error("主机密钥发生变化，连接已阻断");
    else message.warning("首次连接，请核对并确认主机指纹");
  } catch (cause) {
    message.error(commandErrorText(cause, "捕获主机密钥失败"));
  }
}

async function confirmHostKey(replaceChanged: boolean) {
  try {
    await release.confirmHostKey(replaceChanged);
    message.success(replaceChanged ? "已重新确认变化后的主机密钥" : "主机密钥已确认");
  } catch (cause) {
    message.error(commandErrorText(cause, "确认主机密钥失败"));
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
          <span v-else>保存前执行真实校验</span>
          <span v-if="release.profile">最后修改：{{ release.profile.updatedBy }} · {{ release.profile.updatedAt }}</span>
          <span class="warning"><Eye :size="14" />可查看完整凭据</span>
        </div>
      </div>
      <div class="page-actions">
        <n-tag v-if="release.profile" size="small" :bordered="false" type="info">版本 {{ release.profile.version }}</n-tag>
        <n-button size="small" secondary data-testid="release-key-management-open" :disabled="!projects.isReady" @click="keyManagementOpen = true"><template #icon><KeyRound /></template>密钥备份</n-button>
        <n-button size="small" secondary data-testid="host-key-open" :disabled="!projects.isReady" @click="hostKeyOpen = true"><template #icon><ShieldCheck /></template>主机密钥</n-button>
        <n-button v-if="!editing" size="small" type="primary" data-testid="release-edit" :disabled="!projects.isReady" @click="startEdit"><template #icon><Pencil /></template>{{ release.profile ? "编辑配置" : "新建配置" }}</n-button>
        <template v-else>
          <n-button size="small" @click="cancelEdit"><template #icon><Undo2 /></template>取消</n-button>
          <n-button size="small" secondary @click="validate">校验配置</n-button>
          <n-button size="small" type="primary" data-testid="release-save" :loading="release.saving" @click="save"><template #icon><Save /></template>保存配置</n-button>
        </template>
      </div>
    </header>

    <n-alert v-if="!projects.activeProject" type="info" :show-icon="false">请先在顶部新增并连接项目。</n-alert>
    <n-alert v-else-if="!projects.isReady" type="warning" :show-icon="false">
      当前项目尚未就绪：{{ projects.activeProject.statusMessage }}。请先完成数据库连接、Schema 初始化和平台登录。
    </n-alert>
    <n-alert v-if="release.conflict" type="error" :show-icon="false">
      发布配置已被其他实例更新，本次保存未覆盖远端数据。
      <n-button size="tiny" text type="primary" @click="reloadAfterConflict"><template #icon><RefreshCw /></template>加载最新版本</n-button>
    </n-alert>
    <n-alert v-if="release.error && !release.conflict" type="error" :show-icon="false">{{ release.error }}</n-alert>

    <n-empty v-if="projects.isReady && !release.profile && !editing && !release.loading" description="当前项目尚未创建发布参数" class="page-empty" />

    <div v-if="projects.isReady && (release.profile || editing)" class="release-layout">
      <section class="data-panel parameter-panel">
        <header class="panel-heading"><span class="feature-icon info"><FileKey2 :size="19" /></span><div><strong>项目发布参数</strong><small>保存后其他工作台实例读取同一版本</small></div></header>
        <div class="parameter-scroll">
          <div class="form-section"><h3>平台接入</h3><div class="form-grid">
            <label>平台主机<n-input v-model:value="release.draft.values.platformHost" data-testid="release-platform-host" size="small" :disabled="!editing" /></label>
            <label>平台 API 端口<n-input-number v-model:value="release.draft.values.platformApiPort" data-testid="release-api-port" size="small" :disabled="!editing" :show-button="false" /></label>
            <label class="span-2">平台 API AuthKey<n-input v-model:value="release.draft.credentials.platformAuthKey" data-testid="release-auth-key" size="small" :disabled="!editing" /></label>
          </div></div>
          <div class="form-section"><h3>MQTT 配置</h3><div class="form-grid">
            <label>平台 MQTT 主机<n-input v-model:value="release.draft.values.platformMqttHost" data-testid="release-mqtt-host" size="small" :disabled="!editing" /></label>
            <label>平台 MQTT 端口<n-input-number v-model:value="release.draft.values.platformMqttPort" data-testid="release-mqtt-port" size="small" :disabled="!editing" :show-button="false" /></label>
            <label>平台 MQTT 账号<n-input v-model:value="release.draft.credentials.platformMqttUser" data-testid="release-mqtt-user" size="small" :disabled="!editing" /></label>
            <label>平台 MQTT 密码<n-input v-model:value="release.draft.credentials.platformMqttPassword" data-testid="release-mqtt-password" type="password" show-password-on="click" size="small" :disabled="!editing" /></label>
            <label>一体机 MQTT 账号<n-input v-model:value="release.draft.credentials.aioMqttUser" data-testid="release-aio-mqtt-user" size="small" :disabled="!editing" /></label>
            <label>一体机 MQTT 密码<n-input v-model:value="release.draft.credentials.aioMqttPassword" data-testid="release-aio-mqtt-password" type="password" show-password-on="click" size="small" :disabled="!editing" /></label>
          </div></div>
          <div class="form-section"><h3>SSH 连接</h3><div class="form-grid">
            <label>SSH 用户名<n-input v-model:value="release.draft.credentials.sshUser" data-testid="release-ssh-user" size="small" :disabled="!editing" /></label>
            <label>SSH 端口<n-input-number v-model:value="release.draft.values.sshPort" data-testid="release-ssh-port" size="small" :disabled="!editing" :show-button="false" /></label>
            <label>连接超时（秒）<n-input-number v-model:value="release.draft.values.sshTimeoutSeconds" data-testid="release-ssh-timeout" size="small" :disabled="!editing" :show-button="false" /></label>
            <label>SSH 密码<n-input v-model:value="release.draft.credentials.sshPassword" type="password" show-password-on="click" size="small" :disabled="!editing" /></label>
            <label class="span-2">SSH 私钥（可选）<n-input v-model:value="release.draft.credentials.sshPrivateKey" data-testid="release-ssh-private-key" type="textarea" size="small" :disabled="!editing" placeholder="粘贴项目共享私钥内容" :autosize="{ minRows: 2, maxRows: 4 }" /></label>
          </div></div>
          <div class="form-section"><h3>远端目录</h3><div class="form-grid">
            <label>数据根目录<n-input v-model:value="release.draft.values.aioDataRoot" data-testid="release-data-root" size="small" :disabled="!editing" /></label>
            <label>部署根目录<n-input v-model:value="release.draft.values.aioDeployRoot" data-testid="release-deploy-root" size="small" :disabled="!editing" /></label>
          </div></div>
        </div>
      </section>

      <section class="data-panel template-panel">
        <header class="panel-heading"><span class="feature-icon operation"><Code2 :size="19" /></span><div><strong>部署模板</strong><small>未知占位符和未定义变量会阻止保存</small></div></header>
        <n-tabs v-model:value="activeTemplate" type="line" size="small" class="template-tabs">
          <n-tab-pane name="env" tab=".env 模板"><n-input v-model:value="release.draft.values.envTemplate" data-testid="release-env-template" type="textarea" :disabled="!editing" class="code-editor" :autosize="false" /></n-tab-pane>
          <n-tab-pane name="compose" tab="docker-compose.yml"><n-input v-model:value="release.draft.values.composeTemplate" data-testid="release-compose-template" type="textarea" :disabled="!editing" class="code-editor" :autosize="false" /></n-tab-pane>
        </n-tabs>
        <footer class="template-footer">
          <span v-if="release.validation?.valid"><CheckCircle2 :size="15" />校验通过 · {{ release.validation.recognizedPlaceholderCount }} 个模板变量</span>
          <span v-else>尚未校验当前编辑内容</span>
        </footer>
      </section>
    </div>

    <n-modal v-model:show="hostKeyOpen" preset="card" title="SSH 主机密钥" class="host-key-modal" :bordered="false">
      <p class="modal-description">先捕获目标主机当前指纹。首次连接必须确认；已确认指纹发生变化时默认阻断，只能显式重新确认。</p>
      <div class="host-key-capture-row">
        <n-input v-model:value="hostKeyForm.host" data-testid="host-key-host" placeholder="一体机 IP 或主机名" />
        <n-input-number v-model:value="hostKeyForm.port" :show-button="false" :min="1" :max="65535" />
        <n-button type="primary" secondary data-testid="host-key-capture" :loading="release.hostKeyLoading" @click="captureHostKey">捕获指纹</n-button>
      </div>
      <n-alert v-if="release.hostKeyObservation" :type="release.hostKeyObservation.state === 'confirmed' ? 'success' : release.hostKeyObservation.state === 'changed' ? 'error' : 'warning'" :show-icon="false">
        <strong>{{ release.hostKeyObservation.state === "confirmed" ? "指纹一致" : release.hostKeyObservation.state === "changed" ? "指纹发生变化，连接已阻断" : "首次连接，等待确认" }}</strong>
        <div class="fingerprint-value">当前：{{ release.hostKeyObservation.algorithm }} · {{ release.hostKeyObservation.fingerprint }}</div>
        <div v-if="release.hostKeyObservation.expectedFingerprint" class="fingerprint-value">已确认：{{ release.hostKeyObservation.expectedFingerprint }}</div>
      </n-alert>
      <div v-if="release.hostKeyObservation && release.hostKeyObservation.state !== 'confirmed'" class="host-key-actions">
        <n-button v-if="release.hostKeyObservation.state === 'unconfirmed'" type="primary" data-testid="host-key-confirm" @click="confirmHostKey(false)">确认首次指纹</n-button>
        <n-popconfirm v-else positive-text="重新确认" negative-text="保持阻断" @positive-click="confirmHostKey(true)">
          <template #trigger><n-button type="error" data-action-owner="popconfirm">重新确认变化后的指纹</n-button></template>
          请先通过现场可信渠道核对当前指纹。重新确认后旧指纹将被替换，确定继续？
        </n-popconfirm>
      </div>
      <div class="host-key-list">
        <strong>已确认主机</strong>
        <n-empty v-if="!release.hostKeys.length" description="暂无已确认主机密钥" size="small" />
        <div v-for="key in release.hostKeys" :key="`${key.host}:${key.port}`" class="host-key-list-item">
          <span>{{ key.host }}:{{ key.port }}</span><code>{{ key.fingerprint }}</code><n-tag size="small" type="success" :bordered="false">已确认</n-tag>
        </div>
      </div>
      <template #footer><n-space justify="end"><n-button @click="hostKeyOpen = false">关闭</n-button></n-space></template>
    </n-modal>

    <n-modal
      v-model:show="keyManagementOpen"
      preset="card"
      title="项目主密钥备份与轮换"
      class="host-key-modal"
      :bordered="false"
      @after-leave="clearKeyPassphrases"
    >
      <p class="modal-description">
        发布凭据由随机项目主密钥加密，不再绑定数据库密码。密钥只保存在本机安全存储；其他电脑必须导入与当前项目匹配的口令保护密钥包。
      </p>
      <n-alert type="warning" :show-icon="false">
        密钥包口令无法找回。轮换会立即使其他电脑上的旧密钥失效，轮换后必须重新导出并安全分发。
      </n-alert>
      <div class="host-key-list key-transfer-fields">
        <label>
          <span>密钥包口令</span>
          <n-input
            v-model:value="keyPassphrase"
            data-testid="release-key-passphrase"
            type="password"
            show-password-on="mousedown"
            placeholder="至少12个字节"
            autocomplete="new-password"
          />
        </label>
        <label>
          <span>再次输入口令（导出时校验）</span>
          <n-input
            v-model:value="keyPassphraseConfirmation"
            data-testid="release-key-passphrase-confirmation"
            type="password"
            show-password-on="mousedown"
            placeholder="再次输入相同口令"
            autocomplete="new-password"
          />
        </label>
      </div>
      <div class="host-key-actions">
        <n-button data-testid="release-key-export" :disabled="!release.profile" :loading="release.keyOperationLoading" @click="exportMasterKey">
          <template #icon><Download /></template>导出口令保护密钥包
        </n-button>
        <n-button data-testid="release-key-import" :loading="release.keyOperationLoading" @click="importMasterKey">
          <template #icon><Upload /></template>导入并验证密钥包
        </n-button>
        <n-popconfirm positive-text="确认轮换" negative-text="取消" @positive-click="rotateMasterKey">
          <template #trigger>
            <n-button type="warning" data-testid="release-key-rotate" data-action-owner="popconfirm" :disabled="!release.profile" :loading="release.keyOperationLoading">
              <template #icon><RotateCw /></template>轮换主密钥
            </n-button>
          </template>
          轮换会使其他电脑上的旧密钥立即失效。确定继续？
        </n-popconfirm>
      </div>
      <template #footer>
        <n-space justify="end"><n-button @click="keyManagementOpen = false">关闭</n-button></n-space>
      </template>
    </n-modal>
  </section>
</template>

<style scoped>
.key-transfer-fields {
  display: grid;
  gap: 12px;
  margin-top: 16px;
}

.key-transfer-fields label {
  display: grid;
  gap: 6px;
}
</style>
