<script setup lang="ts">
import { darkTheme, dateZhCN, NConfigProvider, NDialogProvider, NMessageProvider, zhCN } from "naive-ui";
import { computed, onBeforeUnmount, ref, watchEffect } from "vue";

import DesktopShell from "@/shell/DesktopShell.vue";
import { createThemeOverrides } from "@/app/theme";
import { usePreferencesStore } from "@/stores/preferences";

const preferences = usePreferencesStore();
const media = window.matchMedia("(prefers-color-scheme: dark)");
const systemDark = ref(media.matches);
const onMediaChange = (event: MediaQueryListEvent) => (systemDark.value = event.matches);
media.addEventListener("change", onMediaChange);
onBeforeUnmount(() => media.removeEventListener("change", onMediaChange));

const resolvedDark = computed(
  () => preferences.theme === "dark" || (preferences.theme === "system" && systemDark.value)
);
const themeOverrides = computed(() => createThemeOverrides(preferences.density));

watchEffect(() => {
  document.documentElement.dataset.theme = resolvedDark.value ? "dark" : "light";
  document.documentElement.dataset.fontSize = preferences.fontSize;
  document.documentElement.dataset.density = preferences.density;
  document.documentElement.dataset.reduceMotion = String(preferences.reduceMotion);
});
</script>

<template>
  <n-config-provider :locale="zhCN" :date-locale="dateZhCN" :theme="resolvedDark ? darkTheme : null" :theme-overrides="themeOverrides">
    <n-dialog-provider>
      <n-message-provider placement="top-right">
        <desktop-shell />
      </n-message-provider>
    </n-dialog-provider>
  </n-config-provider>
</template>
