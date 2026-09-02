import { defineStore } from "pinia";
import { ref } from "vue";

import { commandErrorCode, commandErrorText } from "@/shared/api/errors";
import { useWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
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
  const loading = ref(false);
  const saving = ref(false);
  const keyOperationLoading = ref(false);
  const error = ref("");
  const conflict = ref(false);
  let loadRequest = 0;
  let mutationRequest = 0;

  function cloneDraft(source: ReleaseProfileDraft): ReleaseProfileDraft {
    return {
      values: { ...source.values },
      credentials: { ...source.credentials },
      expectedVersion: source.expectedVersion
    };
  }

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
    const request = ++loadRequest;
    mutationRequest += 1;
    projectId.value = nextProjectId;
    profile.value = undefined;
    draft.value = emptyReleaseProfileDraft();
    validation.value = undefined;
    loading.value = true;
    error.value = "";
    conflict.value = false;
    try {
      const loadedProfile = await useWorkbenchAdapter().getReleaseProfile(nextProjectId);
      if (request === loadRequest && projectId.value === nextProjectId) {
        profile.value = loadedProfile ?? undefined;
        resetDraft();
      }
      return loadedProfile ?? undefined;
    } catch (cause) {
      if (request === loadRequest) {
        error.value = commandErrorText(cause, "读取发布参数失败");
      }
      throw cause;
    } finally {
      if (request === loadRequest) loading.value = false;
    }
  }

  async function validate() {
    const request = ++mutationRequest;
    const expectedProjectId = projectId.value;
    const candidate = cloneDraft(draft.value);
    error.value = "";
    try {
      const result = await useWorkbenchAdapter().validateReleaseProfile(candidate);
      if (request === mutationRequest && projectId.value === expectedProjectId) {
        validation.value = result;
      }
      return result;
    } catch (cause) {
      if (request === mutationRequest) {
        validation.value = undefined;
        error.value = commandErrorText(cause, "发布参数校验失败");
      }
      throw cause;
    }
  }

  async function save() {
    if (!projectId.value) throw new Error("没有活动项目");
    const expectedProjectId = projectId.value;
    const request = ++mutationRequest;
    const candidate = cloneDraft(draft.value);
    saving.value = true;
    error.value = "";
    conflict.value = false;
    try {
      const checked = await useWorkbenchAdapter().validateReleaseProfile(candidate);
      const saved = await useWorkbenchAdapter().saveReleaseProfile(expectedProjectId, candidate);
      if (request === mutationRequest && projectId.value === expectedProjectId) {
        validation.value = checked;
        profile.value = saved;
        resetDraft();
      }
      return saved;
    } catch (cause) {
      if (request === mutationRequest) {
        conflict.value = commandErrorCode(cause) === "CONFIG_VERSION_CONFLICT";
        error.value = commandErrorText(cause, "保存发布参数失败");
      }
      throw cause;
    } finally {
      if (request === mutationRequest) saving.value = false;
    }
  }

  async function exportMasterKey(filePath: string, passphrase: string) {
    if (!projectId.value) throw new Error("没有活动项目");
    const expectedProjectId = projectId.value;
    keyOperationLoading.value = true;
    error.value = "";
    try {
      return await useWorkbenchAdapter().exportReleaseMasterKey(expectedProjectId, { filePath, passphrase });
    } catch (cause) {
      error.value = commandErrorText(cause, "导出项目主密钥失败");
      throw cause;
    } finally {
      keyOperationLoading.value = false;
    }
  }

  async function importMasterKey(filePath: string, passphrase: string) {
    if (!projectId.value) throw new Error("没有活动项目");
    const expectedProjectId = projectId.value;
    keyOperationLoading.value = true;
    error.value = "";
    try {
      const result = await useWorkbenchAdapter().importReleaseMasterKey(expectedProjectId, { filePath, passphrase });
      if (projectId.value === expectedProjectId) await load(expectedProjectId);
      return result;
    } catch (cause) {
      error.value = commandErrorText(cause, "导入项目主密钥失败");
      throw cause;
    } finally {
      keyOperationLoading.value = false;
    }
  }

  return {
    projectId,
    profile,
    draft,
    validation,
    loading,
    saving,
    keyOperationLoading,
    error,
    conflict,
    load,
    resetDraft,
    validate,
    save,
    exportMasterKey,
    importMasterKey
  };
});
