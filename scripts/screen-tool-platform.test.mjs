import { test } from 'node:test';
import assert from 'node:assert/strict';
import { chmodSync, lstatSync, mkdtempSync, readFileSync, realpathSync, rmSync, statSync, symlinkSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import process from 'node:process';
import { Buffer } from 'node:buffer';
import { assertManifestTarget, assertToolTarget, binaryTarget, buildTarget, executableName } from './screen-tool-platform.mjs';
import { copyJavaLegalNotices } from './java-legal-notices.mjs';

function pe(machine = 0x8664) {
  const bytes = Buffer.alloc(128); bytes.write('MZ'); bytes.writeUInt32LE(64, 0x3c); bytes.write('PE\0\0', 64, 'binary'); bytes.writeUInt16LE(machine, 68); return bytes;
}
function mach(cpu = 0x01000007) {
  const bytes = Buffer.alloc(32); bytes.writeUInt32LE(0xfeedfacf); bytes.writeUInt32LE(cpu, 4); return bytes;
}
function universal() {
  const bytes = Buffer.alloc(48); bytes.writeUInt32BE(0xcafebabe); bytes.writeUInt32BE(2, 4); bytes.writeUInt32BE(0x01000007, 8); bytes.writeUInt32BE(0x0100000c, 28); return bytes;
}
function elf() {
  const bytes = Buffer.alloc(64); bytes.writeUInt32BE(0x7f454c46); bytes[4] = 2; bytes[5] = 1; bytes.writeUInt16LE(62, 18); return bytes;
}

test('各目标平台使用独立工具名，不允许在 Windows 冒充 Mac 构建', () => {
  for (const [platform, arch] of [['win32', 'x64'], ['darwin', 'x64'], ['darwin', 'arm64'], ['linux', 'x64']]) {
    assert.deepEqual(buildTarget(platform, arch, { platform, arch }), { platform, architecture: arch });
    assert.equal(executableName('adb', platform), platform === 'win32' ? 'adb.exe' : 'adb');
  }
  assert.throws(() => buildTarget('darwin', 'arm64', { platform: 'win32', arch: 'x64' }), /请在/);
  assert.throws(() => buildTarget('darwin', 'x64', { platform: 'darwin', arch: 'arm64' }), /请在/);
  assert.throws(() => buildTarget('win32', 'arm64', { platform: 'win32', arch: 'arm64' }), /暂不支持/);
});

test('识别 Windows、Linux 和 Intel/Apple Silicon/Universal Mac 文件', () => {
  assert.deepEqual(binaryTarget(pe()), { platform: 'win32', architectures: ['x64'] });
  assert.deepEqual(binaryTarget(elf()), { platform: 'linux', architectures: ['x64'] });
  assert.deepEqual(binaryTarget(mach()), { platform: 'darwin', architectures: ['x64'] });
  assert.deepEqual(binaryTarget(mach(0x0100000c)), { platform: 'darwin', architectures: ['arm64'] });
  assert.deepEqual(binaryTarget(universal()), { platform: 'darwin', architectures: ['x64', 'arm64'] });
});

test('重命名为 adb 也不能将 Windows 或 Linux 工具打入 Mac 包', () => {
  const target = { platform: 'darwin', architecture: 'arm64' };
  assert.throws(() => assertToolTarget(pe(), target, { name: 'adb' }), /平台不匹配/);
  assert.throws(() => assertToolTarget(elf(), target, { name: 'adb' }), /平台不匹配/);
  assert.equal(assertToolTarget(universal(), target, { name: 'adb' }).requiresRosetta, false);
});

test('Apple Silicon 的 ADB 与 Java 要包含本机架构，APK 解析工具可明确使用 Rosetta', () => {
  const target = { platform: 'darwin', architecture: 'arm64' };
  assert.equal(assertToolTarget(mach(), target, { name: 'aapt', allowRosetta: true }).requiresRosetta, true);
  assert.throws(() => assertToolTarget(mach(), target, { name: 'ADB' }), /ADB 架构不匹配/);
  assert.equal(assertToolTarget(universal(), target, { name: 'ADB' }).requiresRosetta, false);
  assert.throws(() => assertToolTarget(mach(), target, { name: 'Java' }), /Java 架构不匹配/);
  assert.equal(assertToolTarget(mach(0x0100000c), target, { name: 'Java' }).requiresRosetta, false);
  assert.throws(() => assertToolTarget(mach(0x0100000c), { platform: 'darwin', architecture: 'x64' }, { allowRosetta: true }), /架构不匹配/);
});

test('保留已验证的 Windows 32 位 ADB 在 x64 包内运行，Java 仍要求 x64', () => {
  const target = { platform: 'win32', architecture: 'x64' };
  assert.equal(assertToolTarget(pe(0x14c), target, { name: 'ADB', allowWindowsX86: true }).requiresRosetta, false);
  assert.throws(() => assertToolTarget(pe(0x14c), target, { name: 'Java' }), /Java 架构不匹配/);
});

test('清单缺少平台、系统不同或应用架构不同均拒绝', () => {
  const target = { platform: 'darwin', architecture: 'arm64' };
  assert.doesNotThrow(() => assertManifestTarget({ os: 'darwin', architecture: 'arm64' }, target));
  for (const versions of [undefined, {}, { os: 'win32', architecture: 'arm64' }, { os: 'darwin', architecture: 'x64' }]) {
    assert.throws(() => assertManifestTarget(versions, target), /平台或架构不匹配/);
  }
});

test('截断、损坏或无法识别的可执行文件拒绝', () => {
  for (const bytes of [Buffer.alloc(0), Buffer.alloc(30), pe().subarray(0, 50), universal().subarray(0, 30)]) {
    assert.throws(() => binaryTarget(bytes));
  }
  const bytes = pe(); bytes.writeUInt32LE(1000, 0x3c);
  assert.throws(() => binaryTarget(bytes), /PE 文件头无效/);
});

test('命令行验证和重算清单都拒绝错误系统，且不改原清单', () => {
  const root = mkdtempSync(join(tmpdir(), 'inx-platform-reject-')); mkdirSync(join(root, 'tools'));
  const manifest = join(root, 'tools/android-manifest.json');
  const original = JSON.stringify({ formatVersion: 1, versions: { os: process.platform === 'darwin' ? 'win32' : 'darwin', architecture: process.arch }, files: [] });
  writeFileSync(manifest, original);
  for (const action of ['--verify', '--refresh-manifest']) {
    const result = spawnSync(process.execPath, ['scripts/package-screen-tools.mjs', '--output', root, action], { encoding: 'utf8', windowsHide: true, timeout: 10000 });
    assert.notEqual(result.status, 0); assert.match(result.stderr, /平台或架构不匹配/);
    assert.equal(readFileSync(manifest, 'utf8'), original);
  }
});

function legalFixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'inx-java-legal-'));
  const source = join(root, 'jdk');
  const runtime = join(root, 'runtime');
  for (const directory of [join(source, 'legal/java.base'), join(source, 'legal/java.logging'), join(runtime, 'legal/java.base'), join(runtime, 'legal/java.logging'), join(runtime, 'bin')]) mkdirSync(directory, { recursive: true });
  t.after(() => {
    assert.equal(realpathSync(root).startsWith(realpathSync(tmpdir())), true);
    rmSync(root, { recursive: true, force: true });
  });
  return { root, source, runtime };
}

