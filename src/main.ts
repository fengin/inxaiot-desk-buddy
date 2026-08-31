import "virtual:uno.css";
import "@/app/design-tokens.css";
import "@/app/styles.css";

import { createPinia } from "pinia";
import { createApp } from "vue";

import App from "@/app/App.vue";
import { i18n } from "@/app/i18n";
import { router } from "@/app/router";
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { RealWorkbenchAdapter } from "@/shared/api/realWorkbenchAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { RealActivityAdapter } from "@/shared/api/realActivityAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { RealAioAdapter } from "@/shared/api/realAioAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { RealOperationsAdapter } from "@/shared/api/realOperationsAdapter";
import { isTauriRuntime } from "@/shared/api/activity";
import { configureDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import { RealDataDirectoryAdapter } from "@/shared/api/realDataDirectoryAdapter";
import { configureDiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";
import { RealDiagnosticsAdapter } from "@/shared/api/realDiagnosticsAdapter";
import { configureSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { RealSystemDialogAdapter } from "@/shared/api/realSystemDialogAdapter";

async function bootstrap() {
  if (isTauriRuntime()) {
    configureWorkbenchAdapter(new RealWorkbenchAdapter());
    configureActivityAdapter(new RealActivityAdapter());
    configureAioAdapter(new RealAioAdapter());
    configureOperationsAdapter(new RealOperationsAdapter());
    configureDataDirectoryAdapter(new RealDataDirectoryAdapter());
    configureDiagnosticsAdapter(new RealDiagnosticsAdapter());
    configureSystemDialogAdapter(new RealSystemDialogAdapter());
  } else if (import.meta.env.DEV) {
    const { FixtureWorkbenchAdapter } = await import("@/dev-fixtures/workbenchFixtureAdapter");
    const { FixtureActivityAdapter } = await import("@/dev-fixtures/activityFixtureAdapter");
    const { FixtureAioAdapter } = await import("@/dev-fixtures/aioFixtureAdapter");
    const { FixtureOperationsAdapter } = await import("@/dev-fixtures/operationsFixtureAdapter");
    const { FixtureDataDirectoryAdapter } = await import("@/dev-fixtures/dataDirectoryFixtureAdapter");
    const { FixtureDiagnosticsAdapter } = await import("@/dev-fixtures/diagnosticsFixtureAdapter");
    const { FixtureSystemDialogAdapter } = await import("@/dev-fixtures/systemDialogFixtureAdapter");
    configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
    configureActivityAdapter(new FixtureActivityAdapter());
    configureAioAdapter(new FixtureAioAdapter());
    configureOperationsAdapter(new FixtureOperationsAdapter());
    configureDataDirectoryAdapter(new FixtureDataDirectoryAdapter());
    configureDiagnosticsAdapter(new FixtureDiagnosticsAdapter());
    configureSystemDialogAdapter(new FixtureSystemDialogAdapter());
  } else {
    throw new Error("生产构建只能在 Tauri Runtime 中使用，Fixture Adapter 已禁用");
  }
  createApp(App).use(createPinia()).use(router).use(i18n).mount("#app");
}

void bootstrap();
