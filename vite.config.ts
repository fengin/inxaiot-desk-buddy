import { fileURLToPath, URL } from "node:url";

import vue from "@vitejs/plugin-vue";
import UnoCSS from "unocss/vite";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [vue(), UnoCSS()],
  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url))
    }
  },
  clearScreen: false,
  server: {
    strictPort: true
  },
  test: {
    environment: "happy-dom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
    pool: "threads",
    maxWorkers: 1,
    fileParallelism: false
  }
});
