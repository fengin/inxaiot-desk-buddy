<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { NButton, NCheckbox, NInput, NInputNumber, NModal, NSelect } from "naive-ui";
import { appConfigDisplay, type AppConfigScalar, type AppConfigVoiceEdit } from "@/shared/model/screenAppConfig";
import { appConfigFieldIssue, type AppConfigSchemaFieldState } from "@/shared/model/screenAppConfigSchema";

const props = defineProps<{
  fields: AppConfigSchemaFieldState[];
  edits: Record<string, AppConfigVoiceEdit>;
  dirtyKeys: string[];
  ready: boolean;
  busy: boolean;
  selectedCount: number;
  schemaErrors: string[];
  screenName: (id: string) => string;
}>();
const emit = defineEmits<{ edit: [field: string, value: AppConfigScalar | null] }>();
const show = ref(false);
const sections = computed(() => {
  const groups = new Map<string, AppConfigSchemaFieldState[]>();
  for (const field of props.fields) {
    const name = field.definition.section || "语音参数";
    groups.set(name, [...(groups.get(name) ?? []), field]);
  }
  return [...groups].map(([name, fields]) => ({ name, fields }));
});
const editedKeys = computed(() => new Set(props.dirtyKeys));
const legacyOnly = computed(() => !props.fields.some(field => field.definition.key.startsWith("audioConfig.")));
watch(() => props.busy, value => { if (value) show.value = false; });

function value(field: AppConfigSchemaFieldState): AppConfigScalar | null {
  const edit = props.edits[field.definition.key];
  return edit?.mode === "set" ? edit.value : field.summary.mixed ? null : field.summary.value ?? null;
}
function disabled(field: AppConfigSchemaFieldState): boolean { return props.busy || !props.ready || !field.editable; }
function mixed(field: AppConfigSchemaFieldState): boolean {
  return disabled(field) || (props.edits[field.definition.key]?.mode !== "set" && field.summary.mixed);
}
function hint(field: AppConfigSchemaFieldState): string {
  if (!props.ready) return "重新读取配置后确认实际值；未修改项保留原设置。";
  if (field.incompatible) return "所选屏的参数规则不同，请分批修改。";
  if (field.unavailableIds.length) return `${field.unavailableIds.map(props.screenName).join("、")}不支持此项，或尚未读取到有效参数值；取消选择这些屏后可继续修改。`;
  if (field.definition.type === "boolean" && field.summary.mixed) return "半选保留各屏原设置；点击依次统一开启、统一关闭、恢复保留。";
  return field.summary.mixed ? "各屏当前值不同；输入后统一修改，清空输入可恢复各自原值。" : "改回当前值即可取消本项修改；清空输入可恢复原值。";
}
function issue(field: AppConfigSchemaFieldState): string | undefined {
  const edit = props.edits[field.definition.key];
  return edit?.mode === "set" && field.editable ? appConfigFieldIssue(field.definition, edit.value) : undefined;
}
function status(field: AppConfigSchemaFieldState): "error" | "warning" | undefined {
  return issue(field) ? "error" : editedKeys.value.has(field.definition.key) ? "warning" : undefined;
}
function defaultText(field: AppConfigSchemaFieldState): string {
  if (field.defaultsVary) return "默认值因设备而异";
  return field.definition.defaultValue === undefined ? "" : `默认：${appConfigDisplay(field.definition.defaultValue, false, field.definition)}`;
}
function rangeText(field: AppConfigSchemaFieldState): string {
  return field.definition.min !== undefined || field.definition.max !== undefined
    ? `范围：${field.definition.min ?? "不限"}～${field.definition.max ?? "不限"}${field.definition.unit ? ` ${field.definition.unit}` : ""}` : "";
}
function currentOutOfRange(field: AppConfigSchemaFieldState): boolean {
  return field.editable && props.edits[field.definition.key]?.mode !== "set" && !field.summary.mixed && Boolean(appConfigFieldIssue(field.definition, field.summary.value));
}
</script>

