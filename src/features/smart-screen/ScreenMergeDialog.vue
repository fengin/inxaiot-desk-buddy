<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { NAlert, NButton, NCheckbox, NDrawer, NDrawerContent, NPopconfirm, NRadio } from "naive-ui";
import { GitCompareArrows } from "lucide-vue-next";
import { useSmartScreensStore } from "@/stores/smartScreens";
import { useScreenAdapter } from "@/shared/api/screenAdapter";
import { buildScreenMergeFields, createScreenMergeChoices, screenMergeFields, screenMergeSourceValue, screenTaskActive, validateScreenMergeFields } from "@/shared/model/screen";
import type { ScreenMergeCandidate, ScreenMergeField, ScreenMergeFields, ScreenMergeSource } from "@/shared/model/screen";
import { screenSpaceLabel } from "@/shared/model/screenSpace";
import CompactOperationTable from "@/shared/components/CompactOperationTable.vue";

const show = defineModel<boolean>("show", { default: false });
const store = useSmartScreensStore();
const preview = ref<ScreenMergeCandidate>();
const choices = ref(createScreenMergeChoices());
const identityConfirmed = ref(false);
const saving = ref(false);
const candidateColumns=[{key:'local',title:'本机屏',width:'38%'},{key:'platform',title:'平台屏',width:'36%'},{key:'reason',title:'提示'}];
const fieldColumns=[{key:'field',title:'字段',width:'82px'},{key:'local',title:'本机记录'},{key:'platform',title:'平台记录'},{key:'result',title:'合并结果'}];
const error = ref("");
const result = ref<{ ignored: boolean; name: string; fields?: ScreenMergeFields; resultPending?: boolean }>();
const previewStamp = ref("");
let context = 0;
onBeforeUnmount(() => { ++context; });

function stamp(candidate: ScreenMergeCandidate | undefined): string {
  return JSON.stringify([candidate, store.snapshot.spaces, store.snapshot.spacesAvailable]);
}

const current = computed(() => store.candidates.find((candidate) => candidate.key === preview.value?.key));
const stale = computed(() => Boolean(preview.value && !result.value && stamp(current.value) !== previewStamp.value));
const busy = computed(() => Boolean(preview.value && store.snapshot.tasks.some((task) => screenTaskActive(task)
  && task.targets.some((target) => [preview.value!.local.id, preview.value!.platform.id, ...preview.value!.platform.aliases].includes(target.screenId)))));
const merged = computed(() => preview.value ? buildScreenMergeFields(preview.value, choices.value) : undefined);
const validation = computed(() => merged.value ? validateScreenMergeFields(merged.value, store.snapshot.spaces, store.snapshot.spacesAvailable !== false) : []);
const localCount = computed(() => Object.values(choices.value).filter((source) => source === "local").length);
const locked = computed(() => saving.value || Boolean(result.value) || stale.value || !store.platformAvailable);
const canResolve = computed(() => Boolean(preview.value) && !locked.value && !busy.value);
const canMerge = computed(() => canResolve.value && !validation.value.length && (!preview.value?.conflict || identityConfirmed.value));

function review(candidate: ScreenMergeCandidate | undefined) {
  if (saving.value) return;
  preview.value = candidate ? JSON.parse(JSON.stringify(candidate)) as ScreenMergeCandidate : undefined;
  previewStamp.value = stamp(preview.value);
  choices.value = createScreenMergeChoices();
  identityConfirmed.value = false;
  error.value = "";
  result.value = undefined;
}

watch([show, () => store.projectId], ([open]) => {
  ++context;
  saving.value = false;
  review(open ? store.candidates[0] : undefined);
}, { immediate: true });

watch(() => store.candidates, (candidates) => {
  if (show.value && !preview.value && !result.value) review(candidates[0]);
});

function selectSource(field: ScreenMergeField, source: ScreenMergeSource) {
  if (locked.value) return;
  choices.value[field] = source;
  error.value = "";
}

