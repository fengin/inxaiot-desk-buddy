import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, resolve, relative, sep } from 'node:path';
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import process from 'node:process';
import console from 'node:console';
import { assertManifestTarget, assertToolTarget, buildTarget, executableName } from './screen-tool-platform.mjs';

const args = process.argv.slice(2);
const option = (name, fallback) => {
  const index = args.indexOf(name);
  if (index < 0) return fallback;
  if (!args[index + 1] || args[index + 1].startsWith('--')) throw new Error(`${name} 缺少参数`);
  return args[index + 1];
};
const target = buildTarget(option('--platform', process.platform), option('--arch', process.arch));
const output = option('--output');
if (!output) throw new Error('必须指定 --output：Windows/Linux 为程序目录，macOS 为 Contents/Resources');
const root = join(resolve(output), 'tools');
const exe = name => executableName(name, target.platform);
const run = (file, arguments_) => {
  const result = spawnSync(file, arguments_, { encoding: 'utf8', windowsHide: true, timeout: 120000 });
  if (result.error || result.status !== 0) throw new Error(`随包工具运行失败：${file}\n${result.error?.message ?? result.stderr}`);
  return `${result.stdout}${result.stderr}`.trim();
};
const required = [`android/${exe('adb')}`, `android-build/${exe('aapt')}`, 'android-build/lib/apksigner.jar', `java/bin/${exe('java')}`,
  ...(target.platform === 'win32' ? ['android/AdbWinApi.dll','android/AdbWinUsbApi.dll'] : [])];
const manifestPath = join(root, 'android-manifest.json');
const entries = directory => readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
  const path = join(directory, entry.name);
  if (entry.isSymbolicLink()) throw new Error(`随包工具不接受符号链接：${path}`);
  return entry.isDirectory() ? entries(path) : path === manifestPath ? [] : [path];
});
const hash = file => createHash('sha256').update(readFileSync(file)).digest('hex');
const checkBinaries = (adb, aapt, java) => {
  const binaries = {
    adb: assertToolTarget(readFileSync(adb), target, { name: 'ADB', allowWindowsX86: true }),
    aapt: assertToolTarget(readFileSync(aapt), target, { name: 'aapt', allowRosetta: true, allowWindowsX86: true }),
    java: assertToolTarget(readFileSync(java), target, { name: 'Java' })
  };
  return { toolArchitectures: Object.fromEntries(Object.entries(binaries).map(([name, value]) => [name, value.architectures])), requiresRosetta: Object.values(binaries).some(value => value.requiresRosetta) };
};
const smoke = () => {
  for (const name of required) if (!existsSync(join(root, name))) throw new Error(`缺少随包文件：${name}`);
  const binaryInfo = checkBinaries(join(root, 'android', exe('adb')), join(root, 'android-build', exe('aapt')), join(root, 'java/bin', exe('java')));
  if (binaryInfo.requiresRosetta) console.log('此 Apple Silicon 工具包包含 Intel 版 Android 工具，运行电脑需要 Rosetta 2；Java 使用原生 arm64。');
  console.log(run(join(root, 'android', exe('adb')), ['version']));
  console.log(run(join(root, 'android-build', exe('aapt')), ['version']));
  console.log(run(join(root, 'java/bin', exe('java')), ['-jar', join(root, 'android-build/lib/apksigner.jar'), 'version']));
  return binaryInfo;
};
if (args.includes('--verify')) {
  const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
  assertManifestTarget(manifest.versions, target);
  const actual = entries(root).map(file => relative(root, file).split(sep).join('/')).sort();
  const declared = manifest.files.map(file => file.path).sort();
  if (manifest.formatVersion !== 1 || JSON.stringify(actual) !== JSON.stringify(declared)) throw new Error('工具清单与文件集合不一致');
  for (const file of manifest.files) if (hash(join(root, file.path)) !== file.sha256) throw new Error(`工具校验失败：${file.path}`);
  smoke();
} else {
  let versions;
  if (args.includes('--refresh-manifest')) {
    versions = JSON.parse(readFileSync(manifestPath, 'utf8')).versions;
    assertManifestTarget(versions, target);
  } else {
    const sdk = option('--sdk', process.env.ANDROID_SDK_ROOT ?? process.env.ANDROID_HOME);
    const java = option('--java', process.env.JAVA_HOME);
    if (!sdk || !java) throw new Error('请提供 Android SDK 和 Java 21 路径（--sdk、--java 或对应环境变量）');
    if (existsSync(root)) throw new Error('tools 已存在，请使用新的输出目录');
    const platform = join(sdk, 'platform-tools'), build = join(sdk, 'build-tools/36.1.0');
    if (!existsSync(join(platform, 'NOTICE.txt'))) throw new Error('缺少 Android platform-tools 许可证说明');
    const revision = path => readFileSync(join(path, 'source.properties'), 'utf8').match(/^Pkg.Revision\s*=\s*(.+)$/m)?.[1].trim();
    const javaRelease = readFileSync(join(java, 'release'), 'utf8');
    const javaVersion = javaRelease.match(/JAVA_VERSION="([^"]+)"/)?.[1];
    const vendor = javaRelease.match(/IMPLEMENTOR="([^"]+)"/)?.[1];
    if (!javaVersion?.startsWith('21.') || !['Eclipse Adoptium', 'JetBrains s.r.o.'].includes(vendor)) throw new Error('随包 Java 必须使用 Temurin/JetBrains Java 21，并保留许可证');
    if (!(Number(revision(platform)?.split('.')[0]) >= 35) || revision(build) !== '36.1.0') throw new Error('需要 platform-tools 35+ 和 build-tools 36.1.0');
    if (!existsSync(join(java, 'legal/java.base/LICENSE'))) throw new Error('缺少 Java 许可证');
    checkBinaries(join(platform, exe('adb')), join(build, exe('aapt')), join(java, 'bin', exe('java')));
    mkdirSync(join(root, 'android-build/lib'), { recursive: true });
    cpSync(platform, join(root, 'android'), { recursive: true, dereference: true });
    for (const name of [exe('aapt'), 'NOTICE.txt', 'source.properties']) cpSync(join(build, name), join(root, 'android-build', name));
    cpSync(join(build, 'lib/apksigner.jar'), join(root, 'android-build/lib/apksigner.jar'));
    for (const name of readdirSync(build)) if (/\.(dll|so|dylib)$/.test(name) || name === 'lib64') cpSync(join(build, name), join(root, 'android-build', name), { recursive: true, dereference: true });
    if (existsSync(join(java, 'jmods'))) {
      run(join(java, 'bin', exe('jlink')), ['--add-modules','java.base,java.logging,jdk.crypto.ec,jdk.charsets','--strip-debug','--no-header-files','--no-man-pages','--output',join(root,'java')]);
      cpSync(join(java, 'legal'), join(root, 'java/legal'), {recursive:true,dereference:true});
    } else cpSync(java, join(root, 'java'), { recursive: true, dereference: true });
    versions = { platformTools:revision(platform),buildTools:revision(build),javaRuntime:`${vendor} ${javaVersion}`,os:target.platform,architecture:target.architecture };
  }
  Object.assign(versions, smoke());
  const files = entries(root).sort().map(file => ({path:relative(root,file).split(sep).join('/'),sha256:hash(file)}));
  writeFileSync(manifestPath, JSON.stringify({formatVersion:1,versions,files},null,2)+'\n','utf8');
  console.log(`工具包已生成：${root}（${files.length} 个文件）`);
}