test('覆盖 jlink 只读许可证，不修改源 JDK 和运行时程序的内容及权限', t => {
  const { source, runtime } = legalFixture(t);
  const sourceFile = join(source, 'legal/java.logging/LICENSE');
  const targetFile = join(runtime, 'legal/java.logging/LICENSE');
  const java = join(runtime, 'bin/java');
  writeFileSync(sourceFile, '完整许可证原文'); chmodSync(sourceFile, 0o444);
  writeFileSync(targetFile, 'jlink 原许可证'); chmodSync(targetFile, 0o444);
  writeFileSync(java, '可执行文件内容'); chmodSync(java, 0o755);
  const sourceMode = statSync(sourceFile).mode;
  const javaMode = statSync(java).mode;
  copyJavaLegalNotices(source, runtime);
  assert.equal(readFileSync(targetFile, 'utf8'), '完整许可证原文');
  assert.equal(readFileSync(sourceFile, 'utf8'), '完整许可证原文');
  assert.equal(statSync(sourceFile).mode, sourceMode);
  assert.equal(statSync(targetFile).mode & 0o777, sourceMode & 0o777);
  assert.equal(readFileSync(java, 'utf8'), '可执行文件内容');
  assert.equal(statSync(java).mode, javaMode);
});

test('Java 模块许可证的源链接与目标链接均转成独立文件，保持许可证内容', t => {
  const { root, source, runtime } = legalFixture(t);
  const original = join(source, 'legal/java.base/LICENSE');
  const linked = join(source, 'legal/java.logging/LICENSE');
  const target = join(runtime, 'legal/java.logging/LICENSE');
  const outside = join(root, 'outside-license');
  writeFileSync(original, '共享许可证原文'); writeFileSync(outside, '目标链接不能改写这里');
  try {
    symlinkSync(original, linked, 'file');
    symlinkSync(outside, target, 'file');
  } catch (error) {
    if (process.platform === 'win32' && error.code === 'EPERM') { t.skip('当前 Windows 账号没有创建文件符号链接权限；Unix CI 执行此验证'); return; }
    throw error;
  }
  copyJavaLegalNotices(source, runtime);
  assert.equal(lstatSync(linked).isSymbolicLink(), true);
  assert.equal(lstatSync(target).isSymbolicLink(), false);
  assert.equal(readFileSync(target, 'utf8'), '共享许可证原文');
  assert.equal(readFileSync(outside, 'utf8'), '目标链接不能改写这里');
});

test('不允许 Java 许可证目标指向源 JDK 或通过目录链接写到外部', t => {
  const { root, source, runtime } = legalFixture(t);
  assert.throws(() => copyJavaLegalNotices(source, source), /必须分开/);
  rmSync(join(runtime, 'legal/java.logging'), { recursive: true });
  const outside = join(root, 'outside'); mkdirSync(outside);
  try { symlinkSync(outside, join(runtime, 'legal/java.logging'), process.platform === 'win32' ? 'junction' : 'dir'); } catch (error) {
    if (process.platform === 'win32' && error.code === 'EPERM') { t.skip('当前 Windows 账号没有创建目录链接权限；Unix CI 执行此验证'); return; }
    throw error;
  }
  assert.throws(() => copyJavaLegalNotices(source, runtime), /目标目录无效/);
});