function selectAll(source: ScreenMergeSource) {
  if (locked.value) return;
  choices.value = createScreenMergeChoices(source);
  error.value = "";
}

function displayValue(field: ScreenMergeField, value: string | null | undefined): string {
  if (field === "size") return value === "unknown" ? "待确认" : `${value} 寸`;
  if (!value) return field === "space" ? "待定空间" : "未填写";
  return value;
}

function resultValue(field: ScreenMergeField): string {
  if (field === "space") return screenSpaceLabel(result.value?.fields ?? merged.value ?? {}, store.snapshot.spaces, store.snapshot.spacesAvailable !== false);
  return displayValue(field, (result.value?.fields ?? merged.value)?.[field]);
}

function sourceValue(field: ScreenMergeField, source: ScreenMergeSource): string {
  if (!preview.value) return "";
  return field === "space" ? screenSpaceLabel(preview.value[source], store.snapshot.spaces, store.snapshot.spacesAvailable !== false)
    : displayValue(field, screenMergeSourceValue(preview.value, source, field));
}

async function resolve(ignore = false) {
  if (!(ignore ? canResolve.value : canMerge.value) || !preview.value) return;
  const expectedContext = context;
  const projectId = store.projectId;
  const candidate = JSON.parse(JSON.stringify(preview.value)) as ScreenMergeCandidate;
  const decision = ignore ? { kind: "ignore" as const } : {
    kind: "merge" as const, choices: { ...choices.value }, identityConfirmed: identityConfirmed.value
  };
  saving.value = true;
  error.value = "";
  try {
    const resolved = await useScreenAdapter().merge(projectId, candidate, decision);
    if (context !== expectedContext || store.projectId !== projectId || !show.value) return;
    result.value = { ignored: ignore, name: resolved?.fields.name ?? candidate.platform.name, fields: resolved?.fields, resultPending: resolved?.resultPending };
    await store.refresh();
  } catch (cause) {
    if (context !== expectedContext || store.projectId !== projectId || !show.value) return;
    error.value = cause instanceof Error ? cause.message : "合并失败，请重新核对后重试";
  } finally {
    if (context === expectedContext && store.projectId === projectId) saving.value = false;
  }
}
</script>

