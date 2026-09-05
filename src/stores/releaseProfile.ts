import { defineStore } from "pinia";
import { ref, watch } from "vue";

import { commandErrorCode, commandErrorText, commandFieldErrors } from "@/shared/api/errors";
import { useWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import {
  emptyReleaseProfileDraft,
  type ReleaseProfileDraft,
  type ReleaseProfileField,
  type ReleaseProfileFieldErrors,
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
  const agentReplacing = ref(false);
  const agentOpening = ref(false);
  const error = ref("");
  const conflict = ref(false);
  const fieldErrors = ref<ReleaseProfileFieldErrors>({});
  let loadRequest = 0;
  let mutationRequest = 0;
  let draftRevision = 0;

  function draftFields(candidate: ReleaseProfileDraft) {
    return Object.fromEntries(
      Object.entries(candidate).filter(([group]) => group !== "expectedVersion")
        .flatMap(([group, values]) => Object.entries(values).map(([key, value]) => [`${group}.${key}`, value]))
    );
  }

  let previousFields = draftFields(draft.value);
  watch(draft, () => {
    draftRevision += 1;
    validation.value = undefined;
    const nextFields = draftFields(draft.value);
    const remaining = { ...fieldErrors.value };
    const hadErrors = Object.keys(remaining).length > 0;
    const sharedSshError = remaining["credentials.sshPassword"]
      && remaining["credentials.sshPassword"] === remaining["credentials.sshPrivateKey"];
    for (const field of Object.keys(remaining) as ReleaseProfileField[]) {
      if (nextFields[field] !== previousFields[field]) delete remaining[field];
    }
    if (sharedSshError && (nextFields["credentials.sshPassword"] !== previousFields["credentials.sshPassword"]
      || nextFields["credentials.sshPrivateKey"] !== previousFields["credentials.sshPrivateKey"])) {
      delete remaining["credentials.sshPassword"];
      delete remaining["credentials.sshPrivateKey"];
    }
    if (nextFields["values.envTemplate"] !== previousFields["values.envTemplate"]) {
      delete remaining["values.composeTemplate"];
    }
    fieldErrors.value = remaining;
    if (hadErrors && !Object.keys(remaining).length) error.value = "";
    previousFields = nextFields;
  }, { deep: true, flush: "sync" });

  function checkNumericInputs(candidate: ReleaseProfileDraft) {
    const invalid: ReleaseProfileFieldErrors = {};
    for (const field of ["platformApiPort", "platformMqttPort", "sshPort", "sshTimeoutSeconds"] as const) {
      const value = candidate.values[field];
      const max = field === "sshTimeoutSeconds" ? 3600 : 65535;
      if (!Number.isInteger(value) || value < 1 || value > max) {
        invalid[`values.${field}`] = field === "sshTimeoutSeconds"
          ? "连接超时必须是 1～3600 秒的整数" : "端口必须是 1～65535 的整数";
      }
    }
    if (Object.keys(invalid).length) {
      throw { code: "CONFIG_VALIDATION_FAILED", params: { summary: "请修正标红的输入项" }, fieldErrors: invalid };
    }
  }

  function cloneDraft(source: ReleaseProfileDraft): ReleaseProfileDraft {
    return {
      values: { ...source.values },
      credentials: { ...source.credentials },
      expectedVersion: source.expectedVersion
    };
  }

  function resetDraft() {
    validation.value = undefined;
    fieldErrors.value = {};
    error.value = "";
    conflict.value = false;
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
    fieldErrors.value = {};
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
    const revision = draftRevision;
    error.value = "";
    validation.value = undefined;
    fieldErrors.value = {};
    try {
      checkNumericInputs(candidate);
      const result = await useWorkbenchAdapter().validateReleaseProfile(candidate);
      if (request === mutationRequest && projectId.value === expectedProjectId && revision === draftRevision) {
        validation.value = result;
      }
      return result;
    } catch (cause) {
      if (request === mutationRequest && projectId.value === expectedProjectId && revision === draftRevision) {
        validation.value = undefined;
        error.value = commandErrorText(cause, "发布参数校验失败");
        fieldErrors.value = commandFieldErrors(cause);
      }
      throw cause;
    }
  }

  async function save() {
    if (!projectId.value) throw new Error("没有活动项目");
    const expectedProjectId = projectId.value;
    const request = ++mutationRequest;
    const candidate = cloneDraft(draft.value);
    const revision = draftRevision;
    saving.value = true;
    error.value = "";
    conflict.value = false;
    validation.value = undefined;
    fieldErrors.value = {};
    try {
      checkNumericInputs(candidate);
      const checked = await useWorkbenchAdapter().validateReleaseProfile(candidate);
      const saved = await useWorkbenchAdapter().saveReleaseProfile(expectedProjectId, candidate);
      if (request === mutationRequest && projectId.value === expectedProjectId) {
        profile.value = saved;
        resetDraft();
        validation.value = checked;
      }
      return saved;
    } catch (cause) {
      if (request === mutationRequest && projectId.value === expectedProjectId && revision === draftRevision) {
        conflict.value = commandErrorCode(cause) === "CONFIG_VERSION_CONFLICT";
        error.value = commandErrorText(cause, "保存发布参数失败");
        fieldErrors.value = commandFieldErrors(cause);
      }
      throw cause;
    } finally {
      if (request === mutationRequest) saving.value = false;
    }
  }

  async function replaceAgentScript(filePath: string) {
    if (!projectId.value || !profile.value) throw new Error("请先保存发布参数");
    const expectedProjectId = projectId.value;
    const expectedVersion = profile.value.version;
    const request = ++mutationRequest;
    agentReplacing.value = true;
    error.value = "";
    conflict.value = false;
    try {
      const saved = await useWorkbenchAdapter().replaceReleaseAgentScript(expectedProjectId, {
        filePath,
        expectedVersion
      });
      if (request === mutationRequest && projectId.value === expectedProjectId) {
        profile.value = saved;
        // 脚本是独立于发布参数草稿的共享配置；更新脚本不能丢弃用户正在编辑的参数。
        // 仅把草稿的乐观锁基线推进到脚本更新后的版本，后续保存才会写入这份草稿。
        draft.value = {
          values: { ...draft.value.values },
          credentials: { ...draft.value.credentials },
          expectedVersion: saved.version
        };
      }
      return saved;
    } catch (cause) {
      if (request === mutationRequest && projectId.value === expectedProjectId) {
        conflict.value = commandErrorCode(cause) === "CONFIG_VERSION_CONFLICT";
        error.value = commandErrorText(cause, "更换一体机脚本失败");
      }
      throw cause;
    } finally {
      if (request === mutationRequest) agentReplacing.value = false;
    }
  }

  async function openAgentScript() {
    if (!projectId.value || !profile.value) throw new Error("请先保存发布参数");
    agentOpening.value = true;
    try {
      await useWorkbenchAdapter().openReleaseAgentScript(projectId.value);
    } finally {
      agentOpening.value = false;
    }
  }

  return {
    projectId,
    profile,
    draft,
    validation,
    loading,
    saving,
    agentReplacing,
    agentOpening,
    error,
    conflict,
    fieldErrors,
    load,
    resetDraft,
    validate,
    save,
    replaceAgentScript,
    openAgentScript
  };
});
