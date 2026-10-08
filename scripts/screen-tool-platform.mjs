import process from 'node:process';

const supported = { win32: ['x64'], darwin: ['x64', 'arm64'], linux: ['x64'] };

export function buildTarget(platform = process.platform, architecture = process.arch, host = process) {
  if (!supported[platform]?.includes(architecture)) throw new Error(`暂不支持随包工具目标：${platform}/${architecture}`);
  if (platform !== host.platform || architecture !== host.arch) throw new Error(`请在 ${platform}/${architecture} 环境构建并运行工具检查，当前为 ${host.platform}/${host.arch}`);
  return { platform, architecture };
}

export const executableName = (name, platform) => platform === 'win32' ? `${name}.exe` : name;

export function assertManifestTarget(versions, target) {
  if (versions?.os !== target.platform || versions?.architecture !== target.architecture) {
    throw new Error(`工具清单平台或架构不匹配：需要 ${target.platform}/${target.architecture}，实际 ${versions?.os ?? '未记录'}/${versions?.architecture ?? '未记录'}`);
  }
}

const machArchitecture = cpu => ({ 0x01000007: 'x64', 0x0100000c: 'arm64', 7: 'ia32' })[cpu];

// 读取文件头，避免只靠扩展名或构建电脑名称把其他系统的工具装进包中。
export function binaryTarget(bytes) {
  if (bytes.length < 20) throw new Error('可执行文件头不完整');
  if (bytes.toString('ascii', 0, 2) === 'MZ') {
    if (bytes.length < 64) throw new Error('Windows 可执行文件头不完整');
    const offset = bytes.readUInt32LE(0x3c);
    if (offset + 6 > bytes.length || bytes.toString('binary', offset, offset + 4) !== 'PE\0\0') throw new Error('Windows PE 文件头无效');
    const architecture = { 0x8664: 'x64', 0xaa64: 'arm64', 0x14c: 'ia32' }[bytes.readUInt16LE(offset + 4)];
    return { platform: 'win32', architectures: architecture ? [architecture] : [] };
  }
  if (bytes.readUInt32BE(0) === 0x7f454c46) {
    if (![1, 2].includes(bytes[5])) throw new Error('Linux ELF 文件头无效');
    const machine = bytes[5] === 1 ? bytes.readUInt16LE(18) : bytes.readUInt16BE(18);
    const architecture = { 62: 'x64', 183: 'arm64', 3: 'ia32' }[machine];
    return { platform: 'linux', architectures: architecture ? [architecture] : [] };
  }
  const magic = bytes.readUInt32BE(0);
  if ([0xfeedface, 0xfeedfacf, 0xcefaedfe, 0xcffaedfe].includes(magic)) {
    const little = [0xcefaedfe, 0xcffaedfe].includes(magic);
    const architecture = machArchitecture(little ? bytes.readUInt32LE(4) : bytes.readUInt32BE(4));
    return { platform: 'darwin', architectures: architecture ? [architecture] : [] };
  }
  if ([0xcafebabe, 0xcafebabf, 0xbebafeca, 0xbfbafeca].includes(magic)) {
    const little = [0xbebafeca, 0xbfbafeca].includes(magic);
    const read = offset => little ? bytes.readUInt32LE(offset) : bytes.readUInt32BE(offset);
    const count = read(4), stride = [0xcafebabf, 0xbfbafeca].includes(magic) ? 32 : 20;
    if (!count || count > 32 || 8 + count * stride > bytes.length) throw new Error('macOS Universal 文件头无效');
    return { platform: 'darwin', architectures: [...new Set(Array.from({ length: count }, (_, i) => machArchitecture(read(8 + i * stride))).filter(Boolean))] };
  }
  throw new Error('无法识别可执行文件平台');
}

export function assertToolTarget(bytes, target, { name, allowRosetta = false, allowWindowsX86 = false } = {}) {
  const actual = binaryTarget(bytes);
  if (actual.platform !== target.platform) throw new Error(`${name ?? '工具'} 平台不匹配：需要 ${target.platform}，实际 ${actual.platform}`);
  if (actual.architectures.includes(target.architecture)) return { ...actual, requiresRosetta: false };
  // 官方 Windows ADB 可为 32 位，x64 Windows 可运行；Java 仍使用目标原生架构。
  if (allowWindowsX86 && target.platform === 'win32' && target.architecture === 'x64' && actual.architectures.includes('ia32')) {
    return { ...actual, requiresRosetta: false };
  }
  if (allowRosetta && target.platform === 'darwin' && target.architecture === 'arm64' && actual.architectures.includes('x64')) {
    return { ...actual, requiresRosetta: true };
  }
  throw new Error(`${name ?? '工具'} 架构不匹配：需要 ${target.architecture}，实际 ${actual.architectures.join('/') || '未知'}`);
}
