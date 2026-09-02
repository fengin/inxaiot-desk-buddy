import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it, vi } from "vitest";

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

  it("loads release configuration without a manual host-key prerequisite", async () => {
    const adapter = new FixtureWorkbenchAdapter();
    const hostKeys = vi.spyOn(adapter, "listHostKeys").mockRejectedValue(new Error("not a UI dependency"));
    configureWorkbenchAdapter(adapter);
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    expect(store.profile).toBeDefined();
    expect(hostKeys).not.toHaveBeenCalled();
  });

  it("exports and imports a versioned project master key through the adapter", async () => {
    const store = useReleaseProfileStore();
    await store.load("project-shenzhen-bay");
    const exported = await store.exportMasterKey(
      "D:\\secure\\project-key.inxkey",
      "strong-passphrase"
    );
    expect(exported.keyVersion).toBe(1);
    const imported = await store.importMasterKey(
      "D:\\secure\\project-key.inxkey",
      "strong-passphrase"
    );
    expect(imported.keyVersion).toBe(1);
    expect(store.keyOperationLoading).toBe(false);
  });
});
