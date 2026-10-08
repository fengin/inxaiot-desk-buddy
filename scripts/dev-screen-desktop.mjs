import { spawn } from "node:child_process";
import { fileURLToPath, URL } from "node:url";
import process from "node:process";
import console from "node:console";

const root = fileURLToPath(new URL("../", import.meta.url));
const frontendOnly = process.argv.includes("--frontend");
const args = frontendOnly
  ? [fileURLToPath(new URL("../node_modules/vite/bin/vite.js", import.meta.url)), "--host", "127.0.0.1", "--port", "1421", "--strictPort"]
  : [fileURLToPath(new URL("../node_modules/@tauri-apps/cli/tauri.js", import.meta.url)), "dev", "--config", "src-tauri/tauri.screen-prototype.conf.json"];

console.log(frontendOnly ? "启动桌面原型热更新服务（1421）" : "启动智能屏桌面开发原型：独立标识与数据目录，全部使用模拟适配器");
const child = spawn(process.execPath, args, {
  cwd: root,
  env: { ...process.env, VITE_SCREEN_PROTOTYPE: "1" },
  stdio: "inherit",
  windowsHide: true
});
child.on("error", (error) => { console.error(error.message); process.exitCode = 1; });
child.on("exit", (code) => { process.exitCode = code ?? 1; });
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => child.kill(signal));
