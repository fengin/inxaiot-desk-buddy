import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdirSync, mkdtempSync, readFileSync, writeFileSync, readdirSync, rmSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { Buffer } from 'node:buffer';
import { buildWindows } from './build-windows.mjs';

test('Windows先准备内嵌ADB、再构建并独立验证，发布目录无需tools', t=>{
  const root=mkdtempSync(join(tmpdir(),'inx-windows-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  const steps=[];const payload=Buffer.from('embedded fixture');
  const run=(file,args,options)=>{
    assert.equal(options.cwd,root);assert.equal(options.windowsHide,true);assert.ok(!options.shell);
    if(args[0]==='scripts/package-screen-tools.mjs'){
      steps.push('prepare');writeFileSync(args[args.indexOf('--embedded-output')+1],payload);
    } else if(args[0]==='node_modules/@tauri-apps/cli/tauri.js'){
      steps.push('build');assert.deepEqual(readFileSync(options.env.INX_EMBEDDED_ADB),payload);
      const executable=join(root,'src-tauri/target/release/inxaiot-desk-buddy.exe');mkdirSync(dirname(executable),{recursive:true});writeFileSync(executable,Buffer.concat([Buffer.from('MZ'),payload]));
    } else if(args[0]==='--verify-package') {
      steps.push('verify');assert.equal(options.env.JAVA_HOME,undefined);assert.equal(options.env.INX_ADB_PATH,undefined);
      writeFileSync(args[1],JSON.stringify({successful:true,embedded:true}));
    } else assert.fail('非预期步骤');
    return {status:0};
  };
  buildWindows({platform:'win32',arch:'x64',root,env:{JAVA_HOME:'old-java',INX_ADB_PATH:'old-adb'},run,node:'node'});
  assert.deepEqual(steps,['prepare','build','verify']);
  assert.deepEqual(readdirSync(join(root,'src-tauri/target')),['release']);
});
test('Windows工具准备失败即停止，不生成缺工具程序',t=>{
  const root=mkdtempSync(join(tmpdir(),'inx-windows-failed-'));t.after(()=>rmSync(root,{recursive:true,force:true}));
  let calls=0;assert.throws(()=>buildWindows({platform:'win32',arch:'x64',root,env:{},run:()=>{calls++;return {status:1};}}),/构建步骤失败/);assert.equal(calls,1);
  assert.deepEqual(readdirSync(join(root,'src-tauri/target')),[]);
});
test('Windows构建拒绝不匹配的系统、架构和产物目录',()=>{
  assert.throws(()=>buildWindows({platform:'darwin'}),/Windows x64/);
  assert.throws(()=>buildWindows({platform:'win32',arch:'arm64'}),/Windows x64/);
  assert.throws(()=>buildWindows({platform:'win32',arch:'x64',env:{CARGO_TARGET_DIR:'elsewhere'}}),/默认产物目录/);
});
