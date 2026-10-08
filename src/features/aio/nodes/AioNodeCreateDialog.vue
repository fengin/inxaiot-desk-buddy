<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import { NAlert, NButton, NForm, NFormItem, NInput, NModal, useMessage } from "naive-ui";
import { useAioAdapter } from "@/shared/api/aioAdapter";
import { commandErrorText } from "@/shared/api/errors";
import OperationFormTheme from "@/shared/components/OperationFormTheme.vue";
import type { AioImportSession, InventoryApplyOutcome, InventoryValues } from "@/shared/model/aio";

const props = defineProps<{ projectId: string }>();
const show = defineModel<boolean>("show", { default: false });
const emit = defineEmits<{ created: [outcome: InventoryApplyOutcome] }>();
const message = useMessage();
const form = reactive<InventoryValues>({ name: "", ip: "", mac: "", location: "", remark: "" });
const error = ref("");
const busy = ref(false);
const preview = ref<AioImportSession>();
const takeover = computed(() => preview.value?.items[0]?.classification === "platform_existing");
let request = 0;
const applyingSessionIds = new Set<string>();

async function discardPreview(session = preview.value) {
  if (!session) return;
  await useAioAdapter().discardImport(session.localProjectId, session.id);
  if (preview.value?.id === session.id) preview.value = undefined;
}

watch([show, () => props.projectId], ([open]) => {
  ++request;
  busy.value = false;
  const previous = preview.value;
  preview.value = undefined;
  if (previous && !applyingSessionIds.has(previous.id)) void discardPreview(previous).catch((cause) => message.warning(commandErrorText(cause, "新增预览尚未关闭，请到导入清单中处理")));
  if (!open) return;
  Object.assign(form, { name: "", ip: "", mac: "", location: "", remark: "" });
  error.value = "";
});
onBeforeUnmount(() => {
  ++request;
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

async function revise() {
  if (busy.value) return;
  const generation = ++request;
  busy.value = true;
  try { await discardPreview(); if (request === generation) error.value = ""; }
  catch (cause) { if (request === generation) error.value = commandErrorText(cause, "释放新增预览失败，请重试"); }
  finally { if (request === generation) busy.value = false; }
}

async function save() {
  if (busy.value || !props.projectId) return;
  const values: InventoryValues = {
    name: form.name.trim(), ip: form.ip.trim(), mac: form.mac.trim(),
    location: form.location?.trim() || undefined, remark: form.remark?.trim() || undefined
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
      if (result.session.items[0]?.classification === "platform_existing") return;
    }
    const session = preview.value!;
    requestSession = session;
    if (!["new_pending", "platform_existing"].includes(session.items[0]?.classification ?? "")) {
      throw new Error("该 MAC 已存在或资料有冲突，请核对后再新增");
    }
    const wasTakeover = takeover.value;
    applyingSessionIds.add(session.id);
    const outcome = await adapter.applyImport(projectId, session.id);
    applyingSessionIds.delete(session.id);
    if (preview.value?.id === session.id) preview.value = undefined;
    if (!current()) return;
    if (outcome.localSessionFinalized) message.success(wasTakeover ? "已接管到工作台" : "一体机已新增");
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
      <p class="aio-create-note">保存后加入一体机列表，可继续部署升级。新增不会修改设备或平台业务资料。</p>
      <n-alert v-if="error" type="error" class="aio-create-alert" data-testid="aio-create-error">{{ error }}</n-alert>
      <n-alert v-if="takeover" type="info" class="aio-create-alert" data-testid="aio-create-takeover">该 MAC 已在平台登记，名称和 IP 一致。确认后将这台一体机加入工作台管理。</n-alert>
      <n-form label-placement="top" size="small" :disabled="busy || !!preview" @submit.prevent="save">
        <div class="aio-create-grid">
          <n-form-item label="名称" required :show-feedback="false"><n-input v-model:value="form.name" placeholder="请输入一体机名称" data-testid="aio-create-name" /></n-form-item>
          <n-form-item label="IP 地址" required :show-feedback="false"><n-input v-model:value="form.ip" placeholder="例如：192.168.3.79" data-testid="aio-create-ip" /></n-form-item>
          <n-form-item label="MAC 地址" required :show-feedback="false"><n-input v-model:value="form.mac" placeholder="例如：AA:BB:CC:DD:EE:01" data-testid="aio-create-mac" /></n-form-item>
          <n-form-item label="安装位置" :show-feedback="false"><n-input v-model:value="form.location" placeholder="可选" data-testid="aio-create-location" /></n-form-item>
          <n-form-item label="备注" :show-feedback="false" class="aio-create-wide"><n-input v-model:value="form.remark" placeholder="可选" data-testid="aio-create-remark" /></n-form-item>
        </div>
      </n-form>
      <div class="aio-create-footer">
        <n-button v-if="takeover" :disabled="busy" @click="revise">返回修改</n-button>
        <span class="aio-create-spacer"></span>
        <n-button :disabled="busy" @click="close">取消</n-button>
        <n-button type="primary" :disabled="busy" :loading="busy" data-testid="aio-create-save" @click="save">{{ takeover ? '确认接管' : '新增' }}</n-button>
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
