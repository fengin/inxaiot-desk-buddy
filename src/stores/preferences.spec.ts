import { createPinia, setActivePinia } from "pinia";
import { beforeEach, describe, expect, it } from "vitest";

import { usePreferencesStore } from "@/stores/preferences";

describe("preferences store", () => {
  beforeEach(() => {
    localStorage.clear();
    setActivePinia(createPinia());
  });

  it("persists theme, density and page size as one preference snapshot", () => {
    const store = usePreferencesStore();
    store.apply({
      theme: "dark",
      fontSize: "large",
      density: "comfortable",
      reduceMotion: true,
      pageSize: 100,
      navigationCollapsed: true
    });
    const persisted = JSON.parse(localStorage.getItem("inxaiot-desk-buddy.preferences.v1") ?? "{}");
    expect(persisted).toMatchObject({ theme: "dark", fontSize: "large", density: "comfortable", pageSize: 100 });
  });
});

