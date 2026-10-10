<script setup lang="ts">
import { onBeforeUnmount, reactive, ref, watch } from "vue";
import { NAlert, NButton, NForm, NFormItem, NInput, NModal, NPopconfirm, useMessage } from "naive-ui";
import { useAioAdapter } from "@/shared/api/aioAdapter";
import { commandErrorCode, commandErrorText } from "@/shared/api/errors";
import type { AioNodeDetail, InventoryValues } from "@/shared/model/aio";
import { projectSpacePath, type ProjectSpaceNode } from "@/shared/model/projectSpace";
import { AIO_ADDRESS_MAX_LENGTH, aioAddressAfterSpaceChange, aioAddressError, aioAddressSuggestion, hasAioSpace } from "@/shared/model/aioLocation";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";
import OperationFormTheme from "@/shared/components/OperationFormTheme.vue";

const props = defineProps<{ projectId: string; detail?: AioNodeDetail }>();
const show = defineModel<boolean>("show", { default: false });
const emit = defineEmits<{ saved: [] }>();
const message = useMessage();
const form = reactive({ name: "", ip: "", buildingId: "", addrAlias: "" });
const base = ref<AioNodeDetail>();
const spaces = ref<ProjectSpaceNode[]>([]);
const directoryError = ref("");
const loading = ref(false), saving = ref(false), error = ref("");
const occupied = ref("");
let generation = 0;
const addressEdited = ref(false);
let initialForm = { ...form };

function fill(detail: AioNodeDetail) {
  const node = detail.platform;
  Object.assign(form, { name: node?.name ?? detail.node.name, ip: node?.ip ?? detail.node.ip,
    buildingId: (node?.buildingId ?? detail.node.buildingId)?.replace(/^0$/, "") ?? "",
    addrAlias: node?.addrAlias ?? detail.node.location });
}
watch([show, () => props.projectId], async ([open]) => {
  const request = ++generation; saving.value = false; occupied.value = ""; error.value = "";
  if (!open || !props.detail) return;
  base.value = JSON.parse(JSON.stringify(props.detail)) as AioNodeDetail; fill(base.value);
  addressEdited.value = false; initialForm = { ...form };
  spaces.value = []; directoryError.value = ""; loading.value = true;
  try {
    const result = await useAioAdapter().listSpaces(props.projectId);
    if (request === generation) {
      spaces.value = result;
      if (!addressEdited.value && !form.addrAlias.trim()) {
        form.addrAlias = aioAddressSuggestion(result, form.buildingId);
        initialForm.addrAlias = form.addrAlias;
      }
    }
  } catch (cause) { if (request === generation) directoryError.value = commandErrorText(cause, "空间目录读取失败"); }
  finally { if (request === generation) loading.value = false; }
}, { immediate: true });
onBeforeUnmount(() => { ++generation; });

function selectSpace(value: string | null) {
  const buildingId = value || "";
  if (buildingId === form.buildingId) return;
  form.addrAlias = aioAddressAfterSpaceChange(spaces.value, form.buildingId, buildingId, form.addrAlias);
  form.buildingId = buildingId;
  addressEdited.value = true;
}

async function save(takeover = false) {
  if (saving.value || loading.value || !base.value) return;
  const project = props.projectId, request = generation, adapter = useAioAdapter();
  const current = () => request === generation && props.projectId === project && show.value;
  let before = base.value;
  const draft = { ...form };
  error.value = ""; saving.value = true;
  try {
    if (takeover) {
      const fresh = await adapter.getNodeDetail(project, before.node.mac);
      if (!current()) return;
      // 接手后读取最新资料；未改动字段沿用最新值，保留用户明确修改的字段。
      const original = initialForm;
      fill(fresh);
      if (!form.addrAlias.trim()) form.addrAlias = aioAddressSuggestion(spaces.value, form.buildingId);
      initialForm = { ...form };
      for (const key of ["name", "ip"] as const) if (draft[key] !== original[key]) form[key] = draft[key];
      if (draft.buildingId !== original.buildingId) {
        form.buildingId = draft.buildingId; form.addrAlias = draft.addrAlias;
      } else if (draft.addrAlias !== original.addrAlias) form.addrAlias = draft.addrAlias;
      base.value = fresh; before = fresh;
    }
    if (!form.name.trim() || !form.ip.trim()) throw new Error("请填写名称和 IP 地址");
    const addressError = aioAddressError(form.buildingId, form.addrAlias);
    if (addressError) throw new Error(addressError);
    const values: InventoryValues = { name: form.name.trim(), ip: form.ip.trim(), mac: before.node.mac,
      buildingId: form.buildingId || undefined, spacePath: projectSpacePath(spaces.value, form.buildingId) || undefined,
      addrAlias: form.addrAlias.trim() || undefined };
    await adapter.updateNode(project, { mac: before.node.mac, expectedVersion: before.node.version,
      platformBase: before.platform ?? null, values, forceTakeover: takeover });
    if (!current()) return;
    message.success(before.platform ? "资料已保存到平台" : "资料已保存到当前电脑");
    show.value = false; emit("saved");
  } catch (cause) {
    if (!current()) return;
    if (commandErrorCode(cause) === "AIO_EDIT_LOCKED") {
      let instance = "另一台电脑";
      try { instance = JSON.parse((cause as { params: { details: string } }).params.details).instance || instance; } catch { /* 保留通用提示 */ }
      occupied.value = `该一体机正在由 ${instance} 操作。`;
    } else { error.value = commandErrorText(cause, "资料保存失败"); occupied.value = ""; }
  } finally { if (current()) saving.value = false; }
}
</script>

