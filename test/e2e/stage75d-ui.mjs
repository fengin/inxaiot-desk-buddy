import { spawn } from "node:child_process";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { remote } from "webdriverio";

const projectRoot = resolve(import.meta.dirname, "../..");
const binary = resolve(projectRoot, "src-tauri/target/debug/inxaiot-desk-buddy.exe");
const defaultDirectory = resolve(
  process.env.APPDATA ?? "",
  "com.inxaiot.desk-buddy.stage75d"
);
const targetDirectory = process.env.INX_STAGE75D_DATA_DIR;
const nonEmptyDirectory = process.env.INX_STAGE75D_NONEMPTY_DIR;
const webdriverPort = 4447;

if (!existsSync(binary)) throw new Error(`阶段7.5-D E2E二进制不存在：${binary}`);
if (!targetDirectory || !nonEmptyDirectory) {
  throw new Error("必须提供INX_STAGE75D_DATA_DIR和INX_STAGE75D_NONEMPTY_DIR");
}
for (const directory of [defaultDirectory, targetDirectory, nonEmptyDirectory]) {
  if (existsSync(directory)) throw new Error(`验收目录已存在，拒绝复用：${directory}`);
}
mkdirSync(nonEmptyDirectory, { recursive: true });
writeFileSync(resolve(nonEmptyDirectory, "keep.txt"), "stage75d-keep", "utf8");

let app;
let browser;
const commandTrace = [];

async function connect() {
  let lastError;
  for (let attempt = 0; attempt < 50; attempt += 1) {
    try {
      return await remote({
        hostname: "127.0.0.1",
        port: webdriverPort,
        logLevel: "silent",
        capabilities: {}
      });
    } catch (error) {
      lastError = error;
      await delay(400);
    }
  }
  throw lastError;
}

async function startApplication() {
  app = spawn(binary, [], {
    cwd: projectRoot,
    env: { ...process.env, TAURI_WEBDRIVER_PORT: String(webdriverPort) },
    stdio: "ignore",
    windowsHide: true
  });
  browser = await connect();
  await browser.execute(() => {
    globalThis.__stage75dCommands = [];
    const internals = globalThis.__TAURI_INTERNALS__;
    if (internals?.invoke && !internals.__stage75dWrapped) {
      const originalInvoke = internals.invoke.bind(internals);
      internals.invoke = async (command, args, options) => {
        globalThis.__stage75dCommands.push(`${command}:started`);
        try {
          const result = await originalInvoke(command, args, options);
          globalThis.__stage75dCommands.push(`${command}:succeeded`);
          return result;
        } catch (error) {
          globalThis.__stage75dCommands.push(
            `${command}:failed:${String(error?.code ?? error?.message ?? error)}`
          );
          throw error;
        }
      };
      internals.__stage75dWrapped = true;
    }
  });
  await waitBody("INX 实施工作台");
}

async function stopApplication() {
  try {
    const trace = await browser.execute(() => globalThis.__stage75dCommands ?? []);
    commandTrace.push(...trace);
  } catch {
    // 仅诊断；目录与界面实证仍是主门禁。
  }
  const exit = new Promise((resolveExit) => app.once("exit", resolveExit));
  await clickSelector('button[aria-label="关闭"]');
  await Promise.race([
    exit,
    delay(15_000).then(() => {
      throw new Error("应用未在15秒内正常关闭");
    })
  ]);
  try { await browser.deleteSession(); } catch { /* 窗口已退出 */ }
  browser = undefined;
  app = undefined;
  await delay(800);
}

async function testElement(testId) {
  const element = await browser.$(`[data-testid="${testId}"]`);
  await element.waitForExist({ timeout: 20_000 });
  return element;
}

async function clickTest(testId) {
  const element = await testElement(testId);
  await element.waitForDisplayed({ timeout: 20_000 });
  await element.click();
}

async function clickSelector(selector) {
  const element = await browser.$(selector);
  await element.waitForDisplayed({ timeout: 20_000 });
  await element.click();
}

