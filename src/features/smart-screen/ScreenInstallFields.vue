<script setup lang="ts">
import { computed, onBeforeUnmount, ref } from "vue";
import { NButton, NCheckbox, NInput, NSelect } from "naive-ui";
import { validateScreenApk } from "@/shared/model/screen";
import type { ScreenApkSelection } from "@/shared/model/screen";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import OperationFormTheme from "@/shared/components/OperationFormTheme.vue";

const props = defineProps<{ disabled?: boolean; sizeWarning?: string; real?: boolean }>();
const file = defineModel<ScreenApkSelection | File | null>("file", { default: null });
const reinstall = defineModel<boolean>("reinstall", { default: false });
const fileInput = ref<HTMLInputElement>();
const error = ref("");
const parsing = ref(false);
const packageDisabled = computed(() => props.disabled || parsing.value || Boolean(props.sizeWarning));
const metadata = computed(() => file.value && "path" in file.value ? file.value : undefined);
let active = true;
onBeforeUnmount(() => { active = false; });
const fileSize = computed(() => !file.value ? "" : file.value.size >= 1024 * 1024
  ? `${(file.value.size / (1024 * 1024)).toFixed(1)} MB`
  : `${Math.max(1, Math.ceil(file.value.size / 1024))} KB`);

function chooseFile(event: Event) {
  const input = event.target as HTMLInputElement;
  const selected = input.files?.[0];
  input.value = "";
  // 文件窗口取消时不丢失已经选择的文件；不读取 APK 内容或推断包内元数据。
  if (!selected || packageDisabled.value || !active) return;
  error.value = validateScreenApk({ name: selected.name, size: selected.size, lastModified: selected.lastModified }).join("；");
  file.value = error.value ? null : selected;
}
async function pick() {
  if (packageDisabled.value) return;
  if (!props.real) { fileInput.value?.click(); return; }
  parsing.value = true; error.value = "";
  try {
    const selected = await useScreenAdapter().selectApk?.();
    if (active && selected && !props.disabled && !props.sizeWarning) file.value = selected;
  } catch (cause) { if (active) error.value = cause instanceof Error ? cause.message : "安装包读取失败"; }
  finally { if (active) parsing.value = false; }
}
</script>

<template>
  <operation-form-theme>
  <div class="screen-install-fields">
    <label class="operation-setting-label">选择应用<n-select value="xiaoxin" :options="[{ label: '智能小新', value: 'xiaoxin' }]" size="small" :disabled="disabled" aria-label="选择安装应用" /></label>
    <div class="operation-file-card">
      <div class="screen-apk-heading"><span class="operation-file-label">本地 APK 文件</span><span v-if="sizeWarning" class="screen-apk-size-warning" role="status" data-testid="screen-apk-size-warning">{{ sizeWarning }}</span></div>
      <input ref="fileInput" type="file" accept=".apk" hidden :disabled="packageDisabled" aria-label="选择本地 APK 文件" data-testid="screen-apk-input" @change="chooseFile" />
      <div class="operation-file-picker" :data-testid="file ? 'screen-apk-selected' : undefined">
        <n-input :value="file?.name ?? ''" size="tiny" readonly :disabled="packageDisabled" placeholder="请选择本地 APK 文件" :title="file?.name" aria-label="已选 APK 文件" />
        <n-button size="tiny" secondary :disabled="packageDisabled" :loading="parsing" data-testid="screen-apk-picker" @click="pick">{{ parsing ? '正在读取' : '选择' }}</n-button>
      </div>
      <small v-if="file" class="operation-file-meta">文件大小：{{ fileSize }}</small>
      <small v-if="metadata" class="operation-file-meta">小新{{ metadata.appVersion }}-{{ metadata.appVersionCode }} · {{ metadata.abis?.join(' / ') || '不限制处理器架构' }}<br>安装包信息已读取</small>
      <span v-if="error" class="screen-danger screen-apk-error" role="alert">{{ error }}</span>
    </div>
    <n-checkbox v-model:checked="reinstall" :disabled="disabled" data-testid="screen-install-reinstall">相同版本覆盖安装，保留数据</n-checkbox>
  </div>
  </operation-form-theme>
</template>

<style scoped>
.screen-install-fields { display: grid; gap: 7px; }
.screen-install-fields .operation-setting-label { display: grid; grid-template-columns: 62px minmax(0, 1fr); align-items: center; gap: 6px; }
.screen-apk-error { font-size: var(--inx-font-size-table); line-height: 1.5; }
.screen-apk-heading { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 8px; }
.screen-apk-size-warning { color: var(--inx-color-warning); font-size: var(--inx-font-size-table); line-height: 1.5; }
</style>