<template>
  <n-modal :show="show" preset="card" title="编辑一体机" class="aio-edit-dialog" style="width: min(660px, calc(100vw - 32px))" :bordered="false" :mask-closable="!saving" :close-on-esc="!saving" :closable="!saving" @update:show="value => { if (!saving) show = value; }">
    <operation-form-theme>
      <p class="edit-note">{{ base?.platform ? '已注册一体机：保存后直接更新平台资料。' : '未注册一体机：资料只保存在当前电脑，部署后再注册到平台。' }}</p>
      <n-alert v-if="error" type="error" class="edit-alert" data-testid="aio-edit-error">{{ error }}</n-alert>
      <n-alert v-if="directoryError" type="warning" class="edit-alert">{{ directoryError }}；空间信息暂不能调整。</n-alert>
      <n-alert v-if="occupied" type="warning" class="edit-alert">
        {{ occupied }}接手后，对方将不能继续提交或执行设备操作。
        <n-popconfirm positive-text="确认接手" negative-text="取消" @positive-click="save(true)">
          <template #trigger><n-button size="small" :disabled="saving">接手并保存</n-button></template>
          确认已核实对方的操作可以停止？接手后会重新读取最新资料，再保存本次修改。
        </n-popconfirm>
      </n-alert>
      <n-form label-placement="top" size="small" :disabled="saving" @submit.prevent="save()">
        <div class="edit-grid">
          <n-form-item label="名称" required :show-feedback="false"><n-input v-model:value="form.name" :maxlength="32" data-testid="aio-edit-name" /></n-form-item>
          <n-form-item label="IP 地址" required :show-feedback="false"><n-input v-model:value="form.ip" :maxlength="32" data-testid="aio-edit-ip" /></n-form-item>
          <n-form-item label="MAC 地址" :show-feedback="false"><n-input :value="base?.node.mac" readonly disabled /></n-form-item>
          <n-form-item label="空间位置" :show-feedback="false"><project-space-select :model-value="form.buildingId" :spaces="spaces" :disabled="saving || loading || !!directoryError" placeholder="可选，请选择空间" :fallback-label="base?.node.spacePath || '原空间待核实'" @update:model-value="selectSpace" /></n-form-item>
          <n-form-item label="具体位置" :required="hasAioSpace(form.buildingId)" :show-feedback="false" class="edit-wide"><n-input v-model:value="form.addrAlias" :maxlength="AIO_ADDRESS_MAX_LENGTH" :placeholder="hasAioSpace(form.buildingId) ? '请填写具体位置，可修改自动填入的楼栋楼层' : '例如：门口弱电柜（可选）'" data-testid="aio-edit-address" @update:value="addressEdited = true" /></n-form-item>
        </div>
      </n-form>
      <div class="edit-footer"><n-button :disabled="saving" @click="show = false">取消</n-button><n-button type="primary" :loading="saving" :disabled="loading || saving" data-testid="aio-edit-save" @click="save()">保存修改</n-button></div>
    </operation-form-theme>
  </n-modal>
</template>

<style scoped>
.edit-note { margin: 0 0 12px; color: var(--inx-color-text-secondary); font-size: var(--inx-font-size-table); }
.edit-alert { margin-bottom: 12px; }
.edit-grid { display: grid; grid-template-columns: repeat(2,minmax(0,1fr)); gap: 12px 16px; }
.edit-wide { grid-column: 1 / -1; }
.edit-footer { display: flex; justify-content: end; gap: 8px; margin-top: 16px; }
@media (max-width: 540px) { .edit-grid { grid-template-columns: 1fr; } }
</style>