async function setInput(testId, value) {
  await testElement(testId);
  const updated = await browser.execute((id, nextValue) => {
    const root = globalThis.document.querySelector(`[data-testid="${id}"]`);
    const input = root?.matches("input,textarea")
      ? root
      : root?.querySelector("input,textarea");
    if (
      !(input instanceof globalThis.HTMLInputElement)
      && !(input instanceof globalThis.HTMLTextAreaElement)
    ) return false;
    const prototype = input instanceof globalThis.HTMLTextAreaElement
      ? globalThis.HTMLTextAreaElement.prototype
      : globalThis.HTMLInputElement.prototype;
    const setter = Object.getOwnPropertyDescriptor(prototype, "value")?.set;
    if (!setter) return false;
    setter.call(input, String(nextValue));
    input.dispatchEvent(new globalThis.Event("input", { bubbles: true }));
    input.dispatchEvent(new globalThis.Event("change", { bubbles: true }));
    return true;
  }, testId, String(value));
  if (!updated) throw new Error(`无法输入界面字段：${testId}`);
}

async function waitBody(text, timeout = 25_000) {
  await browser.waitUntil(
    async () => (await browser.$("body").getText()).includes(text),
    { timeout, timeoutMsg: `界面未出现：${text}` }
  );
}

async function assertActualDirectory(expected) {
  await clickTest("open-about");
  await waitBody("实际数据目录");
  await waitBody(expected);
  await waitBody("协议 1");
  await waitBody("8787fa59f40ffbd2dcb3ceda0fbe378cf6622de9f208aac687c8d5b554ff95c6");
  await clickSelector(".about-modal .n-card-header__close");
}

try {
  await startApplication();
  await waitBody("当前项目上下文未就绪");
  await clickTest("open-preferences");
  await waitBody("当前实际目录");
  await setInput("data-directory-input", targetDirectory);
  await clickTest("preferences-save");
  await waitBody("数据目录切换已安全登记");
  await stopApplication();

  if (!existsSync(resolve(defaultDirectory, "local.db"))) {
    throw new Error("首次启动未在默认目录创建local.db");
  }
  if (existsSync(resolve(targetDirectory, "local.db"))) {
    throw new Error("重启前目标目录不应提前生效");
  }

  await startApplication();
  if (!existsSync(resolve(targetDirectory, "local.db"))) {
    throw new Error("重启后目标目录没有实际local.db");
  }
  for (const child of ["logs", "task-logs", "task-artifacts"]) {
    if (!existsSync(resolve(targetDirectory, child))) {
      throw new Error(`重启后目标目录缺少：${child}`);
    }
  }
  await assertActualDirectory(targetDirectory);
  await clickTest("open-preferences");
  await clickTest("data-directory-rollback");
  await waitBody("数据目录回滚已登记");
  await clickTest("preferences-save");
  await stopApplication();

  await startApplication();
  await assertActualDirectory(defaultDirectory);
  await clickTest("open-preferences");
  await setInput("data-directory-input", nonEmptyDirectory);
  await clickTest("preferences-save");
  await waitBody("目标数据目录不是空目录");
  await setInput("data-directory-input", defaultDirectory);
  await clickTest("preferences-save");

  const finalTrace = await browser.execute(() => globalThis.__stage75dCommands ?? []);
  commandTrace.push(...finalTrace);
  await stopApplication();
  process.stdout.write(
    `STAGE75D_UI_GATE_PASS|default=${defaultDirectory}|target=${targetDirectory}\n`
  );
} catch (error) {
  let body = "";
  try { body = await browser?.$("body").getText(); } catch { /* 仅诊断 */ }
  process.stderr.write(
    `STAGE75D_UI_GATE_FAIL|${String(error?.message ?? error)}|commands=${JSON.stringify(commandTrace)}|body=${body.slice(0, 1800)}\n`
  );
  process.exitCode = 1;
} finally {
  if (browser) {
    try { await browser.deleteSession(); } catch { /* 已退出 */ }
  }
  if (app && !app.killed) app.kill();
}