<template>
  <n-drawer v-model:show="show" width="min(1060px, 96vw)" class="screen-dialog" :mask-closable="!saving" :close-on-esc="!saving">
    <n-drawer-content title="核对疑似重复屏" :closable="!saving" :body-content-style="{display:'flex',flexDirection:'column',height:'100%',minHeight:'0',overflow:'hidden',gap:'8px'}">
      <div class="screen-callout">相同 IP 或 MAC 仅表示疑似重复。请逐项选择保留本机或平台值，确认合并后两端记录采用相同结果，并保留本机操作历史。</div>
      <n-alert v-if="!store.platformAvailable" type="warning" class="merge-alert" data-testid="merge-platform-unavailable">平台不可用，暂不能核对或合并。恢复连接后请重新核对。</n-alert>
      <n-alert v-if="result" :type="result.resultPending ? 'warning' : 'success'" class="merge-alert" data-testid="merge-result">
        <template v-if="result.ignored">已保留为不同屏。本次候选不再提示，关键身份变化后会重新匹配。</template>
        <template v-else>“{{ result.name }}”合并成功。本机与平台记录已按所选字段保持一致，原本机操作历史继续保留。</template>
        <p v-if="result.resultPending">资料合并已完成，操作记录仍待补存。请在任务面板处理原任务，无需再次合并。</p>
      </n-alert>
      <n-alert v-if="error" type="error" class="merge-alert" data-testid="merge-error">{{ error }}</n-alert>
      <div v-if="!preview && store.platformAvailable" class="screen-empty"><GitCompareArrows :size="30" /><strong>当前没有待核对的候选</strong><span>刷新平台或新增智能屏后，会自动匹配相同 IP / MAC。</span></div>
      <div v-if="preview" class="screen-merge-layout merge-layout">
        <compact-operation-table class="merge-candidates" :columns="candidateColumns" label="疑似重复候选">
          <tr v-for="(candidate,index) in store.candidates" :key="candidate.key" :class="{'merge-candidate-active':preview.key===candidate.key}"><td><button type="button" :disabled="saving" :data-candidate-key="candidate.key" :title="`候选 ${index+1}：${candidate.local.name} / ${candidate.platform.name}`" @click="review(candidate)">{{candidate.local.name}}</button></td><td :title="candidate.platform.name">{{candidate.platform.name}}</td><td :title="candidate.conflict?'身份冲突':candidate.matchMac?'MAC 相同':'IP 相同'">{{candidate.conflict?'身份冲突':candidate.matchMac?'MAC 相同':'IP 相同'}}</td></tr>
          <tr v-if="!store.candidates.length && result"><td colspan="3" class="merge-candidates-empty">当前候选已全部处理</td></tr>
        </compact-operation-table>
        <section class="screen-comparison merge-comparison">
          <compact-operation-table class="merge-fields-table" :columns="fieldColumns" label="合并字段对比" :reset-key="preview.key">
              <template #header-local><span class="merge-header-label merge-heading-local">本机记录<n-checkbox size="small" :checked="localCount === screenMergeFields.length" :indeterminate="localCount > 0 && localCount < screenMergeFields.length" :disabled="locked" aria-label="全部采用本机记录" title="将全部字段设为本机记录" data-testid="merge-all-local" @update:checked="selectAll('local')" /></span></template>
              <template #header-platform><span class="merge-header-label merge-heading-platform">平台记录<n-checkbox size="small" :checked="localCount === 0" :indeterminate="localCount > 0 && localCount < screenMergeFields.length" :disabled="locked" aria-label="全部采用平台记录" title="将全部字段设为平台记录" data-testid="merge-all-platform" @update:checked="selectAll('platform')" /></span></template>
                <tr v-for="field in screenMergeFields" :key="field.key" :data-merge-field="field.key">
                  <th scope="row">{{ field.label }}</th>
                  <td v-for="source in (['local', 'platform'] as const)" :key="source" :class="{ 'merge-source-selected': choices[field.key] === source }">
                    <n-radio :name="`merge-${field.key}`" :value="source" :checked="choices[field.key] === source" :disabled="locked" size="small" @update:checked="selectSource(field.key, source)">
                      <span class="merge-source-value">{{ sourceValue(field.key, source) }}</span>
                    </n-radio>
                  </td>
                  <td class="merge-value-result" :class="`merge-result-${choices[field.key]}`"><span :title="`${choices[field.key] === 'local' ? '本机记录' : '平台记录'}：${resultValue(field.key)}`">{{ resultValue(field.key) }}</span></td>
                </tr>
          </compact-operation-table>
          <div class="merge-selection-summary" data-testid="merge-selection-summary">本机来源 {{ localCount }} 项 <span>｜</span> 平台来源 {{ screenMergeFields.length - localCount }} 项</div>
          <template v-if="!result">
            <n-alert v-if="stale && store.platformAvailable" type="warning" class="merge-alert" data-testid="merge-stale">候选记录或空间目录已变化，请重新核对后再合并。<div class="merge-review-action"><n-button size="small" :disabled="saving" data-testid="merge-review-again" @click="review(current ?? store.candidates[0])">重新核对</n-button></div></n-alert>
            <n-alert v-if="preview.conflict" type="warning" class="merge-alert">两条记录的 MAC 不一致。请核实实际设备身份，并选择要保留的 MAC；只有确认是同一台设备后才能合并。<div class="merge-identity-confirm"><n-checkbox v-model:checked="identityConfirmed" :disabled="locked" data-testid="merge-confirm-identity">已核实是同一台设备</n-checkbox></div></n-alert>
            <n-alert v-if="busy" type="warning" class="merge-alert" data-testid="merge-busy">相关屏仍有活动任务或待核实结果，处理完成后才能合并。</n-alert>
            <n-alert v-if="validation.length && !stale" type="warning" class="merge-alert" data-testid="merge-validation"><div v-for="item in validation" :key="item">{{ item }}</div></n-alert>
            <p class="merge-note">所在空间连同节点和完整路径整体采用一侧，不能拼接两侧层级。合并只统一记录，不会修改屏端网络或重新安装应用。<template v-if="store.snapshot.mode !== 'real'">当前原型仅修改模拟数据。</template></p>
          </template>
        </section>
      </div>
      <template #footer>
        <div class="screen-dialog-footer">
          <n-popconfirm v-if="preview && !result" positive-text="确认不同屏" negative-text="取消" :disabled="!canResolve" @positive-click="resolve(true)"><template #trigger><n-button quaternary :disabled="!canResolve" data-action-owner="different-screen-confirm">不是同一台</n-button></template>保留两条独立记录，不再提示本次相同候选？</n-popconfirm>
          <span class="screen-spacer"></span>
          <n-button :disabled="saving" @click="show = false">{{ result ? '关闭' : '暂不合并' }}</n-button>
          <n-button v-if="result && store.candidates.length" type="primary" :disabled="saving || !store.platformAvailable" @click="review(store.candidates[0])">继续核对</n-button>
          <n-button v-else-if="!result" type="primary" :disabled="!canMerge" :loading="saving" data-testid="merge-submit" @click="resolve()">确认合并</n-button>
        </div>
      </template>
    </n-drawer-content>
  </n-drawer>
