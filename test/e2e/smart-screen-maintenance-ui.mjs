import { spawn } from "node:child_process";
import { readFile, mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { remote } from "webdriverio";

const root=process.cwd(), port=4448;
const evidence=resolve(root,".review-tools","smart-screen-step6-"+Date.now());
await mkdir(evidence,{recursive:true});
const app=spawn(process.env.INX_SCREEN_E2E_APP_PATH||resolve(root,"src-tauri/target/debug/inxaiot-desk-buddy.exe"),[],{cwd:root,env:{...process.env,TAURI_WEBDRIVER_PORT:String(port)},stdio:"ignore",windowsHide:true});
let browser,projectId,failure;
async function command(name,args={}){
  const reply=await browser.executeAsync((method,data,done)=>{
    globalThis.__TAURI_INTERNALS__.invoke(method,data).then(value=>done({ok:true,value})).catch(error=>done({ok:false,message:error?.params?.summary||error?.message||error?.code||"命令失败"}));
  },name,args);
  if(!reply.ok)throw new Error(name+"："+reply.message);return reply.value;
}
async function click(selector){
  assert.ok(await browser.execute((query)=>{const element=document.querySelector(query);if(!element||element.disabled)return false;element.click();return true;},selector),"找不到可用控件："+selector);
}
async function button(text){
  assert.ok(await browser.execute((label)=>{const element=[...document.querySelectorAll("button")].find(el=>el.textContent.trim()===label&&el.getClientRects().length);if(!element||element.disabled)return false;element.click();return true;},text),"找不到按钮："+text);
}
try{
  for(let n=0;n<60;n++){try{browser=await remote({hostname:"127.0.0.1",port,logLevel:"silent",capabilities:{}});break;}catch{await delay(500);}}
  if(!browser)throw new Error("验收桌面未就绪");
  await browser.setTimeout({script:60000});
  const project=await command("create_local_project",{input:{name:"智能屏步骤6诊断验收-"+Date.now(),platformUrl:"",dbHost:"",dbPort:3306,dbUser:"",dbTlsEnabled:false,dbPassword:null,businessDb:"",workbenchDb:"inxaiot_desk_buddy"}});
  projectId=project.id;
  const description=await readFile(resolve(root,"test/测试数据说明.txt"),"utf8");
  const address=label=>description.split(/\r?\n/).map(line=>line.trim()).find(line=>line.startsWith(label)).slice(label.length).trim().split(":")[0];
  for(const [size,ip] of [["4",address("4寸屏：")],["10",address("10寸屏：")]]){
    await command("screen_save_local",{localProjectId:projectId,fields:{name:size+" 寸实机",ip,mac:"",size,spaceId:null,location:"诊断采集"},id:null,expectedRevision:null});
  }
  await browser.execute(id=>{localStorage.setItem("inx.workbench.active-project",id);location.hash="/screen/nodes";},projectId);
  await browser.refresh();
  await browser.waitUntil(async()=> (await browser.$("body").getText()).includes("4 寸实机"),{timeout:20000});
  await click('[aria-label="选择当前页"]');
  await button("批量操作");
  await click('[aria-label="选择智能屏操作"] .n-base-selection-label');
  await browser.waitUntil(async()=>browser.execute(()=>[...document.querySelectorAll(".n-base-select-option")].some(el=>el.textContent.trim()==="采集诊断")),{timeout:10000});
  assert.ok(await browser.execute(()=>{const option=[...document.querySelectorAll(".n-base-select-option")].find(el=>el.textContent.trim()==="采集诊断");option?.click();return Boolean(option);}));
  await click('[data-testid="screen-operation-first-action"]');
  let task;
  await browser.waitUntil(async()=>{
    const data=await command("screen_load",{localProjectId:projectId,refresh:false});
    task=data.tasks.find(t=>t.action==="diagnostics");
    return task&& !["running","cancelling"].includes(task.state);
  },{timeout:90000,interval:250});
  assert.equal(task.state,"succeeded");assert.equal(task.targets.length,2);
  assert.ok(task.targets.every(t=>t.state==="succeeded"));
  await browser.waitUntil(async()=> (await browser.$("body").getText()).includes("诊断文件已保存在来源电脑"),{timeout:10000});
  assert.ok(!(await browser.$("body").getText()).includes("原型模拟"),"正式结果不能标成原型模拟");
  await button("查看诊断文件");
  await browser.waitUntil(async()=> (await browser.$("body").getText()).includes("智能屏诊断报告"),{timeout:10000});
  const report=await command("screen_read_diagnostics",{localProjectId:projectId,taskId:task.id});
  assert.equal(JSON.parse(report).screens.length,2);
  const exported=await command("screen_export_diagnostics",{localProjectId:projectId,taskId:task.id,directory:evidence});
  assert.equal(await readFile(exported,"utf8"),report);
  await delay(500);
  await browser.saveScreenshot(resolve(evidence,"02-diagnostic-preview.png"));
  await browser.execute(()=>document.querySelector(".n-card-header__close")?.click());
  await browser.waitUntil(async()=>browser.execute(()=>![...document.querySelectorAll('.n-modal')].some(el=>el.getClientRects().length)),{timeout:5000});
  await button("本次日志");
  await browser.saveScreenshot(resolve(evidence,"01-inspection-results.png"));
  await button("任务与日志面板");
  await browser.waitUntil(async()=> (await browser.$("body").getText()).includes("开始设备检查"),{timeout:10000});
  await browser.saveScreenshot(resolve(evidence,"02-shared-task-panel.png"));
  const snapshot=await command("screen_load",{localProjectId:projectId,refresh:false});
  assert.equal(Object.keys(snapshot.observations).length,2);
  assert.ok(snapshot.screens.every(s=>s.appVersion===null));
  await writeFile(resolve(evidence,"result.json"),JSON.stringify({passed:true,date:new Date().toISOString(),taskId:task.id,targets:task.targets.map(t=>({name:t.name,state:t.state})),checks:["正式页面发起诊断","真实任务队列","两块屏逐台结果","诊断报告可查看","导出文件与查看内容一致","界面动画结束后截图","统一任务和日志","本机实测与平台登记分开"]},null,2),"utf8");
}catch(error){failure=error;if(browser)await browser.saveScreenshot(resolve(evidence,"failure.png")).catch(()=>{});}
finally{
  if(browser){
    if(projectId){try{await command("delete_local_project",{projectId});}catch(error){failure??=error;}}
    await browser.deleteSession().catch(()=>{});
  }
  if(app.exitCode===null)app.kill();
}
if(failure){console.error("智能屏诊断桌面验收失败："+(failure instanceof Error?failure.message:"未知错误"));process.exitCode=1;}
else console.log("SCREEN_STEP6_UI_PASS "+evidence);
