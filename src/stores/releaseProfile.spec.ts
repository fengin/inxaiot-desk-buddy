import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { useReleaseProfileStore } from "@/stores/releaseProfile";

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

  it("captures and confirms a first-use HostKey", async () => {
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const captured = await store.captureHostKey("192.168.3.79", 22);
    expect(captured.state).toBe("unconfirmed");
    const confirmed = await store.confirmHostKey(false);
    expect(confirmed.state).toBe("confirmed");
    expect(store.hostKeys).toHaveLength(1);
  });

  it("exports, rotates and imports a versioned project master key through the adapter", async () => {
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const exported = await store.exportMasterKey(
      "D:\\secure\\project-key.inxkey",
      "strong-passphrase"
    );
    expect(exported.keyVersion).toBe(1);
    const rotated = await store.rotateMasterKey();
    expect(rotated.keyVersion).toBe(2);
    const imported = await store.importMasterKey(
      "D:\\secure\\project-key.inxkey",
      "strong-passphrase"
    );
    expect(imported.keyVersion).toBe(2);
    expect(store.keyOperationLoading).toBe(false);
  });
});
