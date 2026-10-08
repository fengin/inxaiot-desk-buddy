<script setup lang="ts">
import { NConfigProvider, type GlobalThemeOverrides } from "naive-ui";

// 字号与控件高度分开：紧凑文件控件仍用 tiny 高度，文字与其他表单字段一致。
// 局部继承上层主题，保留表格小按钮和其他业务区域原有的小字号。
const themeOverrides: GlobalThemeOverrides = {
  common: {
    fontSizeTiny: "var(--inx-font-size-base)",
    fontSizeMini: "var(--inx-font-size-base)"
  }
};
</script>

<template>
  <n-config-provider abstract :theme-overrides="themeOverrides"><slot /></n-config-provider>
</template>
