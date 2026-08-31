import { spawn, spawnSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { resolve } from "node:path";
import process from "node:process";
import { setTimeout as delay } from "node:timers/promises";
import { remote } from "webdriverio";

const projectRoot = resolve(import.meta.dirname, "../..");
const binary = resolve(projectRoot, "src-tauri/target/debug/inxaiot-desk-buddy.exe");
const stage75b = process.env.INX_STAGE75B_UI === "true";
const stageLabel = stage75b ? "7.5-B" : "7.5-A";
const isolatedSchema = process.env.INX_STAGE75_SCHEMA;
const stageDataDirectory = resolve(
  process.env.APPDATA ?? "",
  stage75b ? "com.inxaiot.desk-buddy.stage75b" : "com.inxaiot.desk-buddy.stage75a"
);
const webdriverPort = stage75b ? 4446 : 4445;

if (!isolatedSchema?.startsWith("inxaiot_desk_buddy_stage75_")) {
  throw new Error("INX_STAGE75_SCHEMA必须是本次隔离工作台Schema");
}
if (!existsSync(binary)) throw new Error(`阶段7.5-A E2E二进制不存在：${binary}`);
if (existsSync(stageDataDirectory)) {
  throw new Error(`独立验收数据目录已存在，拒绝复用：${stageDataDirectory}`);
}

const description = readFileSync(resolve(projectRoot, "test/测试数据说明.txt"), "utf8");
const platformYaml = readFileSync(
  resolve(projectRoot, "../../inxvision-platform/inxaiot-starter-platform/src/main/resources/application-dev.yml"),
  "utf8"
);
const envTemplate = readFileSync(resolve(projectRoot, "test/templates/env.template"), "utf8");
const composeTemplate = readFileSync(resolve(projectRoot, "test/docker-compose.yml"), "utf8");
const privateKey = readFileSync(resolve(projectRoot, "test/id_rsa"), "utf8");

function line(label) {
  const value = description.split(/\r?\n/).find((item) => item.trim().startsWith(label));
  if (!value) throw new Error(`测试数据缺少：${label}`);
  return value.trim().slice(label.length).trim();
}

function yamlDefault(key) {
  const match = platformYaml.match(new RegExp(`\\$\\{${key}:([^}]+)\\}`));
  if (!match) throw new Error(`平台开发配置缺少：${key}`);
  return match[1];
}

const loginStart = description.indexOf("{");
const loginEnd = description.indexOf("}", loginStart);
const login = JSON.parse(description.slice(loginStart, loginEnd + 1));
const [apiHost, apiPort] = line("平台API：").split(":");
const [mqttHost, mqttPort] = line("平台mqtt：").split(":");
const sensitiveValues = [
  yamlDefault("MYSQL_PASSWORD"),
  login.credentials,
  login.sessionUUID,
  login.imageCode,
  line("平台API auth Key："),
  line("平台mqtt密码："),
  privateKey
].filter(Boolean);

function redact(value) {
  let text = String(value ?? "未知错误");
  for (const secret of sensitiveValues) text = text.replaceAll(secret, "[REDACTED]");
  return text;
}

function prepareStage75bTestBinary() {
  const deps = resolve(projectRoot, "src-tauri/target/debug/deps");
  const candidates = readdirSync(deps)
    .filter((name) => name.startsWith("stage75b_real-") && name.endsWith(".exe"))
    .map((name) => resolve(deps, name))
    .sort((left, right) => statSync(right).mtimeMs - statSync(left).mtimeMs);
  if (!candidates[0]) {
    throw new Error("未找到已编译的7.5-B夹具二进制，请在构建验收应用前执行测试编译");
  }
  return candidates[0];
}

const stage75bTestBinary = stage75b ? prepareStage75bTestBinary() : undefined;

const app = spawn(binary, [], {
  cwd: projectRoot,
  env: { ...process.env, TAURI_WEBDRIVER_PORT: String(webdriverPort) },
  stdio: "ignore",
  windowsHide: false
});

let browser;
let deletedThroughUi = false;

async function connect() {
  let lastError;
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      return await remote({
        hostname: "127.0.0.1",
        port: webdriverPort,
        logLevel: "silent",
        capabilities: {}
      });
    } catch (error) {
      lastError = error;
      await delay(500);
    }
  }
  throw lastError;
}

