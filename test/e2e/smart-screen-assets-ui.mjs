import { spawn } from "node:child_process";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { remote } from "webdriverio";

const root = process.cwd();
const evidence = resolve(root, ".review-tools", "smart-screen-step2-" + Date.now());
await mkdir(evidence, { recursive: true });
const port = 4447;
const binary = resolve(root, "src-tauri/target/debug/inxaiot-desk-buddy.exe");
function startApp() { return spawn(binary, [], { cwd: root, env: { ...process.env, TAURI_WEBDRIVER_PORT: String(port) }, stdio: "ignore", windowsHide: true }); }
let app = startApp();
const projectIds = [];
let browser;
async function connect() {
  for (let n = 0; n < 60; n++) {
    try { return await remote({ hostname: "127.0.0.1", port, logLevel: "silent", capabilities: {} }); }
    catch { await delay(500); }
  }
  throw new Error("智能屏验收桌面未就绪");
}
async function command(name, args = {}) {
  const reply = await browser.executeAsync((commandName, data, done) => {
    globalThis.__TAURI_INTERNALS__.invoke(commandName, data)
      .then(value => done({ ok: true, value }))
      .catch(error => done({ ok: false, message: error?.params?.summary || error?.message || error?.code || "命令失败" }));
  }, name, args);
  if (!reply.ok) throw new Error(name + "：" + reply.message);
  return reply.value;
}
async function waitText(text) {
  await browser.waitUntil(async () => (await browser.$("body").getText()).includes(text), { timeout: 30_000, timeoutMsg: "页面未显示：" + text });
}
async function selectProject(id) {
  await browser.execute((project) => { localStorage.setItem("inx.workbench.active-project", project); location.hash = "/screen/nodes"; }, id);
  await browser.refresh();
  await waitText("智能屏列表");
  await browser.waitUntil(async () => (await command("list_local_projects")).some(p => p.id === id), { timeout: 10_000 });
}
async function clickText(text) {
  const ok = await browser.execute((label) => {
    const element = [...document.querySelectorAll("button")].find(e => e.textContent.trim() === label && e.getClientRects().length);
    if (!element || element.disabled) return false;
    element.click(); return true;
  }, text);
  assert.ok(ok, "找不到可用按钮：" + text);
}
async function fill(label, value) {
  const ok = await browser.execute((wanted, text) => {
    const item = [...document.querySelectorAll(".n-form-item")].find(e => e.querySelector(".n-form-item-label")?.textContent.trim().startsWith(wanted));
    const input = item?.querySelector("input");
    if (!input) return false;
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, text);
    input.dispatchEvent(new Event("input", { bubbles: true })); input.dispatchEvent(new Event("change", { bubbles: true }));
    return true;
  }, label, value);
  assert.ok(ok, "找不到表单：" + label);
}
let failure;
try {
  browser = await connect();
  await browser.setTimeout({ script: 60000 });
  const local = await command("create_local_project", { input: { name: "智能屏步骤2本机验收-" + Date.now(), platformUrl: "", dbHost: "", dbPort: 3306, dbUser: "", dbTlsEnabled: false, dbPassword: null, businessDb: "", workbenchDb: "inxaiot_desk_buddy" } });
  projectIds.push(local.id);
  await selectProject(local.id);
  await clickText("新增智能屏");
  await fill("屏名称", "桌面新增测试屏");
  await fill("设备 IP", "192.0.2.114");
  await fill("安装位置", "步骤2桌面验收");
  await clickText("加入本机管理");
  await waitText("桌面新增测试屏");
  let current = await command("screen_load", { localProjectId: local.id, refresh: false });
  assert.equal(current.screens.length, 1);
  assert.equal(current.screens[0].source, "local");
  assert.equal(current.screens[0].appVersion, null);
  assert.equal(current.platformAvailable, false);
  const originalId = current.screens[0].id;
  await browser.refresh();
  await waitText("桌面新增测试屏");
  current = await command("screen_load", { localProjectId: local.id, refresh: false });
  assert.equal(current.screens[0].id, originalId);
  await browser.saveScreenshot(resolve(evidence, "01-local-assets.png"));
  await browser.deleteSession();
  if (app.exitCode === null) app.kill();
  await new Promise(resolveExit => { if (app.exitCode !== null) resolveExit(); else app.once("exit", resolveExit); });
  app = startApp(); browser = await connect();
  await browser.setTimeout({ script: 60000 });
  await selectProject(local.id);
  await waitText("桌面新增测试屏");
  assert.equal((await command("screen_load", { localProjectId: local.id, refresh: false })).screens[0].id, originalId);

  const text = await readFile(resolve(root, "test/测试数据说明.txt"), "utf8");
  const line = label => text.split(/\r?\n/).map(s => s.trim().replace(/^\uFEFF/, "")).find(s => s.startsWith(label))?.slice(label.length).trim();
  const [user, ...parts] = line("平台数据库账号密码：").split("/");
  const login = JSON.parse(text.slice(text.indexOf("{"), text.indexOf("}", text.indexOf("{")) + 1));
  const project = await command("create_local_project", { input: {
    name: "智能屏步骤2平台验收-" + Date.now(), platformUrl: "http://" + line("平台API："), dbHost: line("平台主机："), dbPort: Number(process.env.INX_TEST_MYSQL_PORT || "3306"), dbUser: user, dbTlsEnabled: false, dbPassword: parts.join("/"), businessDb: "inxvision_iot_dev_demo", workbenchDb: "inxaiot_desk_buddy_screen_readonly"
  } });
  projectIds.push(project.id);
  await command("login_project", { projectId: project.id, request: { username: login.principal, password: login.credentials, sessionUuid: login.sessionUUID, imageCode: login.imageCode } });
  await selectProject(project.id);
  await browser.waitUntil(async () => {
    const data = await command("screen_load", { localProjectId: project.id, refresh: true });
    return data.platformAvailable && data.screens.length > 0;
  }, { timeout: 30_000 });
  await clickText("刷新平台");
  const data = await command("screen_load", { localProjectId: project.id, refresh: true });
  await waitText(data.screens[0].name);
  assert.ok(data.spaces.length > 0);
  assert.ok(!data.screens.some(s => s.id === originalId), "项目切换不能混入本机屏");
  const screen = data.screens[0];
  await command("screen_save_draft", { localProjectId: project.id, id: screen.id, fields: { name: screen.name, ip: screen.ip, mac: screen.mac, size: screen.size, spaceId: screen.spaceId, location: "步骤2未提交草稿" }, expectedRevision: 0, expectedAssetRevision: screen.revision });
  await clickText("刷新平台");
  await waitText("资料有变更");
  await browser.saveScreenshot(resolve(evidence, "02-platform-assets-draft.png"));
  await browser.refresh();
  await waitText("资料有变更");
  const saved = await command("screen_load", { localProjectId: project.id, refresh: true });
  assert.equal(saved.platformDrafts[screen.id].values.location, "步骤2未提交草稿");
  assert.equal(saved.screens.find(s => s.id === screen.id).location, screen.location);
  await writeFile(resolve(evidence, "result.json"), JSON.stringify({ passed: true, date: new Date().toISOString(), platformScreens: data.screens.length, spaces: data.spaces.length, checks: ["本机新增表单", "页面重载保留", "程序关闭重开保留", "平台真实读取", "项目隔离", "草稿展示", "草稿重载", "平台资料未修改"] }, null, 2), "utf8");
  console.log("SCREEN_STEP2_UI_PASS " + evidence);
} catch (error) {
  failure = error;
  if (browser) await browser.saveScreenshot(resolve(evidence, "failure.png")).catch(() => {});
} finally {
  if (browser) {
    for (const projectId of projectIds.reverse()) {
      try { await command("delete_local_project", { projectId }); }
      catch { console.error("测试项目清理未完成：" + projectId); }
    }
    await browser.deleteSession().catch(() => {});
  }
  if (app.exitCode === null) app.kill();
}
if (failure) {
  console.error("智能屏桌面验收失败：" + (failure instanceof Error ? failure.message : "未知错误"));
  process.exitCode = 1;
}
