import { spawn,spawnSync } from "node:child_process";
import { readFile,readdir,stat,mkdir,writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { remote } from "webdriverio";
const root=process.cwd(),port=4447,suffix=randomUUID().replaceAll("-","");
const biz="inxaiot_desk_buddy_ui_b_"+suffix,ops="inxaiot_desk_buddy_ui_w_"+suffix;
const evidence=resolve(root,".review-tools","smart-screen-step4-"+Date.now());await mkdir(evidence,{recursive:true});
const description=await readFile(resolve(root,"test/测试数据说明.txt"),"utf8");
const line=label=>description.split(/\r?\n/).map(s=>s.trim().replace(/^\uFEFF/,"")).find(s=>s.startsWith(label)).slice(label.length).trim();
const login=JSON.parse(description.slice(description.indexOf("{"),description.indexOf("}",description.indexOf("{"))+1));
const [dbUser,...passwordParts]=line("平台数据库账号密码：").split("/");const dbPassword=passwordParts.join("/");
const redact=text=>[dbPassword,login.credentials].reduce((value,secret)=>secret?value.split(secret).join("***"):value,String(text));
const deps=resolve(root,"src-tauri/target/debug/deps");
const binaries=await Promise.all((await readdir(deps)).filter(n=>/^smart_screen_fixture_real-.*\.exe$/.test(n)).map(async name=>({path:resolve(deps,name),mtime:(await stat(resolve(deps,name))).mtimeMs})));
binaries.sort((a,b)=>b.mtime-a.mtime);if(!binaries[0])throw new Error("请先构建智能屏桌面测试数据程序");
function fixture(action){
  const output=spawnSync(binaries[0].path,["--exact","desktop_fixture","--ignored","--nocapture"],{cwd:resolve(root,"src-tauri"),env:{...process.env,INX_SCREEN_FIXTURE_ACTION:action,INX_SCREEN_FIXTURE_BIZ:biz,INX_SCREEN_FIXTURE_OPS:ops},encoding:"utf8",windowsHide:true});
  if(output.status!==0)throw new Error("测试库 "+action+" 失败："+redact(output.stdout+output.stderr));
}
let browser,app,projectId,failure,prepared=false;
async function command(name,args={}){
  const reply=await browser.executeAsync((method,data,done)=>{globalThis.__TAURI_INTERNALS__.invoke(method,data).then(value=>done({ok:true,value})).catch(error=>done({ok:false,message:error?.params?.summary||error?.message||error?.code||"操作失败"}));},name,args);
  if(!reply.ok)throw new Error(name+"："+reply.message);return reply.value;
}
async function click(selector){assert.ok(await browser.execute(query=>{const el=document.querySelector(query);if(!el||el.disabled)return false;el.click();return true;},selector),"找不到控件："+selector);}
async function button(text){assert.ok(await browser.execute(label=>{const el=[...document.querySelectorAll("button")].find(e=>e.textContent.trim()===label&&e.getClientRects().length);if(!el||el.disabled)return false;el.click();return true;},text),"找不到按钮："+text);}
async function waitText(text){await browser.waitUntil(async()=>(await browser.$("body").getText()).includes(text),{timeout:30000,timeoutMsg:"页面未显示："+text});}
async function chooseOperation(label){
  await click('[aria-label="选择智能屏操作"] .n-base-selection-label');
  await browser.waitUntil(async()=>browser.execute(wanted=>[...document.querySelectorAll(".n-base-select-option")].some(e=>e.textContent.trim()===wanted),label),{timeout:10000});
  assert.ok(await browser.execute(wanted=>{const option=[...document.querySelectorAll(".n-base-select-option")].find(e=>e.textContent.trim()===wanted);option?.click();return Boolean(option);},label));
}
async function registerCurrent(){
  await click('[data-testid="screen-operation-first-action"]');
  await waitText("请核对逐台差异，确认后更新平台资料");
  assert.ok(!(await browser.$("body").getText()).includes("当前为交互原型"));
  const previous=new Set((await command("screen_load",{localProjectId:projectId,refresh:false})).tasks.map(task=>task.id));
  await click('[data-testid="screen-operation-submit"]');
  await browser.waitUntil(async()=>{const data=await command("screen_load",{localProjectId:projectId,refresh:false});return data.tasks.find(t=>t.action==="register"&&!previous.has(t.id))?.state==="succeeded";},{timeout:90000});
}
async function fillName(value){
  assert.ok(await browser.execute(text=>{const item=[...document.querySelectorAll(".n-form-item")].find(e=>e.querySelector(".n-form-item-label")?.textContent.trim()==="屏名称");const input=item?.querySelector("input");if(!input)return false;Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,"value").set.call(input,text);input.dispatchEvent(new Event("input",{bubbles:true}));return true;},value));
}
try{
  fixture("prepare");prepared=true;
  app=spawn(resolve(root,"src-tauri/target/debug/inxaiot-desk-buddy.exe"),[],{cwd:root,env:{...process.env,TAURI_WEBDRIVER_PORT:String(port)},stdio:"ignore",windowsHide:true});
  for(let n=0;n<60;n++){try{browser=await remote({hostname:"127.0.0.1",port,logLevel:"silent",capabilities:{}});break;}catch{await delay(500);}}
  if(!browser)throw new Error("验收桌面未启动");await browser.setTimeout({script:90000});
  const project=await command("create_local_project",{input:{name:"智能屏步骤4登记验收-"+Date.now(),platformUrl:"http://"+line("平台API："),dbHost:line("平台主机："),dbPort:Number(process.env.INX_TEST_MYSQL_PORT||"3306"),dbUser,dbTlsEnabled:false,dbPassword,businessDb:biz,workbenchDb:ops}});projectId=project.id;
  await command("login_project",{projectId,request:{username:login.principal,password:login.credentials,sessionUuid:login.sessionUUID,imageCode:login.imageCode}});
  await command("screen_load",{localProjectId:projectId,refresh:true});
  const input={name:"4寸登记验收",ip:line("4寸屏：").split(":")[0],mac:"",size:"4",spaceId:"1001",location:"正式登记测试"};
  const localId=await command("screen_save_local",{localProjectId:projectId,fields:input,id:null,expectedRevision:null});
  await browser.execute(id=>{localStorage.setItem("inx.workbench.active-project",id);location.hash="/screen/nodes";},projectId);await browser.refresh();await waitText(input.name);
  await click('[aria-label="选择当前页"]');await button("批量操作");await chooseOperation("注册/更新到平台");
  await registerCurrent();await waitText("平台注册与本机关联已完成");
  const registered=await command("screen_load",{localProjectId:projectId,refresh:true});assert.equal(registered.screens.length,1);const pid=registered.screens[0].id;assert.ok(registered.screens[0].aliases.includes(localId));
  await browser.saveScreenshot(resolve(evidence,"01-registration.png"));
  await button("返回智能屏列表");await button("详情");await button("编辑资料");await fillName("通过桌面修改名称");await button("保存待提交修改");await waitText("资料有变更");await button("关闭");
  await click(".screen-draft-link");await registerCurrent();await waitText("平台资料已更新");
  await button("返回智能屏列表");await waitText("通过桌面修改名称");
  const another=await command("screen_save_local",{localProjectId:projectId,fields:{...input,name:"合并后的名称",mac:registered.screens[0].mac},id:null,expectedRevision:null});
  await button("刷新平台");await browser.waitUntil(async()=>browser.execute(()=>Boolean(document.querySelector(".screen-duplicate-link"))),{timeout:10000});await click(".screen-duplicate-link");
  await click('[data-merge-field="name"] .n-radio input[value="local"]');
  await click('[data-testid="merge-submit"]');await waitText("合并成功");
  await browser.saveScreenshot(resolve(evidence,"02-merge.png"));
  const merged=await command("screen_load",{localProjectId:projectId,refresh:true});assert.equal(merged.screens.length,1);assert.equal(merged.screens[0].id,pid);assert.ok(merged.screens[0].aliases.includes(another));assert.equal(merged.screens[0].name,"合并后的名称");
  await writeFile(resolve(evidence,"result.json"),JSON.stringify({passed:true,date:new Date().toISOString(),checks:["正式页面自动读取MAC","确认注册","单条身份及本机别名","真实编辑草稿","确认资料更新","逐字段选源合并","平台编号保留"],businessSchema:biz,workbenchSchema:ops},null,2),"utf8");
}catch(error){failure=error;if(browser)await browser.saveScreenshot(resolve(evidence,"failure.png")).catch(()=>{});}
finally{
  if(browser){if(projectId)try{await command("delete_local_project",{projectId});}catch(error){failure??=error;}await browser.deleteSession().catch(()=>{});}
  if(app&&app.exitCode===null)app.kill();
  if(prepared)try{fixture("cleanup");}catch(error){failure??=error;}
}
if(failure){console.error("智能屏登记桌面验收失败："+redact(failure instanceof Error?failure.message:failure));process.exitCode=1;}
else console.log("SCREEN_STEP4_UI_PASS "+evidence);
