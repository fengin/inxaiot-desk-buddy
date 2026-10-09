import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
import { dirname, isAbsolute, join, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import process from 'node:process';
import console from 'node:console';

function removeStaging(target, staging) {
  const child=relative(realpathSync(target),realpathSync(staging));
  if(!child || child.startsWith('..') || isAbsolute(child))throw new Error('构建暂存目录越界');
  rmSync(staging,{recursive:true,force:true});
}

export function buildWindows({ platform=process.platform, arch=process.arch, env=process.env, root=resolve(dirname(fileURLToPath(import.meta.url)),'..'), run=spawnSync, node=process.execPath }={}) {
  if (platform!=='win32' || arch!=='x64') throw new Error('Windows 单文件包必须在 Windows x64 上构建');
  if (env.CARGO_BUILD_TARGET || env.CARGO_TARGET_DIR) throw new Error('Windows 单文件包使用默认产物目录和本机架构');
  root=realpathSync(root);
  const target=join(root,'src-tauri/target');mkdirSync(target,{recursive:true});
  const staging=mkdtempSync(join(target,'windows-adb-'));
  const archive=join(staging,'embedded-adb.json.gz');
  const execute=(file,args,variables=env)=>{
    const result=run(file,args,{cwd:root,env:variables,stdio:'inherit',windowsHide:true});
    if(result.error || result.status!==0)throw new Error('Windows 构建步骤失败：'+file+' '+args.join(' '));
  };
  try {
    execute(node,['scripts/package-screen-tools.mjs','--platform','win32','--arch','x64','--output',staging,'--embedded-output',archive]);
    execute(node,['node_modules/@tauri-apps/cli/tauri.js','build','--no-bundle','--config','src-tauri/tauri.release.conf.json'],{...env,INX_EMBEDDED_ADB:archive});
    const executable=join(target,'release/inxaiot-desk-buddy.exe');
    if(!existsSync(executable) || !readFileSync(executable).includes(readFileSync(archive)))throw new Error('Windows 程序未包含预期的 ADB 工具包');
    const report=join(staging,'verification.json');
    const clean={...env};for(const key of Object.keys(clean))if(['JAVA_HOME','ANDROID_HOME','ANDROID_SDK_ROOT','INX_ADB_PATH','INX_AAPT_PATH','INX_APKSIGNER_JAR'].includes(key.toUpperCase()))delete clean[key];
    execute(executable,['--verify-package',report],clean);
    const checked=JSON.parse(readFileSync(report,'utf8'));
    if(!checked.successful || !checked.embedded)throw new Error('Windows 内嵌 ADB 独立运行验证失败');
    return executable;
  } finally {
    removeStaging(target,staging);
  }
}
if(process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  try { console.log('Windows 单文件包构建完成：'+buildWindows()); }
  catch(error){console.error(error.message);process.exitCode=1;}
}
