import { spawn, spawnSync } from "node:child_process";
import { readFile, readdir, stat, mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import assert from "node:assert/strict";
import { remote } from "webdriverio";

const root=process.cwd(),port=4459,suffix=randomUUID().replaceAll("-","");
const business="inxaiot_desk_buddy_ui_b_"+suffix,shared="inxaiot_desk_buddy_ui_w_"+suffix;
const evidence=resolve(root,".review-tools","smart-screen-step7-20261003","desktop-"+Date.now());await mkdir(evidence,{recursive:true});
const executable=process.env.INX_SCREEN_E2E_APP_PATH||resolve(root,"src-tauri/target/debug/inxaiot-desk-buddy.exe");
const description=await readFile(resolve(root,"test/测试数据说明.txt"),"utf8");
const line=label=>description.split(/\r?\n/).map(s=>s.trim().replace(/^\uFEFF/,"")).find(s=>s.startsWith(label)).slice(label.length).trim();
const login=JSON.parse(description.slice(description.indexOf("{"),description.indexOf("}",description.indexOf("{"))+1));
const [dbUser,...passwordParts]=line("平台数据库账号密码：").split("/");const dbPassword=passwordParts.join("/");
const redact=text=>[dbPassword,login.credentials].reduce((value,secret)=>secret?value.split(secret).join("***"):value,String(text));
const deps=resolve(root,"src-tauri/target/debug/deps");
const fixtures=await Promise.all((await readdir(deps)).filter(n=>/^smart_screen_fixture_real-.*\.exe$/.test(n)).map(async name=>({path:resolve(deps,name),mtime:(await stat(resolve(deps,name))).mtimeMs})));
fixtures.sort((a,b)=>b.mtime-a.mtime);assert.ok(fixtures[0]);
function fixture(action){const output=spawnSync(fixtures[0].path,["--exact","desktop_fixture","--ignored","--nocapture"],{cwd:resolve(root,"src-tauri"),env:{...process.env,INX_SCREEN_FIXTURE_ACTION:action,INX_SCREEN_FIXTURE_BIZ:business,INX_SCREEN_FIXTURE_OPS:shared},encoding:"utf8",windowsHide:true});if(output.status!==0)throw Error("测试库操作失败："+redact(output.stdout+output.stderr));}
function database(action){const output=spawnSync("python",[resolve(root,"test/e2e/screen-step7-db.py")],{cwd:root,input:JSON.stringify({business,shared,action}),encoding:"utf8",windowsHide:true});if(output.status!==0)throw Error("隔离数据库检查失败："+redact(output.stderr));return JSON.parse(output.stdout);}
const childEnv={...process.env,TAURI_WEBDRIVER_PORT:String(port)};
for(const key of Object.keys(childEnv))if(["JAVA_HOME","ANDROID_HOME","ANDROID_SDK_ROOT","INX_ADB_PATH","INX_AAPT_PATH","INX_APKSIGNER_JAR","PATH"].includes(key.toUpperCase()))delete childEnv[key];
childEnv.PATH=resolve(process.env.SystemRoot||process.env.SYSTEMROOT||"C:/Windows","System32");
let browser,app,projectId,localProjectId,prepared=false,failure,activeTask,sharedFault=false;
const checks=[],tasks=[];
async function start(){app=spawn(executable,[],{cwd:root,env:childEnv,stdio:"ignore",windowsHide:true});for(let n=0;n<60;n++){try{browser=await remote({hostname:"127.0.0.1",port,logLevel:"silent",capabilities:{}});break;}catch{await delay(500);}}if(!browser)throw Error("桌面未启动");await browser.setTimeout({script:120000});}
async function command(name,args={}){const reply=await browser.executeAsync((method,data,done)=>globalThis.__TAURI_INTERNALS__.invoke(method,data).then(value=>done({ok:true,value})).catch(error=>done({ok:false,message:error?.params?.summary||error?.message||error?.code})),name,args);if(!reply.ok)throw Error(name+"："+reply.message);return reply.value;}
async function button(label){assert.ok(await browser.execute(text=>{const el=[...document.querySelectorAll("button")].find(e=>e.getClientRects().length&&e.textContent.trim()===text&&!e.disabled);el?.click();return Boolean(el);},label),"未找到按钮："+label);}
async function navigate(label){await browser.waitUntil(async()=>browser.execute(text=>[...document.querySelectorAll("a")].some(el=>el.textContent.trim()===text),label),{timeout:20000});await browser.execute(text=>[...document.querySelectorAll("a")].find(el=>el.textContent.trim()===text).click(),label);await delay(350);}
async function selectProject(id){await browser.execute(value=>localStorage.setItem("inx.workbench.active-project",value),id);await browser.refresh();await navigate("智能屏列表");}
async function screenshot(name){await delay(500);await browser.saveScreenshot(resolve(evidence,name+".png"));}
async function load(refresh=false){return command("screen_load",{localProjectId:projectId,refresh});}
async function waitTask(id,expected="succeeded"){activeTask=id;let final;const samples=[];await browser.waitUntil(async()=>{const current=(await load()).tasks.find(t=>t.id===id);if(!current)return false;const sample={state:current.state,targets:current.targets.map(t=>({progress:t.progress,message:t.message}))};if(JSON.stringify(samples.at(-1))!==JSON.stringify(sample))samples.push(sample);if(["running","cancelling","queued","pending"].includes(current.state))return false;final=current;return true;},{timeout:480000,interval:1000,timeoutMsg:"任务未结束："+id});activeTask=null;tasks.push({id,action:final.action,state:final.state,samples});assert.equal(final.state,expected,JSON.stringify(final.targets));return final;}
function input(action,ids,apk){return{action,targetIds:ids,applicationId:apk?"xiaoxin":null,apk:apk||null,appVersion:apk?.appVersion||"",abi:"universal",reinstall:Boolean(apk),concurrency:1,expectedTargets:{}};}
async function execute(action,ids,apk){const request=input(action,ids,apk);const check=await command("screen_preflight",{localProjectId:projectId,input:request});assert.ok(check.items.every(i=>i.state==="ready"),JSON.stringify(check.items));return command("screen_execute",{localProjectId:projectId,preflightId:check.id,input:request});}
async function register(id){const preview=await command("screen_registration_preview",{localProjectId:projectId,screenIds:[id]});assert.ok(preview.items.every(i=>i.state==="ready"),JSON.stringify(preview.items));const task=await command("screen_registration_submit",{localProjectId:projectId,input:{previewId:preview.id,screenIds:[id],macConfirmations:{},spaceConfirmations:[]}});await waitTask(task);return(await load(true)).screens.find(s=>s.aliases.includes(id)).id;}
function installStamp(ip){const adb=resolve(executable,"../tools/android/adb.exe");const result=spawnSync(adb,["-s",ip+":5555","shell","dumpsys package chat.xiaoxin.app"],{encoding:"utf8",windowsHide:true,timeout:30000});assert.equal(result.status,0);return result.stdout.split(/\r?\n/).filter(s=>/versionCode=|versionName=|lastUpdateTime=/.test(s)).join("\n");}
async function crashAndRestart(){await browser.deleteSession().catch(()=>{});browser=null;app.kill();await new Promise(done=>{if(app.exitCode!==null)done();else app.once("exit",done);});await start();await selectProject(projectId);}
try{
  fixture("prepare");prepared=true;await start();
  const project=await command("create_local_project",{input:{name:"智能屏步骤7验收-"+Date.now(),platformUrl:"http://"+line("平台API："),dbHost:line("平台主机："),dbPort:3306,dbUser,dbTlsEnabled:false,dbPassword,businessDb:business,workbenchDb:shared}});projectId=project.id;
  await command("login_project",{projectId,request:{username:login.principal,password:login.credentials,sessionUuid:login.sessionUUID,imageCode:login.imageCode}});
  await load(true);await selectProject(projectId);
  const four=await command("screen_save_local",{localProjectId:projectId,fields:{name:"步骤7四寸屏",ip:line("4寸屏：").split(":")[0],mac:"",size:"4",spaceId:"1001",location:"整体联调"},id:null,expectedRevision:null});
  const ten=(await command("screen_import_local",{localProjectId:projectId,fields:[{name:"步骤7十寸屏",ip:line("10寸屏：").split(":")[0],mac:"",size:"10",spaceId:"1002",location:"整体联调"}]}))[0];
  await button("刷新平台");await screenshot("01-local-assets");
  await waitTask(await execute("inspect",[four,ten]));
  const registeredFour=await register(four);checks.push("本机新增及导入、两屏检查、真实注册");
  const apk=await command("screen_parse_apk",{filePath:resolve(root,"../../inxvision-xiaoxin/xiaoxin-app/release/2.0.9/xiaoxin-2.0.9-release.apk")});assert.equal(apk.appVersionCode,5019);
  database("reject_shared");sharedFault=true;
  const installedFour=await execute("install",[registeredFour],apk);
  await navigate("智能屏操作");await button("查看历史记录");
  const failedShared=await waitTask(installedFour,"needs_review");
  assert.equal(failedShared.targets[0].result.device,"succeeded");assert.equal(failedShared.targets[0].result.business,"succeeded");
  const stamp=installStamp(line("4寸屏：").split(":")[0]);
  await screenshot("02-device-success-shared-pending");
  database("restore_shared");sharedFault=false;
  await crashAndRestart();
  const recovered=(await load()).tasks.find(t=>t.id===installedFour);
  if(recovered.state!=="succeeded")await command("screen_verify",{localProjectId:projectId,taskId:installedFour});
  await waitTask(installedFour);assert.equal(installStamp(line("4寸屏：").split(":")[0]),stamp);
  checks.push("注册屏同版本覆盖成功，共享故障分别显示；退出重开核实补存，安装时间未改变");
  const installedTen=await execute("install",[ten],apk);await waitTask(installedTen);
  assert.ok(!database("summary").operations.some(row=>row[0]===installedTen));
  const registeredTen=await register(ten);
  const preview=await command("screen_version_preview",{localProjectId:projectId,screenIds:[registeredTen]});
  const synced=await command("screen_version_submit",{localProjectId:projectId,previewId:preview.id,screenIds:[registeredTen]});await waitTask(synced);
  const summary=database("summary");assert.equal(summary.screens.length,2);assert.ok(summary.screens.every(row=>row[2]==="2.0.9"));assert.ok(!summary.operations.some(row=>row[0]===installedTen));
  checks.push("未注册屏先安装再注册及独立版本同步，旧本机安装历史保留且没有上传");
  const diagnostic=await execute("diagnostics",[registeredTen]);await waitTask(diagnostic);
  const document=await command("screen_read_diagnostics",{localProjectId:projectId,taskId:diagnostic});assert.ok(document.includes("步骤7十寸屏"));
  const exportPath=await command("screen_export_diagnostics",{localProjectId:projectId,taskId:diagnostic,directory:evidence});assert.equal(await readFile(exportPath,"utf8"),document);
  await navigate("智能屏列表");await button("刷新平台");await screenshot("03-registered-assets");
  await navigate("智能屏操作");await button("查看历史记录");await screenshot("04-history-and-results");
  await button("任务与日志面板");await screenshot("05-task-logs");
  localProjectId=(await command("create_local_project",{input:{name:"步骤7隔离本机项目-"+Date.now(),platformUrl:"",dbHost:"",dbPort:3306,dbUser:"",dbTlsEnabled:false,dbPassword:null,businessDb:"",workbenchDb:"inxaiot_desk_buddy"}})).id;
  await selectProject(localProjectId);const empty=await command("screen_load",{localProjectId:localProjectId,refresh:false});assert.equal(empty.screens.length,0);await screenshot("06-project-isolation");
  await selectProject(projectId);await crashAndRestart();const after=await load(true);assert.equal(after.screens.length,2);assert.ok(after.tasks.some(task=>task.id===installedTen));
  checks.push("诊断查看导出一致、任务日志可见、项目切换隔离、重开后资产和本机历史保留");
  await writeFile(resolve(evidence,"result.json"),JSON.stringify({passed:true,checks,tasks,summary,business,shared},null,2),"utf8");
}catch(error){failure=error;if(browser)await screenshot("failure").catch(()=>{});await writeFile(resolve(evidence,"failure.json"),JSON.stringify({message:redact(error?.message||error),activeTask,projectId,business,shared,checks,tasks},null,2),"utf8");}
finally{
  if(sharedFault)try{database("restore_shared");sharedFault=false;}catch(error){failure??=error;}
  if(browser&&!activeTask){for(const id of [localProjectId,projectId].filter(Boolean))try{await command("delete_local_project",{projectId:id});}catch(error){failure??=error;}await browser.execute(()=>document.querySelector('button.close[aria-label="关闭"]')?.click()).catch(()=>{});await browser.deleteSession().catch(()=>{});}
  if(app&&!activeTask){for(let n=0;n<40&&app.exitCode===null;n++)await delay(250);if(app.exitCode===null)app.kill();}
  if(prepared&&!activeTask)try{fixture("cleanup");}catch(error){failure??=error;}
}
if(failure){console.error("SCREEN_STEP7_UI_FAILED "+redact(failure.message||failure)+" "+evidence);if(activeTask)console.error("保留运行中的测试程序和隔离数据，任务="+activeTask);process.exitCode=1;}else console.log("SCREEN_STEP7_UI_PASS "+evidence);
