<script setup lang="ts">
import { computed } from "vue";
import { NTreeSelect } from "naive-ui";
import type { TreeOption } from "naive-ui";
import { buildScreenSpaceOptions, SCREEN_UNLOCATED_SPACE_KEY } from "@/shared/model/screenSpace";
import type { ScreenSpaceNode } from "@/shared/model/screenSpace";
import { projectSpacePath } from "@/shared/model/projectSpace";

const props = defineProps<{ spaces: ScreenSpaceNode[]; modelValue: string }>();
const emit = defineEmits<{ "update:modelValue": [value: string] }>();
const options = computed(() => [
  { label: "待定空间", value: SCREEN_UNLOCATED_SPACE_KEY },
  ...buildScreenSpaceOptions(props.spaces)
]);
function filter(pattern: string, node: TreeOption) {
  const path = projectSpacePath(props.spaces, String(node.value ?? "")) || String(node.label ?? "");
  return path.toLocaleLowerCase().replace(/\s+/g, "").includes(pattern.toLocaleLowerCase().replace(/\s+/g, ""));
}
function update(value: string | number | (string | number)[] | null) {
  emit("update:modelValue", typeof value === "string" ? value : "");
}
</script>

<template>
  <n-tree-select
    :value="modelValue || null"
    :options="options"
    key-field="value"
    :filter="filter"
    placeholder="全部空间"
    aria-label="所在空间筛选"
    clearable
    filterable
    show-path
    separator="/"
    check-strategy="all"
    @update:value="update"
  />
</template>
