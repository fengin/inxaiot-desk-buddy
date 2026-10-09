import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import process from 'node:process';
import { Buffer } from 'node:buffer';
import { assertManifestTarget, assertToolTarget, binaryTarget, buildTarget, executableName } from './screen-tool-platform.mjs';

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

test('Apple Silicon 的 ADB 必须包含本机架构，不再依赖 Rosetta', () => {
  const target = { platform: 'darwin', architecture: 'arm64' };
  assert.throws(() => assertToolTarget(mach(), target, { name: 'ADB' }), /ADB 架构不匹配/);
  assert.equal(assertToolTarget(universal(), target, { name: 'ADB' }).requiresRosetta, false);
  assert.equal(assertToolTarget(mach(0x0100000c), target, { name: 'ADB' }).requiresRosetta, false);
  assert.throws(() => assertToolTarget(mach(0x0100000c), { platform: 'darwin', architecture: 'x64' }, { name: 'ADB' }), /架构不匹配/);
});

test('保留已验证的 Windows 32 位 ADB 在 x64 包内运行', () => {
  const target = { platform: 'win32', architecture: 'x64' };
  assert.equal(assertToolTarget(pe(0x14c), target, { name: 'ADB', allowWindowsX86: true }).requiresRosetta, false);
  assert.throws(() => assertToolTarget(pe(0x14c), target, { name: '工具' }), /工具 架构不匹配/);
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

test('命令行验证和重算清单都拒绝错误系统，且不改原清单', t => {
  const root = mkdtempSync(join(tmpdir(), 'inx-platform-reject-')); mkdirSync(join(root, 'tools'));
  t.after(()=>rmSync(root,{recursive:true,force:true}));
  const manifest = join(root, 'tools/android-manifest.json');
  const original = JSON.stringify({ formatVersion: 2, versions: { os: process.platform === 'darwin' ? 'win32' : 'darwin', architecture: process.arch }, files: [] });
  writeFileSync(manifest, original);
  for (const action of ['--verify', '--refresh-manifest']) {
    const result = spawnSync(process.execPath, ['scripts/package-screen-tools.mjs', '--output', root, action], { encoding: 'utf8', windowsHide: true, timeout: 10000 });
    assert.notEqual(result.status, 0); assert.match(result.stderr, /平台或架构不匹配/);
    assert.equal(readFileSync(manifest, 'utf8'), original);
  }
});
