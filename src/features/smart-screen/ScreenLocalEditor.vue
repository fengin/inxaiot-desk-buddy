<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from "vue";
import { NAlert, NButton, NForm, NFormItem, NInput, NModal, NPopconfirm, NSelect, useMessage } from "naive-ui";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { normalizeScreenMac, validateLocalScreen } from "@/shared/model/screen";
import type { LocalScreenInput, SmartScreen } from "@/shared/model/screen";
import { screenPlatformDraftValues, screenPlatformFields } from "@/shared/model/screenRegistration";
import { screenSpaceLabel } from "@/shared/model/screenSpace";
import { buildProjectSpaceTree, getProjectSpacePath, projectSpacePath } from "@/shared/model/projectSpace";
import ProjectSpaceSelect from "@/shared/components/ProjectSpaceSelect.vue";

const show = defineModel<boolean>("show", { default: false });
const props = defineProps<{ screen?: SmartScreen }>();
const store = useSmartScreensStore();
const message = useMessage();
const platformEdit = computed(() => props.screen?.source === "platform");
const draft = computed(() => props.screen ? store.snapshot.platformDrafts?.[props.screen.id] : undefined);
const openedRevision = ref<number>();
const openedDraftRevision = ref<number>();
const draftConflicts = ref<string[]>([]);
const form = reactive<LocalScreenInput>({ name: "", ip: "", size: "10", mac: "", spaceId: null, spacePath: "", location: "" });
const automaticMac = ref("");
function validCollectedMac(screen?: SmartScreen) {
  const value = screen?.observedMac?.trim() ?? "";
  const normalized = normalizeScreenMac(value);
  return /^[0-9A-F]{12}$/.test(normalized) && !/^(0{12}|F{12})$/.test(normalized) ? value : "";
}
const collectedMac = computed(() => form.ip.trim() === props.screen?.ip ? validCollectedMac(props.screen) : "");
const canUseCollectedMac = computed(() => Boolean(collectedMac.value && normalizeScreenMac(form.mac.trim()) !== normalizeScreenMac(collectedMac.value)));
function useCollectedMac() {
  if (saving.value || !collectedMac.value) return;
  form.mac = collectedMac.value;
  automaticMac.value = "";
}
const spacesAvailable = computed(() => store.snapshot.spacesAvailable !== false);
const hasSpaceDirectory = computed(() => buildProjectSpaceTree(store.snapshot.spaces).length > 0);
const invalidSpace = computed(() => Boolean(spacesAvailable.value && form.spaceId && !getProjectSpacePath(store.snapshot.spaces, form.spaceId)));
const selectedSpaceLabel = computed(() => screenSpaceLabel(form, store.snapshot.spaces, spacesAvailable.value));
const originalSpace = ref<{ id: string | null; path: string }>({ id: null, path: "" });
const error = ref("");
const saving = ref(false);
let request = 0;
watch([show, () => props.screen?.id], ([open]) => {
  ++request; saving.value = false;
  if (!open) return;
  const screen = props.screen;
  const values = screen?.source === "platform" ? screenPlatformDraftValues(screen, draft.value) : screen;
  automaticMac.value = screen?.source === "local" && !screen.mac.trim() ? validCollectedMac(screen) : "";
  openedRevision.value = screen?.revision;
  openedDraftRevision.value = draft.value?.revision ?? 0;
  draftConflicts.value = [];
  if (screen?.source === "platform" && draft.value) {
    const current = screenPlatformFields(screen);
    const labels = { name: "名称", ip: "IP", mac: "MAC", size: "尺寸", spaceId: "空间", location: "安装位置" };
    for (const key of Object.keys(labels) as (keyof typeof labels)[]) {
      if (draft.value.values[key] !== draft.value.base[key] && current[key] !== draft.value.base[key] && current[key] !== draft.value.values[key]) {
        const value = key === "spaceId" ? projectSpacePath(store.snapshot.spaces, current.spaceId) || "待定空间" : current[key] || "未填写";
        draftConflicts.value.push(`${labels[key]}的当前平台值：${value}`);
      }
    }
  }
  originalSpace.value = { id: values?.spaceId ?? null, path: screen?.source === "platform" ? projectSpacePath(store.snapshot.spaces, values?.spaceId) || screen.spacePath || "" : screen?.spacePath ?? "" };
  Object.assign(form, {
    name: values?.name ?? "", ip: values?.ip ?? "", size: values?.size ?? "10", mac: automaticMac.value || values?.mac || "",
    spaceId: originalSpace.value.id, spacePath: originalSpace.value.path,
    location: values?.location ?? ""
  });
  error.value = "";
}, { immediate: true });
watch(() => form.ip, (ip) => {
  if (automaticMac.value && ip.trim() !== props.screen?.ip) {
    if (form.mac === automaticMac.value) form.mac = "";
    automaticMac.value = "";
  }
});
watch(() => store.projectId, () => { ++request; saving.value = false; show.value = false; });
onBeforeUnmount(() => { ++request; });
function selectSpace(value: string | null) {
  if (saving.value || (!spacesAvailable.value && value)) return;
  form.spaceId = value;
  form.spacePath = value ? projectSpacePath(store.snapshot.spaces, value) : "";
  error.value = "";
}
function restoreSpace() {
  if (saving.value) return;
  form.spaceId = originalSpace.value.id;
  form.spacePath = originalSpace.value.path;
  error.value = "";
}
async function save() {
  if (saving.value) return;
  const spaceId = form.spaceId || null;
  if (spaceId && spacesAvailable.value && !getProjectSpacePath(store.snapshot.spaces, spaceId)) {
    error.value = "原空间关联已失效，请重新选择有效空间或清空关联";
    return;
  }
  if (spaceId && !spacesAvailable.value && spaceId !== originalSpace.value.id) {
    error.value = "空间目录暂不可用，不能分配新的空间，请恢复原关联或清空";
    return;
  }
  const input: LocalScreenInput = {
    name: form.name.trim(), ip: form.ip.trim(), size: form.size, mac: form.mac.trim(),
    spaceId, spacePath: !spaceId ? "" : spacesAvailable.value ? projectSpacePath(store.snapshot.spaces, spaceId) : originalSpace.value.path,
    location: form.location.trim()
  };
  error.value = validateLocalScreen(input).join("；");
  if (error.value) return;
  const projectId = store.projectId, generation = ++request, id = props.screen?.id;
  saving.value = true;
  try {
    if (platformEdit.value && id) {
      await useScreenAdapter().savePlatformDraft(projectId, id, { name: input.name, ip: input.ip, mac: input.mac, size: input.size, spaceId: input.spaceId ?? null, location: input.location }, openedRevision.value, openedDraftRevision.value);
    } else await useScreenAdapter().saveLocal(projectId, input, id, openedRevision.value);
    if (generation !== request || store.projectId !== projectId || !show.value) return;
    await store.refresh();
    if (generation !== request || store.projectId !== projectId || !show.value) return;
    message.success(platformEdit.value ? "修改已保存到本机，请通过“注册/更新到平台”核对提交" : id ? "智能屏信息已更新" : "已加入本机管理，可开始运维");
    show.value = false;
  } catch (cause) { if (generation === request && store.projectId === projectId) error.value = (cause as Error).message; }
  finally { if (generation === request && store.projectId === projectId) saving.value = false; }
}
async function discard() {
  const id = props.screen?.id;
  if (!id || saving.value) return;
  const projectId = store.projectId, generation = ++request;
  saving.value = true;
  try {
    await useScreenAdapter().discardPlatformDraft(projectId, id);
    if (generation !== request || store.projectId !== projectId || !show.value) return;
    await store.refresh();
    if (generation !== request || store.projectId !== projectId || !show.value) return;
    message.success("已放弃本机待提交修改，平台资料保持原值"); show.value = false;
  } catch (cause) { if (generation === request && store.projectId === projectId) error.value = (cause as Error).message; }
  finally { if (generation === request && store.projectId === projectId) saving.value = false; }
}
</script>