</template>

<style scoped>
.merge-layout { flex:1;min-height:0;overflow:hidden;grid-template-columns: 260px minmax(0, 1fr); gap: 10px; margin-top:0; }
.merge-candidates button {display:block;max-width:100%;padding:0;border:0;background:none;color:var(--inx-color-info);font:inherit;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;cursor:pointer}
.merge-candidate-active{background:var(--inx-color-info-soft)}
.merge-candidates button:disabled { cursor: wait; }
.merge-candidates-empty { color: var(--inx-color-text-secondary); font-size: 12px; }
.merge-comparison {display:flex;flex-direction:column;gap:6px;min-height:0;overflow:hidden;min-width: 0; --merge-local-color: color-mix(in srgb, var(--inx-color-info) 80%, var(--inx-color-text)); --merge-platform-color: color-mix(in srgb, var(--inx-color-operation) 75%, var(--inx-color-text)); }
.merge-fields-table :deep(td),.merge-fields-table :deep(th[scope="row"]){padding:5px 7px;white-space:normal;overflow-wrap:anywhere;vertical-align:middle;font-size:var(--inx-font-size-base);font-weight:400;line-height:20px}
.merge-fields-table .merge-heading-local, .merge-result-local { color: var(--merge-local-color); }
.merge-fields-table .merge-heading-platform, .merge-result-platform { color: var(--merge-platform-color); }
.merge-header-label { display: inline-flex; align-items: center; gap: 8px; white-space: nowrap; }
.merge-fields-table th[scope="row"] { white-space: nowrap; }
.merge-fields-table :deep(.n-radio) { width: 100%; align-items: flex-start; }
.merge-fields-table :deep(.n-radio__label) { min-width: 0; overflow-wrap: anywhere; }
.merge-source-selected { background: var(--inx-color-surface-subtle); }
.merge-source-value, .merge-value-result > span { white-space: pre-wrap; overflow-wrap: anywhere; }
.merge-selection-summary { display: flex;flex:none; gap: 10px; padding-top:0; color: var(--inx-color-text-secondary); font-size:11px; }
.merge-selection-summary > span { color: var(--inx-color-border); }
.merge-alert { flex:none;margin:0;font-size:12px }
.merge-identity-confirm, .merge-review-action { margin-top:4px; }
.merge-note {flex:none;color:var(--inx-color-text-secondary);font-size:11px;line-height:17px;margin:0}
.screen-callout{flex:none;margin:0;padding:6px 10px;font-size:11px;line-height:18px}
@media (max-width: 850px) {
  .merge-layout { grid-template-columns: 220px minmax(0,1fr); }
}
</style>
