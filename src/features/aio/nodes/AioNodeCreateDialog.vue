<script setup lang="ts">
import { onBeforeUnmount, reactive, ref, watch } from "vue";
import { NAlert, NButton, NForm, NFormItem, NInput, NModal, useMessage } from "naive-ui";
import { useAioAdapter } from "@/shared/api/aioAdapter";
import { commandErrorText } from "@/shared/api/errors";
import OperationFormTheme from "@/shared/components/OperationFormTheme.vue";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import { projectSpacePath, type ProjectSpaceNode } from "@/shared/model/projectSpace";
import type { AioImportSession, InventoryApplyOutcome, InventoryValues } from "@/shared/model/aio";

const props = defineProps<{ projectId: string }>();
const show = defineModel<boolean>("show", { default: false });
const emit = defineEmits<{ created: [outcome: InventoryApplyOutcome] }>();
const message = useMessage();
const form = reactive<InventoryValues>({ name: "", ip: "", mac: "", location: "", remark: "" });
const error = ref("");
const busy = ref(false);
const spaces = ref<ProjectSpaceNode[]>([]);
const spacesLoading = ref(false);
const spacesError = ref("");
const preview = ref<AioImportSession>();
let request = 0;
let spaceRequest = 0;
const applyingSessionIds = new Set<string>();

async function discardPreview(session = preview.value) {
  if (!session) return;
  await useAioAdapter().discardImport(session.localProjectId, session.id);
  if (preview.value?.id === session.id) preview.value = undefined;
}

watch([show, () => props.projectId], ([open]) => {
  ++request;
  const generation = ++spaceRequest;
  spacesLoading.value = false;
  busy.value = false;
  const previous = preview.value;
  preview.value = undefined;
  if (previous && !applyingSessionIds.has(previous.id)) void discardPreview(previous).catch((cause) => message.warning(commandErrorText(cause, "新增预览尚未关闭，请到导入清单中处理")));
  if (!open) return;
  Object.assign(form, { name: "", ip: "", mac: "", buildingId: undefined, addrAlias: "", location: "", remark: "" });
  error.value = "";
  spaces.value = []; spacesError.value = "";
  const projectId = props.projectId;
  spacesLoading.value = true;
  void useAioAdapter().listSpaces(projectId).then(value => {
    if (spaceRequest === generation && props.projectId === projectId) spaces.value = value;
  }).catch(cause => {
    if (spaceRequest === generation && props.projectId === projectId) spacesError.value = commandErrorText(cause, "空间目录读取失败");
  }).finally(() => { if (spaceRequest === generation) spacesLoading.value = false; });
}, { immediate: true });
onBeforeUnmount(() => {
  ++request;
  ++spaceRequest;
  if (preview.value && !applyingSessionIds.has(preview.value.id)) void discardPreview().catch(() => undefined);
});

async function close() {
  if (busy.value) return;
  const generation = ++request;
  busy.value = true;
  try { await discardPreview(); if (request === generation) show.value = false; }
  catch (cause) { if (request === generation) error.value = commandErrorText(cause, "关闭新增预览失败，请重试"); }
  finally { if (request === generation) busy.value = false; }
}

async function save() {
  if (busy.value || !props.projectId) return;
  const values: InventoryValues = {
    name: form.name.trim(), ip: form.ip.trim(), mac: form.mac.trim(),
    buildingId: form.buildingId || undefined,
    spacePath: projectSpacePath(spaces.value, form.buildingId) || undefined,
    addrAlias: form.addrAlias?.trim() || undefined,
    remark: form.remark?.trim() || undefined
  };
  error.value = [!values.name && "请填写名称", !values.ip && "请填写 IP 地址", !values.mac && "请填写 MAC 地址"].filter(Boolean).join("；");
  if (error.value) return;
  const projectId = props.projectId, generation = ++request, adapter = useAioAdapter();
  const current = () => request === generation && props.projectId === projectId && show.value;
  let requestSession: AioImportSession | undefined;
  busy.value = true;
  try {
    if (!preview.value) {
      const result = await adapter.previewCreate(projectId, values);
      if (!current()) { await adapter.discardImport(projectId, result.session.id); return; }
      preview.value = result.session;
    }
    const session = preview.value!;
    requestSession = session;
    if (session.items[0]?.classification !== "new_pending") {
      throw new Error("该 MAC 已存在或资料有冲突，请核对后再新增");
    }
    applyingSessionIds.add(session.id);
    const outcome = await adapter.applyImport(projectId, session.id);
    applyingSessionIds.delete(session.id);
    if (preview.value?.id === session.id) preview.value = undefined;
    if (!current()) return;
    if (outcome.localSessionFinalized) message.success("一体机已保存到当前电脑");
    else message.warning("一体机已保存，新增预览未能结束，请在导入清单中核对处理，勿重复新增");
    show.value = false;
    emit("created", outcome);
  } catch (cause) {
    if (requestSession) applyingSessionIds.delete(requestSession.id);
    if (current()) {
      error.value = commandErrorText(cause, "新增一体机失败");
      try { await discardPreview(); }
      catch { if (current()) error.value += "；新增预览未能关闭，请重试或到导入清单中处理"; }
    } else if (requestSession) {
      await discardPreview(requestSession).catch(() => undefined);
    }
  } finally { if (current()) busy.value = false; }
}
</script>

