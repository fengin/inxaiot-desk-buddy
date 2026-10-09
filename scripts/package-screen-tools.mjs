import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, resolve, relative, sep } from 'node:path';
import { createHash } from 'node:crypto';
import { gzipSync } from 'node:zlib';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import process from 'node:process';
import console from 'node:console';
import { assertManifestTarget, assertToolTarget, buildTarget, executableName } from './screen-tool-platform.mjs';

const entries = (directory, manifestPath) => readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
  const path = join(directory, entry.name);
  if (entry.isSymbolicLink()) throw new Error('随包工具不接受符号链接：' + path);
  return entry.isDirectory() ? entries(path, manifestPath) : path === manifestPath ? [] : [path];
});
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
export function packageAdb({ output, sdk, platform = process.platform, arch = process.arch, verify = false, refresh = false, embeddedOutput, run = spawnSync }) {
  const target = buildTarget(platform, arch), root = join(resolve(output), 'tools');
  const executable = executableName('adb', target.platform), manifestPath = join(root, 'android-manifest.json');
  const required = ['android/' + executable, 'android/NOTICE.txt', 'android/source.properties',
    ...(platform === 'win32' ? ['android/AdbWinApi.dll', 'android/AdbWinUsbApi.dll'] : [])];
  const smoke = () => {
    for (const name of required) if (!existsSync(join(root, name))) throw new Error('缺少随包文件：' + name);
    const actual = assertToolTarget(readFileSync(join(root, 'android', executable)), target, { name: 'ADB', allowWindowsX86: true });
    const result = run(join(root, 'android', executable), ['version'], { encoding:'utf8', windowsHide:true, timeout:30000 });
    if (result.error || result.status !== 0) throw new Error('随包 ADB 无法运行：' + (result.error?.message ?? result.stderr));
    return { toolArchitectures: { adb: actual.architectures } };
  };
  let versions;
  if (verify || refresh) {
    const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
    if (![1,2].includes(manifest.formatVersion) || (refresh && manifest.formatVersion!==2)) throw new Error('工具清单格式无效');
    assertManifestTarget(manifest.versions, target);
    versions = manifest.versions;
    if (verify) {
      const actual = entries(root, manifestPath).map(file => relative(root,file).split(sep).join('/')).sort();
      const declared = manifest.files.map(file=>file.path).sort();
      if (JSON.stringify(actual)!==JSON.stringify(declared)) throw new Error('工具清单与文件集合不一致');
      for (const file of manifest.files) {
        if (!actual.includes(file.path) || hash(readFileSync(join(root,file.path)))!==file.sha256) throw new Error('工具校验失败：'+file.path);
      }
      smoke();
      return root;
    }
  } else {
    if (!sdk) throw new Error('请提供 Android SDK 路径（--sdk 或 ANDROID_SDK_ROOT / ANDROID_HOME）');
    if (existsSync(root)) throw new Error('tools 已存在，请使用新的输出目录');
    const source = join(sdk, 'platform-tools');
    const revision = readFileSync(join(source,'source.properties'),'utf8').match(/^Pkg.Revision\s*=\s*(.+)$/m)?.[1].trim();
    if (!(Number(revision?.split('.')[0]) >= 35)) throw new Error('需要 platform-tools 35 或以上版本');
    assertToolTarget(readFileSync(join(source,executable)),target,{name:'ADB',allowWindowsX86:true});
    mkdirSync(join(root,'android'), { recursive:true });
    for (const name of required) cpSync(join(source,name.slice('android/'.length)),join(root,name));
    // Unix ADB 如带独立动态库，随包保留；不再复制 fastboot、文件系统工具或 APK/Java 工具。
    if (platform !== 'win32') {
      for (const name of readdirSync(source)) if (/\.(dylib|so)(\.\d+)*$/.test(name) || name==='lib64') cpSync(join(source,name),join(root,'android',name),{recursive:true,dereference:true});
    }
    versions = { platformTools:revision, os:platform, architecture:arch };
  }
  Object.assign(versions,smoke());
  const files=entries(root,manifestPath).sort().map(file=>({path:relative(root,file).split(sep).join('/'),sha256:hash(readFileSync(file))}));
  writeFileSync(manifestPath,JSON.stringify({formatVersion:2,versions,files},null,2)+'\n');
  if (embeddedOutput) {
    if (platform!=='win32') throw new Error('只有 Windows 构建使用内嵌 ADB');
    const payload={formatVersion:1,files:[...files.map(file=>({path:file.path,data:readFileSync(join(root,file.path)).toString('base64')})),{path:'android-manifest.json',data:readFileSync(manifestPath).toString('base64')}]};
    writeFileSync(embeddedOutput,gzipSync(JSON.stringify(payload),{level:9}));
  }
  return root;
}
if (process.argv[1] && resolve(process.argv[1])===fileURLToPath(import.meta.url)) {
  const args=process.argv.slice(2), option=(name,fallback)=>{const i=args.indexOf(name);if(i<0)return fallback;if(!args[i+1]||args[i+1].startsWith('--'))throw new Error(name+' 缺少参数');return args[i+1];};
  try {
    const output=option('--output');if(!output)throw new Error('必须指定 --output');
    const root=packageAdb({output,sdk:option('--sdk',process.env.ANDROID_SDK_ROOT??process.env.ANDROID_HOME),platform:option('--platform',process.platform),arch:option('--arch',process.arch),verify:args.includes('--verify'),refresh:args.includes('--refresh-manifest'),embeddedOutput:option('--embedded-output')});
    console.log('ADB 工具检查通过：'+root);
  } catch(error) {console.error(error.message);process.exitCode=1;}
}