async function testElement(testId) {
  const root = await browser.$(`[data-testid="${testId}"]`);
  await root.waitForExist({ timeout: 20_000 });
  return root;
}

async function inputElement(testId) {
  const root = await testElement(testId);
  const tag = await root.getTagName();
  if (tag === "input" || tag === "textarea") return root;
  const input = await root.$("input,textarea");
  await input.waitForExist({ timeout: 10_000 });
  return input;
}

async function setInput(testId, value) {
  await inputElement(testId);
  const updated = await browser.execute((id, nextValue) => {
    const root = globalThis.document.querySelector(`[data-testid="${id}"]`);
    const input = root?.matches("input,textarea") ? root : root?.querySelector("input,textarea");
    if (!(input instanceof globalThis.HTMLInputElement) && !(input instanceof globalThis.HTMLTextAreaElement)) {
      return false;
    }
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

async function clickTest(testId) {
  const element = await testElement(testId);
  await element.waitForDisplayed({ timeout: 20_000 });
  const clicked = await browser.execute((id) => {
    const target = globalThis.document.querySelector(`[data-testid="${id}"]`);
    if (!(target instanceof globalThis.HTMLElement)) return false;
    target.click();
    return true;
  }, testId);
  if (!clicked) throw new Error(`无法点击界面元素：${testId}`);
}

async function clickButtonText(text) {
  const clicked = await browser.execute((label) => {
    const visible = (element) => {
      const style = globalThis.getComputedStyle(element);
      return style.display !== "none" && style.visibility !== "hidden" && element.getClientRects().length > 0;
    };
    const normalized = label.replace(/\s+/g, "");
    const buttons = [...globalThis.document.querySelectorAll("button")].filter(visible);
    const button = buttons.find((item) => item.textContent?.replace(/\s+/g, "") === normalized)
      ?? buttons.find((item) => item.textContent?.replace(/\s+/g, "").includes(normalized));
    if (!button) return false;
    button.click();
    return true;
  }, text);
  if (!clicked) throw new Error(`找不到可见按钮：${text}`);
}

async function clickLabelText(text) {
  const clicked = await browser.execute((label) => {
    const visible = (element) => {
      const style = globalThis.getComputedStyle(element);
      return style.display !== "none"
        && style.visibility !== "hidden"
        && element.getClientRects().length > 0;
    };
    const target = [...globalThis.document.querySelectorAll("label")]
      .filter(visible)
      .find((item) => item.textContent?.includes(label));
    if (!(target instanceof globalThis.HTMLElement)) return false;
    const control = target.querySelector('input[type="checkbox"],.n-checkbox');
    if (!(control instanceof globalThis.HTMLElement)) return false;
    control.click();
    return true;
  }, text);
  if (!clicked) throw new Error("找不到可见选项：" + text);
}

async function clickSelector(selector) {
  const clicked = await browser.execute((value) => {
    const target = globalThis.document.querySelector(value);
    if (!(target instanceof globalThis.HTMLElement)) return false;
    target.click();
    return true;
  }, selector);
  if (!clicked) throw new Error(`无法点击界面选择器：${selector}`);
}

async function clickRoleText(role, text) {
  const clicked = await browser.execute((targetRole, label) => {
    const normalized = label.replace(/\s+/g, "");
    const target = [...globalThis.document.querySelectorAll(`[role="${targetRole}"],.n-tabs-tab`)]
      .find((item) => item.textContent?.replace(/\s+/g, "") === normalized);
    if (!(target instanceof globalThis.HTMLElement)) return false;
    target.click();
    return true;
  }, role, text);
  if (!clicked) throw new Error(`找不到界面角色：${role}/${text}`);
}

async function waitBody(text, timeout = 30_000) {
  await browser.waitUntil(async () => (await browser.$("body").getText()).includes(text), {
    timeout,
    timeoutMsg: `界面未出现预期文本：${text}`
  });
}

async function waitAnyBody(texts, timeout = 30_000) {
  let matched = "";
  await browser.waitUntil(async () => {
    const body = await browser.$("body").getText();
    matched = texts.find((text) => body.includes(text)) ?? "";
    return Boolean(matched);
  }, { timeout, timeoutMsg: `界面未出现任一预期文本：${texts.join(" / ")}` });
  return matched;
}

function seedStage75bNodes() {
  if (!stage75bTestBinary) throw new Error("7.5-B夹具二进制未准备");
  const seeded = spawnSync(
    stage75bTestBinary,
    ["seed_stage75b_ui_nodes", "--ignored", "--nocapture"],
    {
      cwd: projectRoot,
      env: { ...process.env, INX_STAGE75_SCHEMA: isolatedSchema },
      encoding: "utf8",
      windowsHide: true
    }
  );
  if (seeded.status !== 0) {
    throw new Error(
      "隔离Schema预置真实节点失败：" + redact((seeded.stderr ?? "") + (seeded.stdout ?? ""))
    );
  }
}

async function runStage75bGate() {
  seedStage75bNodes();
  await clickSelector('a[href="#/aio/operations"]');
  await waitBody("部署升级", 20_000);
  await waitBody("stage75b-node-79", 30_000);
  await waitBody("stage75b-node-121", 30_000);
  await clickTest("operation-mode-service");
  await clickButtonText("清空");
  await clickLabelText("stage75b-node-79");
  await clickLabelText("stage75b-node-121");
  await waitBody("已选 2 台", 10_000);
  const imagePath = resolve(
    projectRoot,
    "test/images/device-edge-1.0.0.Alpha.20260819.tar"
  );
  await setInput("operation-artifact-path", imagePath);
  await clickTest("operation-preflight");
  await waitBody("全部通过", 90_000);
  await clickTest("operation-submit");
  await browser.waitUntil(async () => {
    const value = await (await testElement("operation-task-id")).getText();
    return value.trim().length > 20;
  }, { timeout: 30_000, timeoutMsg: "界面未立即显示Task ID" });
  await clickSelector('button[aria-label="关闭"]');
  await testElement("application-exit-impact");
  await waitBody("确认安全关闭", 20_000);
  await clickButtonText("取消关闭");
  await waitBody("已形成最终结果", 360_000);
  let body = await browser.$("body").getText();
  if (!body.includes("成功 2 台，失败 0 台")) {
    throw new Error("真实成功结果统计不准确");
  }

  await clickButtonText("创建下一次任务");
  await clickTest("operation-mode-service");
  await clickButtonText("清空");
  await clickLabelText("stage75b-node-79");
  await clickLabelText("stage75b-node-121");
  await waitBody("已选 2 台", 10_000);
  await setInput("operation-artifact-path", imagePath);
  await clickTest("operation-preflight");
  await waitBody("全部通过", 90_000);
  await clickTest("operation-submit");
  const cancel = await testElement("operation-cancel");
  await cancel.waitForDisplayed({ timeout: 60_000 });
  await clickTest("operation-cancel");
  await waitBody("已形成最终结果", 360_000);
  body = await browser.$("body").getText();
  const cancelled = body.match(/取消\s+(\d+)\s+台/);
  if (!cancelled || Number(cancelled[1]) < 1) {
    throw new Error("界面取消结果未显示真实取消节点数");
  }
  await clickButtonText("查看操作记录");
  await waitBody("项目侧共享操作记录", 30_000);
  await waitBody("单服升级", 30_000);
  await clickButtonText("关闭");
}

try {
  browser = await connect();
  await browser.execute(() => {
    globalThis.__stage75aE2eErrors = [];
    globalThis.__stage75aCommandTrace = [];
    globalThis.__stage75aMessages = [];
    globalThis.addEventListener("error", (event) => globalThis.__stage75aE2eErrors.push(String(event.error ?? event.message)));
    globalThis.addEventListener("unhandledrejection", (event) => globalThis.__stage75aE2eErrors.push(String(event.reason)));
    const internals = globalThis.__TAURI_INTERNALS__;
    if (internals?.invoke && !internals.__stage75aWrapped) {
      const originalInvoke = internals.invoke.bind(internals);
      internals.invoke = async (command, args, options) => {
        globalThis.__stage75aCommandTrace.push(`${command}:started`);
        try {
          const result = await originalInvoke(command, args, options);
          globalThis.__stage75aCommandTrace.push(`${command}:succeeded`);
          return result;
        } catch (error) {
          globalThis.__stage75aCommandTrace.push(`${command}:failed:${String(error?.code ?? error?.message ?? error)}`);
          throw error;
        }
      };
      internals.__stage75aWrapped = true;
    }
    new globalThis.MutationObserver(() => {
      for (const message of globalThis.document.querySelectorAll(".n-message")) {
        const text = message.textContent?.replace(/\s+/g, " ").trim();
        if (text && !globalThis.__stage75aMessages.includes(text)) globalThis.__stage75aMessages.push(text);
      }
    }).observe(globalThis.document.body, { childList: true, subtree: true });
  });
  await waitBody("选择或新增项目");
  await clickTest("project-switcher");
  await waitBody("切换项目");
  await clickButtonText("新增");
  await delay(500);
  const projectDialogDiagnostic = await browser.execute(() => ({
    projectNameExists: Boolean(globalThis.document.querySelector('[data-testid="project-name"]')),
    visibleButtons: [...globalThis.document.querySelectorAll("button")]
      .filter((item) => item.getClientRects().length > 0)
      .map((item) => item.textContent?.replace(/\s+/g, "").slice(0, 80)),
    dialogs: [...globalThis.document.querySelectorAll('[role="dialog"],.n-modal,.n-card')]
      .filter((item) => item.getClientRects().length > 0)
      .map((item) => item.textContent?.replace(/\s+/g, "").slice(0, 300)),
    errors: globalThis.__stage75aE2eErrors
  }));
  if (!projectDialogDiagnostic.projectNameExists) {
    throw new Error(`新增项目弹窗未打开：${JSON.stringify(projectDialogDiagnostic)}`);
  }

  await setInput("project-name", "阶段" + stageLabel + "界面验收项目");
  await setInput("project-platform-url", `http://${line("平台API：")}`);
  await setInput("project-db-host", yamlDefault("MYSQL_HOST"));
  await setInput("project-db-port", yamlDefault("MYSQL_PORT"));
  await setInput("project-db-user", yamlDefault("MYSQL_USER"));
  await setInput("project-db-password", yamlDefault("MYSQL_PASSWORD"));
  await setInput("project-business-db", line("平台业务数据库名："));
  const workbench = await inputElement("project-workbench-db").catch(() => null);
  if (workbench && (await workbench.getValue()) !== isolatedSchema) {
    throw new Error("验收构建未使用指定隔离工作台Schema");
  }

  await clickTest("project-test-connection");
  await waitBody("双数据库连接和平台 Schema 探测通过", 30_000);
  await clickTest("project-save");
  const afterProjectSave = await waitAnyBody(["请显式初始化/升级工作台 Schema", "登录项目平台"], 30_000);
  if (afterProjectSave.includes("请显式")) {
    await clickTest("schema-upgrade");
    await clickButtonText("确认执行");
    await waitBody("登录项目平台", 30_000);
  }

  await setInput("login-username", login.principal);
  await setInput("login-password", login.credentials);
  await setInput("login-image-code", login.imageCode);
  await setInput("login-session-uuid", login.sessionUUID);
  await clickTest("login-submit");
  await waitBody(`平台已登录 · ${login.principal}`, 30_000);

  await clickSelector('a[href="#/aio/release"]');
  await waitBody("当前项目尚未创建发布参数", 20_000);
  await clickTest("release-edit");

  await setInput("release-platform-host", apiHost);
  await setInput("release-api-port", apiPort);
  await setInput("release-auth-key", line("平台API auth Key："));
  await setInput("release-mqtt-host", mqttHost);
  await setInput("release-mqtt-port", mqttPort);
  await setInput("release-mqtt-user", line("平台mqtt账号："));
  await setInput("release-mqtt-password", line("平台mqtt密码："));
  await setInput("release-aio-mqtt-user", "stage75-ui-aio");
  await setInput("release-aio-mqtt-password", "stage75-ui-aio-password");
  await setInput("release-ssh-user", line("一体机ssh用户："));
  await setInput("release-ssh-port", "22");
  await setInput("release-ssh-private-key", privateKey);
  if (stage75b) {
    await setInput("release-data-root", "/opt/data");
    await setInput("release-deploy-root", "/opt/data/deploy/inxvision-edge");
  }
  await setInput("release-env-template", envTemplate);
  await clickRoleText("tab", "docker-compose.yml");
  await setInput("release-compose-template", composeTemplate);
  await clickTest("release-save");
  await waitBody("版本 1", 30_000);

  await clickTest("release-edit");
  await setInput("release-ssh-timeout", "16");
  await clickTest("release-save");
  await waitBody("版本 2", 30_000);

  await clickTest("host-key-open");
  for (const host of ["192.168.3.79", "192.168.3.121"]) {
    await setInput("host-key-host", host);
    await clickTest("host-key-capture");
    await waitBody("首次连接，等待确认", 30_000);
    await clickTest("host-key-confirm");
    await waitBody(`${host}:22`, 30_000);
  }
  await clickButtonText("关闭");
  if (stage75b) await runStage75bGate();
  await clickTest("project-switcher");
  await clickButtonText("编辑当前项目");
  await clickButtonText("删除项目");
  await clickButtonText("仅删除本地入口");
  await waitBody("选择或新增项目", 30_000);
  deletedThroughUi = true;

  process.stdout.write(
    (stage75b ? "STAGE75B_UI_GATE_PASS" : "STAGE75A_UI_GATE_PASS")
      + "|schema="
      + isolatedSchema
      + "\n"
  );
} catch (error) {
  let bodyText = "";
  let commandTrace = [];
  let messages = [];
  if (browser) {
    try { bodyText = await browser.$("body").getText(); } catch { /* 窗口退出时没有正文 */ }
    try {
      ({ commandTrace, messages } = await browser.execute(() => ({
        commandTrace: globalThis.__stage75aCommandTrace ?? [],
        messages: globalThis.__stage75aMessages ?? []
      })));
    } catch { /* 窗口退出时没有诊断 */ }
  }
  process.stderr.write(
    `STAGE75A_UI_GATE_FAIL|${redact(error?.message ?? error)}|commands=${redact(JSON.stringify(commandTrace))}|messages=${redact(JSON.stringify(messages))}|body=${redact(bodyText).slice(0, 2000)}\n`
  );
  process.exitCode = 1;
} finally {
  if (browser) {
    if (!deletedThroughUi) {
      try {
        const projectId = await browser.execute(() => globalThis.localStorage.getItem("inx.workbench.active-project"));
        if (projectId) {
          await browser.executeAsync((id, done) => {
            globalThis.__TAURI_INTERNALS__.invoke("delete_local_project", { projectId: id })
              .then(() => done(true))
              .catch(() => done(false));
          }, projectId);
        }
      } catch {
        // 外部清理门禁会复核本地SQLite；这里仅尽力清理本机凭据引用。
      }
    }
    try { await browser.deleteSession(); } catch { /* 已由应用退出关闭 */ }
  }
  if (!app.killed) app.kill();
}