<template>
  <n-modal :show="show" preset="card" title="新增一体机" class="aio-create-dialog" style="width: min(640px, calc(100vw - 32px))" :bordered="false" :mask-closable="!busy" :close-on-esc="!busy" :closable="!busy" @update:show="value => { if (!value) void close(); }">
    <operation-form-theme>
      <p class="aio-create-note">保存到当前电脑，供后续部署；部署后才注册到平台。</p>
      <n-alert v-if="error" type="error" class="aio-create-alert" data-testid="aio-create-error">{{ error }}</n-alert>
      <n-alert v-if="spacesError" type="warning" class="aio-create-alert">{{ spacesError }}；可暂不选择空间，稍后在详情中补充。</n-alert>
      <n-form label-placement="top" size="small" :disabled="busy || !!preview" @submit.prevent="save">
        <div class="aio-create-grid">
          <n-form-item label="名称" required :show-feedback="false"><n-input v-model:value="form.name" :maxlength="32" placeholder="请输入一体机名称" data-testid="aio-create-name" /></n-form-item>
          <n-form-item label="IP 地址" required :show-feedback="false"><n-input v-model:value="form.ip" placeholder="例如：192.168.3.79" data-testid="aio-create-ip" /></n-form-item>
          <n-form-item label="MAC 地址" required :show-feedback="false"><n-input v-model:value="form.mac" placeholder="例如：AA:BB:CC:DD:EE:01" data-testid="aio-create-mac" /></n-form-item>
          <n-form-item label="空间位置" :show-feedback="false"><project-space-select :model-value="form.buildingId" :spaces="spaces" :disabled="busy || !!preview || spacesLoading || !!spacesError" :placeholder="spacesLoading ? '正在读取空间' : '可选，请选择空间'" data-testid="aio-create-location" @update:model-value="form.buildingId = $event || undefined" /></n-form-item>
          <n-form-item label="具体位置" :show-feedback="false"><n-input v-model:value="form.addrAlias" :maxlength="128" placeholder="例如：门口弱电柜（可选）" data-testid="aio-create-address" /></n-form-item>
          <n-form-item label="备注" :show-feedback="false"><n-input v-model:value="form.remark" placeholder="可选" data-testid="aio-create-remark" /></n-form-item>
        </div>
      </n-form>
      <div class="aio-create-footer">
        <span class="aio-create-spacer"></span>
        <n-button :disabled="busy" @click="close">取消</n-button>
        <n-button type="primary" :disabled="busy" :loading="busy" data-testid="aio-create-save" @click="save">新增</n-button>
      </div>
    </operation-form-theme>
  </n-modal>
</template>

<style scoped>
.aio-create-note { margin: 0 0 12px; color: var(--inx-color-text-secondary); font-size: var(--inx-font-size-table); }
.aio-create-alert { margin-bottom: 12px; }
.aio-create-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 12px 16px; align-items: start; }
.aio-create-wide { grid-column: 1 / -1; }
.aio-create-footer { display: flex; gap: 8px; align-items: center; margin-top: 18px; }
.aio-create-spacer { flex: 1; }
@media (max-width: 540px) { .aio-create-grid { grid-template-columns: 1fr; } }
</style>
