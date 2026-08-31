import { defineStore } from "pinia";
import { ref } from "vue";

import { commandErrorCode, commandErrorText } from "@/shared/api/errors";
import { useWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import type { HostKeyObservation } from "@/shared/model/project";
import {
  emptyReleaseProfileDraft,
  type ReleaseProfileDraft,
  type ReleaseProfileValidation,
  type ReleaseProfileView
} from "@/shared/model/releaseProfile";

export const useReleaseProfileStore = defineStore("release-profile", () => {
  const projectId = ref("");
  const profile = ref<ReleaseProfileView>();
  const draft = ref<ReleaseProfileDraft>(emptyReleaseProfileDraft());
  const validation = ref<ReleaseProfileValidation>();
  const hostKeys = ref<HostKeyObservation[]>([]);
  const hostKeyObservation = ref<HostKeyObservation>();
  const loading = ref(false);
  const saving = ref(false);
  const hostKeyLoading = ref(false);
  const error = ref("");
  const conflict = ref(false);

  function resetDraft() {
    draft.value = profile.value
      ? {
          values: { ...profile.value.values },
          credentials: { ...profile.value.credentials },
          expectedVersion: profile.value.version
        }
      : emptyReleaseProfileDraft();
  }

  async function load(nextProjectId: string) {
    projectId.value = nextProjectId;
    loading.value = true;
    error.value = "";
    conflict.value = false;
    try {
      profile.value = await useWorkbenchAdapter().getReleaseProfile(nextProjectId) ?? undefined;
      resetDraft();
      hostKeys.value = await useWorkbenchAdapter().listHostKeys(nextProjectId);
      return profile.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "读取发布参数失败");
      throw cause;
    } finally {
      loading.value = false;
    }
  }

  async function validate() {
    error.value = "";
    try {
      validation.value = await useWorkbenchAdapter().validateReleaseProfile(draft.value);
      return validation.value;
    } catch (cause) {
      validation.value = undefined;
      error.value = commandErrorText(cause, "发布参数校验失败");
      throw cause;
    }
  }

  async function save() {
    if (!projectId.value) throw new Error("没有活动项目");
    saving.value = true;
    error.value = "";
    conflict.value = false;
    try {
      await validate();
      profile.value = await useWorkbenchAdapter().saveReleaseProfile(projectId.value, draft.value);
      resetDraft();
      return profile.value;
    } catch (cause) {
      conflict.value = commandErrorCode(cause) === "CONFIG_VERSION_CONFLICT";
      error.value = commandErrorText(cause, "保存发布参数失败");
      throw cause;
    } finally {
      saving.value = false;
    }
  }

  async function captureHostKey(host: string, port?: number) {
    if (!projectId.value) throw new Error("没有活动项目");
    hostKeyLoading.value = true;
    error.value = "";
    try {
      hostKeyObservation.value = await useWorkbenchAdapter().captureHostKey(projectId.value, { host, port });
      return hostKeyObservation.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "捕获主机密钥失败");
      throw cause;
    } finally {
      hostKeyLoading.value = false;
    }
  }

  async function confirmHostKey(replaceChanged: boolean) {
    if (!projectId.value || !hostKeyObservation.value) throw new Error("没有待确认主机密钥");
    hostKeyLoading.value = true;
    try {
      hostKeyObservation.value = await useWorkbenchAdapter().confirmHostKey(projectId.value, {
        host: hostKeyObservation.value.host,
        port: hostKeyObservation.value.port,
        algorithm: hostKeyObservation.value.algorithm,
        fingerprint: hostKeyObservation.value.fingerprint,
        replaceChanged
      });
      hostKeys.value = await useWorkbenchAdapter().listHostKeys(projectId.value);
      return hostKeyObservation.value;
    } catch (cause) {
      error.value = commandErrorText(cause, "确认主机密钥失败");
      throw cause;
    } finally {
      hostKeyLoading.value = false;
    }
  }

  return {
    projectId,
    profile,
    draft,
    validation,
    hostKeys,
    hostKeyObservation,
    loading,
    saving,
    hostKeyLoading,
    error,
    conflict,
    load,
    resetDraft,
    validate,
    save,
    captureHostKey,
    confirmHostKey
  };
});
