<script setup lang="ts">
import { computed } from "vue";
import { NTreeSelect } from "naive-ui";
import type { TreeOption, TreeSelectOption } from "naive-ui";
import { buildProjectSpaceTree, getProjectSpacePath, projectSpacePath } from "@/shared/model/projectSpace";
import type { ProjectSpaceNode, ProjectSpaceOption } from "@/shared/model/projectSpace";

const props = withDefaults(defineProps<{
  spaces: readonly ProjectSpaceNode[];
  modelValue?: string | null;
  disabled?: boolean;
  clearable?: boolean;
  placeholder?: string;
  fallbackLabel?: string;
}>(), { modelValue: null, disabled: false, clearable: true, placeholder: "请选择空间", fallbackLabel: "原空间关联待核验" });
const emit = defineEmits<{ "update:modelValue": [value: string | null] }>();
const selectedPath = computed(() => projectSpacePath(props.spaces, props.modelValue ?? "") || (props.modelValue ? props.fallbackLabel : ""));
function countPaths(nodes: readonly ProjectSpaceOption[], counts: Map<string, number>) {
  for (const node of nodes) {
    counts.set(node.path, (counts.get(node.path) ?? 0) + 1);
    if (node.children) countPaths(node.children, counts);
  }
}
function toOptions(nodes: readonly ProjectSpaceOption[], counts: ReadonlyMap<string, number>): TreeSelectOption[] {
  return nodes.map((node) => ({ value: node.value, label: (counts.get(node.path) ?? 0) > 1 ? `${node.label}（编号 ${node.value}）` : node.label, path: node.path,
    ...(node.children ? { children: toOptions(node.children, counts) } : {}) }));
}
const options = computed(() => {
  const source = buildProjectSpaceTree(props.spaces), counts = new Map<string, number>();
  countPaths(source, counts);
  const tree = toOptions(source, counts);
  if (props.modelValue && !getProjectSpacePath(props.spaces, props.modelValue)) {
    // 失效或暂时取不到目录时仅回显原关联，不将缓存路径当成可分配目录。
    tree.push({ value: props.modelValue, label: props.fallbackLabel, path: props.fallbackLabel, disabled: true });
  }
  return tree;
});
function filter(pattern: string, node: TreeOption): boolean {
  const normalize = (value: string) => value.toLocaleLowerCase().replace(/\s+/g, "");
  return normalize(String(node.path ?? node.label ?? "")).includes(normalize(pattern));
}
function update(value: string | number | (string | number)[] | null) {
  if (props.disabled) return;
  if (value === null) { emit("update:modelValue", null); return; }
  if (typeof value === "string" && getProjectSpacePath(props.spaces, value)) emit("update:modelValue", value);
}
</script>

<template>
  <n-tree-select
    :value="modelValue || null"
    :options="options"
    key-field="value"
    label-field="label"
    :disabled="disabled"
    :clearable="clearable"
    :placeholder="placeholder"
    :title="selectedPath"
    :filter="filter"
    aria-label="空间选择"
    filterable
    show-path
    separator="/"
    check-strategy="all"
    @update:value="update"
  />
</template>
