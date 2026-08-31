import { defineStore } from "pinia";
import { computed, ref } from "vue";

import { commandErrorText } from "@/shared/api/errors";
import { useDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import type {
  DataDirectoryStatus,
  DataDirectorySwitchRequest
} from "@/shared/model/dataDirectory";

export const useDataDirectoryStore = defineStore("data-directory", () => {
  const status = ref<DataDirectoryStatus>();
  const loading = ref(false);
  const error = ref("");
  const activeDirectory = computed(() => status.value?.activeDirectory ?? "");

  async function initialize(force = false) {
    if (loading.value || (status.value && !force)) return status.value;
    loading.value = true;
    error.value = "";
    try {
      status.value = await useDataDirectoryAdapter().getStatus();
      return status.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "读取实际数据目录失败");
      throw cause;
    } finally {
      loading.value = false;
    }
  }

  async function scheduleSwitch(request: DataDirectorySwitchRequest) {
    loading.value = true;
    error.value = "";
    try {
      status.value = await useDataDirectoryAdapter().scheduleSwitch(request);
      return status.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "安排数据目录切换失败");
      throw cause;
    } finally {
      loading.value = false;
    }
  }

  async function scheduleRollback() {
    loading.value = true;
    error.value = "";
    try {
      status.value = await useDataDirectoryAdapter().scheduleRollback();
      return status.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "安排数据目录回滚失败");
      throw cause;
    } finally {
      loading.value = false;
    }
  }

  return {
    status,
    loading,
    error,
    activeDirectory,
    initialize,
    scheduleSwitch,
    scheduleRollback
  };
});
