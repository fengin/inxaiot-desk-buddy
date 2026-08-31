import { defineStore } from "pinia";
import { computed, ref } from "vue";

export type ThemeMode = "system" | "light" | "dark";
export type FontSizeMode = "small" | "standard" | "large";
export type DensityMode = "compact" | "comfortable";

interface PreferenceSnapshot {
  theme: ThemeMode;
  fontSize: FontSizeMode;
  density: DensityMode;
  reduceMotion: boolean;
  pageSize: 20 | 50 | 100;
  navigationCollapsed: boolean;
}

const storageKey = "inxaiot-desk-buddy.preferences.v1";

const defaults: PreferenceSnapshot = {
  theme: "system",
  fontSize: "standard",
  density: "compact",
  reduceMotion: false,
  pageSize: 50,
  navigationCollapsed: false
};

function loadPreferences(): PreferenceSnapshot {
  try {
    const value = localStorage.getItem(storageKey);
    return value ? { ...defaults, ...(JSON.parse(value) as Partial<PreferenceSnapshot>) } : { ...defaults };
  } catch {
    return { ...defaults };
  }
}

export const usePreferencesStore = defineStore("preferences", () => {
  const initial = loadPreferences();
  const theme = ref<ThemeMode>(initial.theme);
  const fontSize = ref<FontSizeMode>(initial.fontSize);
  const density = ref<DensityMode>(initial.density);
  const reduceMotion = ref(initial.reduceMotion);
  const pageSize = ref<20 | 50 | 100>(initial.pageSize);
  const navigationCollapsed = ref(initial.navigationCollapsed);

  const snapshot = computed<PreferenceSnapshot>(() => ({
    theme: theme.value,
    fontSize: fontSize.value,
    density: density.value,
    reduceMotion: reduceMotion.value,
    pageSize: pageSize.value,
    navigationCollapsed: navigationCollapsed.value
  }));

  function apply(next: PreferenceSnapshot) {
    theme.value = next.theme;
    fontSize.value = next.fontSize;
    density.value = next.density;
    reduceMotion.value = next.reduceMotion;
    pageSize.value = next.pageSize;
    navigationCollapsed.value = next.navigationCollapsed;
    localStorage.setItem(storageKey, JSON.stringify(snapshot.value));
  }

  function toggleNavigation() {
    navigationCollapsed.value = !navigationCollapsed.value;
    localStorage.setItem(storageKey, JSON.stringify(snapshot.value));
  }

  return { theme, fontSize, density, reduceMotion, pageSize, navigationCollapsed, snapshot, apply, toggleNavigation };
});