<template>
  <n-modal v-model:show="show" preset="card" :title="screen ? '编辑智能屏' : '新增智能屏'" class="screen-dialog screen-editor" :bordered="false" :mask-closable="!saving" :close-on-esc="!saving" :closable="!saving">
    <div class="screen-callout">{{ platformEdit ? '修改先保存到本机，通过“注册/更新到平台”核对后提交。列表仍保留一条平台记录。修改 IP 只更新登记地址，不修改屏的网络配置。' : '保存为“平台未注册”记录，可先安装与维护，再通过“注册/更新到平台”登记。' }}</div>
    <n-alert v-if="draftConflicts.length" type="warning" class="screen-dialog-alert" data-testid="screen-draft-conflict">平台资料已有变化：{{ draftConflicts.join('；') }}。下面保留本机拟修改值，请核对后保存；本次保存将以当前平台资料重新建立比较依据。</n-alert>
    <n-alert v-if="error" type="error" class="screen-dialog-alert">{{ error }}</n-alert>
    <n-form label-placement="top" :disabled="saving" @submit.prevent="save">
      <div class="screen-form-grid">
        <n-form-item label="屏名称"><n-input v-model:value="form.name" placeholder="例如：A座 1F 电梯厅屏" :maxlength="32" /></n-form-item>
        <n-form-item label="设备 IP" required><n-input v-model:value="form.ip" placeholder="例如：192.0.2.120" /></n-form-item>
        <n-form-item label="屏尺寸"><n-select v-model:value="form.size" :options="[{ label: '10 寸屏', value: '10' }, { label: '4 寸屏', value: '4' }, { label: '待确认', value: 'unknown' }]" /></n-form-item>
        <n-form-item label="MAC 地址（可稍后采集）"><div class="screen-mac-field"><n-input v-model:value="form.mac" placeholder="可留空" @update:value="automaticMac = ''" />
          <div v-if="automaticMac && collectedMac && !canUseCollectedMac" class="screen-muted screen-mac-note" data-testid="screen-collected-mac-note">已带入本机采集的 MAC，保存后记入本机资料。</div>
          <div v-else-if="canUseCollectedMac" class="screen-muted screen-mac-note" data-testid="screen-collected-mac-note">本机采集：{{ collectedMac }}<n-button text size="tiny" type="primary" :disabled="saving" data-testid="screen-use-collected-mac" @click="useCollectedMac">填入采集值</n-button></div>
        </div></n-form-item>
        <n-form-item label="空间选择"><project-space-select :model-value="form.spaceId" :spaces="spacesAvailable ? store.snapshot.spaces : []" :disabled="saving || !spacesAvailable || !hasSpaceDirectory" :fallback-label="selectedSpaceLabel" placeholder="可选择任意层级，或暂不选择" aria-label="智能屏空间选择" @update:model-value="selectSpace" /></n-form-item>
        <n-form-item label="安装位置"><n-input v-model:value="form.location" placeholder="例如：东区电梯出口右侧" /></n-form-item>
        <div class="screen-span-all screen-muted">
          <template v-if="!spacesAvailable">空间目录暂不可用。{{ form.spaceId ? '保留原空间关联和缓存路径，恢复后再核验。' : '可先保存为待定空间，稍后选择空间。' }}</template>
          <template v-else-if="invalidSpace">原空间关联已失效，请重新选择有效空间或清空关联。</template>
          <template v-else-if="!hasSpaceDirectory">当前项目暂无可选空间，可先填写安装位置，稍后再关联。</template>
          <template v-else>可选择楼幢、楼层或下级空间；未选择时归入“待定空间”。</template>
          <n-button v-if="form.spaceId && (!spacesAvailable || !hasSpaceDirectory)" size="tiny" text type="primary" :disabled="saving" data-testid="screen-clear-space" @click="selectSpace(null)">清空空间关联</n-button>
          <n-button v-if="!spacesAvailable && originalSpace.id && form.spaceId !== originalSpace.id" size="tiny" text type="primary" :disabled="saving" data-testid="screen-restore-space" @click="restoreSpace">恢复原空间关联</n-button>
        </div>
      </div>
      <div class="screen-dialog-footer"><n-popconfirm v-if="platformEdit && draft" @positive-click="discard" positive-text="放弃修改" negative-text="保留"><template #trigger><n-button :disabled="saving" quaternary type="warning" data-testid="screen-discard-draft">放弃本机修改</n-button></template>放弃尚未提交的资料修改，恢复显示平台已保存的值？</n-popconfirm><span class="screen-spacer"></span><n-button :disabled="saving" @click="show = false">取消</n-button><n-button type="primary" attr-type="submit" :disabled="saving" :loading="saving" data-action-owner="local-screen-form">{{ platformEdit ? '保存待提交修改' : screen ? '保存修改' : '加入本机管理' }}</n-button></div>
    </n-form>
  </n-modal>
</template>

<style scoped>
.screen-form-grid { align-items: start; }
.screen-mac-field { width: 100%; }
.screen-mac-note { margin-top: 4px; }
.screen-mac-note .n-button { margin-left: 8px; }
</style>
