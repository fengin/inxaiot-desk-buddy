import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const sourceRoot = join(process.cwd(), "src");

function productionFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return productionFiles(path);
    if (!/\.(ts|vue)$/.test(entry.name) || entry.name.endsWith(".spec.ts")) return [];
    return [path];
  });
}

describe("Tauri真实模式物理隔离门禁", () => {
  it("configures every production adapter as Real and refuses a non-Tauri production runtime", () => {
    const main = readFileSync(join(sourceRoot, "main.ts"), "utf8");
    for (const adapter of [
      "RealWorkbenchAdapter",
      "RealActivityAdapter",
      "RealAioAdapter",
      "RealOperationsAdapter",
      "RealDataDirectoryAdapter",
      "RealDiagnosticsAdapter",
      "RealSystemDialogAdapter"
    ]) {
      expect(main).toContain(`new ${adapter}()`);
    }
    expect(main).toContain("生产构建只能在 Tauri Runtime 中使用");
  });

  it("keeps Tauri dialog plugin access inside the Real system adapter", () => {
    const violations = productionFiles(sourceRoot)
      .filter((path) => !path.endsWith(join("shared", "api", "realSystemDialogAdapter.ts")))
      .filter((path) => readFileSync(path, "utf8").includes("@tauri-apps/plugin-dialog"));
    expect(violations).toEqual([]);
  });

  it("keeps feature, shell and store production modules free of Demo/Fixture imports", () => {
    const roots = ["features", "shell", "stores"].map((value) => join(sourceRoot, value));
    const violations = roots
      .flatMap(productionFiles)
      .filter((path) => !path.endsWith(`${join("stores", "demo.ts")}`))
      .filter((path) => /@\/dev-fixtures|stores\/demo|DemoStore/.test(readFileSync(path, "utf8")));
    expect(violations).toEqual([]);
  });

  it("requires every visible Vue button to have an action or an explicit disabled state", () => {
    const violations: string[] = [];
    for (const path of productionFiles(sourceRoot).filter((value) => value.endsWith(".vue"))) {
      const source = readFileSync(path, "utf8");
      for (const match of source.matchAll(/<(?:n-button|button)\b[^>]*>/gs)) {
        const tag = match[0];
        if (
          !/@click(?:\.[\w-]+)*=|\bdisabled\b|:disabled=|type="submit"|\bto=|\bhref=|data-action-owner=/.test(tag)
        ) {
          violations.push(`${path}: ${tag.replace(/\s+/g, " ").slice(0, 140)}`);
        }
      }
    }
    expect(violations).toEqual([]);
  });
});
