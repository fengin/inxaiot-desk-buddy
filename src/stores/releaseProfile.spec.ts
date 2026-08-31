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
});
