import { defineStore } from "pinia";
import { ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useDiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";
import type { SystemDiagnostics } from "@/shared/model/diagnostics";

export const useDiagnosticsStore = defineStore("diagnostics", () => {
  const value = ref<SystemDiagnostics>();
  const loading = ref(false);
  const error = ref("");

  async function load(force = false) {
    if (loading.value || (value.value && !force)) return value.value;
    loading.value = true;
    error.value = "";
    try {
      value.value = await useDiagnosticsAdapter().getSystemDiagnostics();
      return value.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "读取系统诊断信息失败");
      throw cause;
    } finally {
      loading.value = false;
    }
  }

  return { value, loading, error, load };
});