<template>
  <n-button text size="small" :type="dirtyKeys.length ? 'warning' : 'primary'" :disabled="busy || !selectedCount" aria-label="语音配置" :title="dirtyKeys.length ? `${dirtyKeys.length} 项语音配置待修改` : '查看和修改语音相关配置'" @click="show = true">语音配置 &gt;</n-button>
  <n-modal v-model:show="show" preset="card" title="语音配置" class="screen-voice-config-modal" :style="{ width: 'min(860px, calc(100vw - 32px))' }" :header-style="{ padding: '16px 20px 12px' }" :content-style="{ padding: '0 20px' }" :footer-style="{ padding: '12px 20px 16px' }" :bordered="false" :auto-focus="false">
    <div class="screen-voice-config-body">
      <p class="screen-voice-config-note">修改内容在主页面检查后统一保存，其他设置保留原值。保存后按需重启小新生效。</p>
      <p v-if="!ready" class="screen-warning">请先重新读取配置，确认当前值后再修改。</p>
      <p v-for="message in schemaErrors" :key="message" class="screen-warning">{{ message }}</p>
      <p v-if="ready && legacyOnly" class="screen-voice-config-note">当前所选小新仅提供语音启动设置。其他语音参数需升级小新后重新读取。</p>
      <section v-for="section in sections" :key="section.name" class="screen-voice-config-section">
        <h4 v-if="!section.fields.every(field => field.definition.type === 'boolean')">{{ section.name }}</h4>
        <div class="screen-voice-config-grid">
          <div v-for="(field, index) in section.fields" :key="field.definition.key" class="screen-voice-config-field" :class="{ 'is-edited': editedKeys.has(field.definition.key), 'is-boolean': field.definition.type === 'boolean', 'is-wide': section.fields.length % 2 === 1 && index === section.fields.length - 1 }" :data-voice-field="field.definition.key" :title="hint(field)">
            <template v-if="field.definition.type === 'boolean'">
              <n-checkbox :checked="value(field) === true" :indeterminate="mixed(field)" :disabled="disabled(field)" :aria-label="field.definition.label" size="small" @update:checked="emit('edit', field.definition.key, $event)">{{ field.definition.label }}</n-checkbox>
            </template>
            <template v-else>
              <label>{{ field.definition.label }}</label>
              <n-select v-if="field.definition.options?.length" :value="value(field) as string | number | null" :options="field.definition.options as { label: string; value: string | number }[]" :disabled="disabled(field)" :status="status(field)" size="small" clearable :placeholder="field.summary.mixed ? '多个值，选择后统一修改' : '尚未确认'" :aria-label="field.definition.label" @update:value="emit('edit', field.definition.key, $event)" />
              <n-input-number v-else-if="field.definition.type === 'number' || field.definition.type === 'integer'" :value="value(field) as number | null" :step="field.definition.step ?? (field.definition.type === 'integer' ? 1 : 0.1)" :disabled="disabled(field)" :status="status(field)" size="small" clearable :placeholder="field.summary.mixed ? '多个值，输入后统一修改' : '尚未确认'" :aria-label="field.definition.label" @update:value="emit('edit', field.definition.key, $event)">
                <template v-if="field.definition.unit" #suffix>{{ field.definition.unit }}</template>
              </n-input-number>
              <n-input v-else :value="value(field) === null ? '' : String(value(field))" :maxlength="field.definition.maxLength" :disabled="disabled(field)" :status="status(field)" size="small" clearable :placeholder="field.summary.mixed ? '多个值，输入后统一修改' : '尚未确认'" :aria-label="field.definition.label" @update:value="emit('edit', field.definition.key, $event || null)" />
            </template>
            <div class="screen-voice-config-meta">
              <span v-if="!field.editable && ready" class="screen-warning">{{ field.incompatible ? '参数规则不同，请分批修改' : '部分屏不支持或当前值未确认' }}</span>
              <span v-else-if="issue(field)" class="screen-warning">{{ issue(field) }}</span>
              <span v-else-if="currentOutOfRange(field)" class="screen-warning">当前值不在允许范围内，可调整为支持的值；未修改时仍保留。</span>
              <span v-else-if="field.definition.description">{{ field.definition.description }}</span>
              <span v-if="rangeText(field)" class="screen-voice-config-meta-value">{{ rangeText(field) }}</span>
              <span v-if="defaultText(field)" class="screen-voice-config-meta-value">{{ defaultText(field) }}</span>
            </div>
          </div>
        </div>
      </section>
    </div>
    <template #footer><div class="screen-voice-config-footer"><span>{{ dirtyKeys.length ? `${dirtyKeys.length} 项语音配置待修改` : '未修改项保留原设置' }}</span><n-button size="small" @click="show = false">完成</n-button></div></template>
  </n-modal>
</template>

<style scoped>
.screen-voice-config-note{margin:0 0 10px;color:var(--inx-color-text-secondary);font-size:var(--inx-font-size-table);line-height:1.5}
.screen-voice-config-section+section{margin-top:14px}.screen-voice-config-section h4{margin:0 0 8px;padding-left:2px;font-size:var(--inx-font-size-base);font-weight:600}
.screen-voice-config-grid{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:10px 14px}
.screen-voice-config-field{display:grid;grid-template-columns:104px minmax(0,1fr);align-items:center;align-content:start;gap:4px 8px;min-width:0;padding:8px 12px;box-sizing:border-box;border-radius:6px;background:var(--inx-color-surface-subtle)}.screen-voice-config-field label{font-size:var(--inx-font-size-base);line-height:20px}
.screen-voice-config-field.is-edited>label,.screen-voice-config-field.is-edited :deep(.n-checkbox__label){color:var(--inx-color-warning)}
.screen-voice-config-field.is-boolean{grid-column:1/-1;display:flex;flex-wrap:wrap;gap:4px 12px;min-height:44px}
.screen-voice-config-meta{grid-column:1/-1;font-size:var(--inx-font-size-table);color:var(--inx-color-text-secondary);line-height:1.5;min-width:0}
.screen-voice-config-meta>span+span::before{content:' · ';white-space:pre}
.screen-voice-config-meta-value{white-space:nowrap}.screen-voice-config-meta .screen-warning{color:var(--inx-color-warning)}
.screen-voice-config-field.is-boolean .screen-voice-config-meta{flex:1 1 300px}
.screen-voice-config-field.is-wide:not(.is-boolean){grid-column:1/-1;grid-template-columns:104px minmax(0,254px) minmax(0,1fr)}
.screen-voice-config-field.is-wide:not(.is-boolean) .screen-voice-config-meta{grid-column:auto}
.screen-voice-config-footer{display:flex;align-items:center;justify-content:space-between;gap:12px;font-size:var(--inx-font-size-table);color:var(--inx-color-text-secondary)}
@media(max-height:700px){.screen-voice-config-note{margin-bottom:8px}.screen-voice-config-section+section{margin-top:10px}.screen-voice-config-section h4{margin-bottom:6px}.screen-voice-config-grid{row-gap:8px}.screen-voice-config-field{padding:6px 10px}.screen-voice-config-field.is-boolean{min-height:40px}}
@media(max-width:640px){.screen-voice-config-grid{grid-template-columns:minmax(0,1fr)}.screen-voice-config-field.is-wide:not(.is-boolean){grid-template-columns:104px minmax(0,1fr)}.screen-voice-config-field.is-wide:not(.is-boolean) .screen-voice-config-meta{grid-column:1/-1}}
</style>
