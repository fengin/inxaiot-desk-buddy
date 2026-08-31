import { defineConfig, presetWind3 } from "unocss";

export default defineConfig({
  presets: [presetWind3()],
  shortcuts: {
    "focus-ring": "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-[var(--inx-color-focus)]"
  },
  theme: {
    fontFamily: {
      sans: "var(--inx-font-ui)",
      mono: "var(--inx-font-mono)"
    }
  }
});

