<script setup lang="ts">
import { NAlert, NButton, NForm, NFormItem, NInput, NModal, NSelect, NSpace, NSwitch, useMessage } from "naive-ui";
import { computed, reactive, watch } from "vue";

import { usePreferencesStore, type DensityMode, type FontSizeMode, type ThemeMode } from "@/stores/preferences";
import { useDataDirectoryStore } from "@/stores/dataDirectory";
import type { DataDirectorySwitchMode } from "@/shared/model/dataDirectory";
import { useSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";

const props = defineProps<{ show: boolean }>();
const emit = defineEmits<{ "update:show": [value: boolean] }>();
const preferences = usePreferencesStore();
const dataDirectory = useDataDirectoryStore();
const message = useMessage();
const dialogs = useSystemDialogAdapter();
let original = { ...preferences.snapshot };

const draft = reactive({
  theme: preferences.theme as ThemeMode,
  fontSize: preferences.fontSize as FontSizeMode,
  density: preferences.density as DensityMode,
  reduceMotion: preferences.reduceMotion,
  pageSize: preferences.pageSize as 20 | 50 | 100,
  dataDirectory: "",
  dataDirectoryMode: "migrate" as Exclude<DataDirectorySwitchMode, "use_existing">
});

watch(
  () => props.show,
  async (show) => {
    if (!show) return;
    original = { ...preferences.snapshot };
    Object.assign(draft, original);
    try {
      await dataDirectory.initialize(true);
      draft.dataDirectory = dataDirectory.status?.pendingDirectory ?? dataDirectory.activeDirectory;
    } catch {
      draft.dataDirectory = "";
    }
  }
);

watch(
  draft,
  () => {
    if (!props.show) return;
    preferences.theme = draft.theme;
    preferences.fontSize = draft.fontSize;
    preferences.density = draft.density;
    preferences.reduceMotion = draft.reduceMotion;
    preferences.pageSize = draft.pageSize;
  },
  { deep: true }
);

const previewLabel = computed(() => {
  const font = draft.fontSize === "small" ? "小字号" : draft.fontSize === "large" ? "大字号" : "标准字号";
  return `${font} · ${draft.density === "compact" ? "紧凑" : "舒适"}`;
});

function cancel() {
  preferences.apply(original);
  emit("update:show", false);
}

async function selectDataDirectory() {
  const selected = await dialogs.selectDirectory("选择 INX 实施工作台数据目录");
  if (selected) draft.dataDirectory = selected;
  if (!dialogs.real) message.info("浏览器Fixture已填入隔离示例目录");
}

async function save() {
  preferences.apply({
    ...original,
    theme: draft.theme,
    fontSize: draft.fontSize,
    density: draft.density,
    reduceMotion: draft.reduceMotion,
    pageSize: draft.pageSize
  });
  const target = draft.dataDirectory.trim();
  const pending = dataDirectory.status?.pendingDirectory;
  if (target && target !== dataDirectory.activeDirectory && target !== pending) {
    try {
      await dataDirectory.scheduleSwitch({
        targetDirectory: target,
        mode: draft.dataDirectoryMode
      });
      message.warning("数据目录切换已安全登记，退出并重新打开应用后生效");
    } catch {
      message.error(dataDirectory.error || "数据目录切换失败");
      return;
    }
  } else {
    message.success("界面设置已保存");
  }
  emit("update:show", false);
}

async function rollbackDataDirectory() {
  try {
    await dataDirectory.scheduleRollback();
    draft.dataDirectory = dataDirectory.status?.pendingDirectory ?? draft.dataDirectory;
    message.warning("数据目录回滚已登记，退出并重新打开应用后生效");
  } catch {
    message.error(dataDirectory.error || "数据目录回滚失败");
  }
}
</script>

<template>
  <n-modal :show="show" preset="card" title="界面设置" class="preferences-modal" :bordered="false" @update:show="emit('update:show', $event)">
    <p class="modal-description">这些设置只影响当前电脑，不随业务项目切换。</p>
    <div class="preference-preview">
      <span>实时预览</span><strong>{{ previewLabel }}</strong><small>表格、按钮和页面间距会同步变化</small>
    </div>
    <n-form label-placement="left" label-width="104" size="small">
      <n-form-item label="主题">
        <n-select v-model:value="draft.theme" :options="[{ label: '跟随系统', value: 'system' }, { label: '浅色', value: 'light' }, { label: '深色', value: 'dark' }]" />
      </n-form-item>
      <n-form-item label="界面字号">
        <n-select v-model:value="draft.fontSize" :options="[{ label: '小', value: 'small' }, { label: '标准', value: 'standard' }, { label: '大', value: 'large' }]" />
      </n-form-item>
      <n-form-item label="界面密度">
        <n-select v-model:value="draft.density" :options="[{ label: '紧凑', value: 'compact' }, { label: '舒适', value: 'comfortable' }]" />
      </n-form-item>
      <n-form-item label="每页条数">
        <n-select v-model:value="draft.pageSize" :options="[{ label: '20 条', value: 20 }, { label: '50 条', value: 50 }, { label: '100 条', value: 100 }]" />
      </n-form-item>
      <n-form-item label="数据目录">
        <div class="data-directory-field">
          <n-input v-model:value="draft.dataDirectory" data-testid="data-directory-input" placeholder="选择 INX 实施工作台数据目录" />
          <n-button secondary @click="selectDataDirectory">选择</n-button>
        </div>
      </n-form-item>
      <n-form-item label="切换方式">
        <n-select
          v-model:value="draft.dataDirectoryMode"
          data-testid="data-directory-mode"
          :options="[
            { label: '迁移当前数据后切换', value: 'migrate' },
            { label: '使用新的空白目录', value: 'empty' }
          ]"
        />
      </n-form-item>
      <n-alert v-if="dataDirectory.status" type="info" :bordered="false">
        当前实际目录：{{ dataDirectory.status.activeDirectory }}
        <template v-if="dataDirectory.status.pendingDirectory"><br />待重启切换：{{ dataDirectory.status.pendingDirectory }}</template>
      </n-alert>
      <n-alert v-if="dataDirectory.status?.lastSwitchError" type="error" :bordered="false">
        上次切换已自动回滚：{{ dataDirectory.status.lastSwitchError }}
      </n-alert>
      <n-form-item label="减少动画"><n-switch v-model:value="draft.reduceMotion" /></n-form-item>
    </n-form>
    <template #footer>
      <n-space justify="space-between">
        <n-button
          size="small"
          secondary
          data-testid="data-directory-rollback"
          :disabled="!dataDirectory.status?.previousDirectory || dataDirectory.loading"
          @click="rollbackDataDirectory"
        >回滚到上一目录</n-button>
        <n-space><n-button size="small" @click="cancel">取消</n-button><n-button size="small" type="primary" data-testid="preferences-save" :loading="dataDirectory.loading" @click="save">保存设置</n-button></n-space>
      </n-space>
    </template>
  </n-modal>
</template>
