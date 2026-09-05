import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useReleaseProfileStore } from "@/stores/releaseProfile";

class CredentialsResetRequiredAdapter extends FixtureWorkbenchAdapter {
  override async getReleaseProfile(projectId: string) {
    const profile = await super.getReleaseProfile(projectId);
    return profile && { ...profile, credentialsResetRequired: true };
  }
}

describe("release profile store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
  });

  it("loads, validates and saves with an optimistic version", async () => {
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const before = store.profile!.version;
    const validation = await store.validate();
    expect(validation.valid).toBe(true);
    await store.save();
    expect(store.profile!.version).toBe(before + 1);
  });

  it("loads release configuration without a manual host-key prerequisite", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const hostKeys = vi.spyOn(adapter, "listHostKeys").mockRejectedValue(new Error("not a UI dependency"));
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    expect(store.profile).toBeDefined();
    expect(hostKeys).not.toHaveBeenCalled();
  });

  it("retains the credential reset marker until the user saves the re-entered configuration", async () => {
    const adapter = new CredentialsResetRequiredAdapter();
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();

    await store.load("project-shenzhen-bay");
    expect(store.profile?.credentialsResetRequired).toBe(true);

    await store.save();
    expect(store.profile?.credentialsResetRequired).toBe(false);
  });

  it("replaces the shared agent script while retaining the unsaved draft and updating its version baseline", async () => {
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const before = store.profile!.version;
    expect(store.profile!.agentScript.source).toBe("built_in");
    store.draft.values.platformHost = "draft-platform.example";
    store.draft.credentials.platformAuthKey = "draft-platform-auth-key";

    await store.replaceAgentScript("D:\\release\\edge-node-agent.sh");

    expect(store.profile!.version).toBe(before + 1);
    expect(store.profile!.agentScript.source).toBe("project");
    expect(store.profile!.agentScript.version).toBe("0.1.9");
    expect(store.draft.values.platformHost).toBe("draft-platform.example");
    expect(store.draft.credentials.platformAuthKey).toBe("draft-platform-auth-key");
    expect(store.draft.expectedVersion).toBe(before + 1);

    const saved = await store.save();
    expect(saved.values.platformHost).toBe("draft-platform.example");
    expect(saved.credentials.platformAuthKey).toBe("draft-platform-auth-key");
  });

  it("blocks invalid saves, clears only edited field feedback and invalidates previous success", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const validation = vi.spyOn(adapter, "validateReleaseProfile").mockRejectedValueOnce({
      code: "CONFIG_VALIDATION_FAILED",
      params: { summary: "请修正标红的输入项" },
      fieldErrors: {
        "credentials.platformAuthKey": "请填写平台 API AuthKey",
        "credentials.platformMqttPassword": "请填写平台 MQTT 密码"
      }
    });
    const save = vi.spyOn(adapter, "saveReleaseProfile");
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    store.draft.credentials.platformAuthKey = "";
    store.draft.credentials.platformMqttPassword = "";
    await expect(store.save()).rejects.toMatchObject({ code: "CONFIG_VALIDATION_FAILED" });
    expect(save).not.toHaveBeenCalled();
    expect(Object.keys(store.fieldErrors)).toHaveLength(2);
    store.draft.credentials.platformAuthKey = "corrected-test-key";
    expect(store.fieldErrors["credentials.platformAuthKey"]).toBeUndefined();
    expect(store.fieldErrors["credentials.platformMqttPassword"]).toBeDefined();
    store.draft.credentials.platformMqttPassword = "corrected-test-password";
    expect(store.fieldErrors).toEqual({});
    expect(store.error).toBe("");
    await store.validate();
    expect(store.validation?.valid).toBe(true);
    store.draft.values.platformHost = "changed.example";
    expect(store.validation).toBeUndefined();
    expect(validation).toHaveBeenCalledTimes(2);
  });

  it("locates empty, out-of-range and fractional numeric inputs before IPC deserialization", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const validate = vi.spyOn(adapter, "validateReleaseProfile");
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    Object.assign(store.draft.values, {
      platformApiPort: null, platformMqttPort: 65536, sshPort: 22.5, sshTimeoutSeconds: 0
    });
    await expect(store.validate()).rejects.toMatchObject({ code: "CONFIG_VALIDATION_FAILED" });
    expect(validate).not.toHaveBeenCalled();
    expect(Object.keys(store.fieldErrors)).toHaveLength(4);
    store.resetDraft();
    expect(store.fieldErrors).toEqual({});
    expect(store.error).toBe("");
  });

  it("clears the shared SSH requirement when either alternative is edited", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    vi.spyOn(adapter, "validateReleaseProfile").mockRejectedValueOnce({
      fieldErrors: {
        "credentials.sshPassword": "SSH 密码和私钥至少填写一项",
        "credentials.sshPrivateKey": "SSH 密码和私钥至少填写一项"
      }
    });
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    await expect(store.validate()).rejects.toBeDefined();
    store.draft.credentials.sshPassword = "corrected-test-password";
    expect(store.fieldErrors).toEqual({});
  });

  it("does not apply a delayed validation error to an edited draft", async () => {
    let rejectValidation!: (reason: unknown) => void;
    const adapter = new FixtureWorkbenchAdapter();
    vi.spyOn(adapter, "validateReleaseProfile").mockImplementationOnce(() => new Promise((_, reject) => {
      rejectValidation = reject;
    }));
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const pending = store.validate().catch(() => undefined);
    store.draft.credentials.platformAuthKey = "corrected-test-key";
    rejectValidation({ fieldErrors: { "credentials.platformAuthKey": "旧输入错误" } });
    await pending;
    expect(store.fieldErrors).toEqual({});
    expect(store.error).toBe("");
  });
});
