import { afterEach } from "vitest";

afterEach(() => {
  document.body.innerHTML = "";
  localStorage.clear();
});
import { configureWorkbenchAdapter } from "@/shared/api/workbenchAdapter";
import { FixtureWorkbenchAdapter } from "@/dev-fixtures/workbenchFixtureAdapter";
import { configureActivityAdapter } from "@/shared/api/activityAdapter";
import { FixtureActivityAdapter } from "@/dev-fixtures/activityFixtureAdapter";
import { configureAioAdapter } from "@/shared/api/aioAdapter";
import { FixtureAioAdapter } from "@/dev-fixtures/aioFixtureAdapter";
import { configureOperationsAdapter } from "@/shared/api/operationsAdapter";
import { FixtureOperationsAdapter } from "@/dev-fixtures/operationsFixtureAdapter";
import { configureDataDirectoryAdapter } from "@/shared/api/dataDirectoryAdapter";
import { FixtureDataDirectoryAdapter } from "@/dev-fixtures/dataDirectoryFixtureAdapter";
import { configureDiagnosticsAdapter } from "@/shared/api/diagnosticsAdapter";
import { FixtureDiagnosticsAdapter } from "@/dev-fixtures/diagnosticsFixtureAdapter";
import { configureSystemDialogAdapter } from "@/shared/api/systemDialogAdapter";
import { FixtureSystemDialogAdapter } from "@/dev-fixtures/systemDialogFixtureAdapter";

configureWorkbenchAdapter(new FixtureWorkbenchAdapter());
configureActivityAdapter(new FixtureActivityAdapter());
configureAioAdapter(new FixtureAioAdapter());
configureOperationsAdapter(new FixtureOperationsAdapter());
configureDataDirectoryAdapter(new FixtureDataDirectoryAdapter());
configureDiagnosticsAdapter(new FixtureDiagnosticsAdapter());
configureSystemDialogAdapter(new FixtureSystemDialogAdapter());
