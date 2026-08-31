import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { configureDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import { FixtureDataDirectoryAdapter } from "@/dev-fixtures/dataDirectoryFixtureAdapter";
import { useDataDirectoryStore } from "@/stores/dataDirectory";

describe("data directory store", () => {
  beforeEach(() => {
    setActivePinia(createPinia());
    configureDataDirectoryAdapter(new FixtureDataDirectoryAdapter());
  });

  it("keeps fixture state behind an adapter and marks restart pending", async () => {
    const store = useDataDirectoryStore();
    await store.initialize();
    expect(store.activeDirectory).toContain("Fixture");
    await store.scheduleSwitch({
      targetDirectory: "D:\\INX\\NewDeskBuddy",
      mode: "migrate"
    });
    expect(store.status?.restartRequired).toBe(true);
    expect(store.status?.pendingDirectory).toBe("D:\\INX\\NewDeskBuddy");
  });
});
