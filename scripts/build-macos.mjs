import { spawnSync } from 'node:child_process'
import { closeSync, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, openSync, readFileSync, readdirSync, readSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path'
import { Buffer } from 'node:buffer'
import console from 'node:console'
import process from 'node:process'
import { fileURLToPath, URL } from 'node:url'
import { binaryTarget } from './screen-tool-platform.mjs'

const defaultProjectRoot = fileURLToPath(new URL('..', import.meta.url))

function requireInside(parent, path) {
  if (existsSync(path) && lstatSync(path).isSymbolicLink()) {
    throw new Error(`构建目录不能是符号链接：${path}`)
  }
  const absolute = existsSync(path) ? realpathSync(path) : join(realpathSync(dirname(path)), basename(path))
  const within = relative(realpathSync(parent), absolute)
  if (!within || within.startsWith('..') || isAbsolute(within)) {
    throw new Error(`构建目录不在预期位置：${path}`)
  }
}

function macosToolBinaries(root) {
  const collect = directory => readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const file = join(directory, entry.name)
    if (entry.isSymbolicLink()) throw new Error(`随包工具不能是符号链接：${file}`)
    if (entry.isDirectory()) return collect(file)
    if (!entry.isFile()) return []
    const descriptor = openSync(file, 'r')
    const header = Buffer.alloc(2048)
    let length
    try { length = readSync(descriptor, header, 0, header.length, 0) } finally { closeSync(descriptor) }
    try { return binaryTarget(header.subarray(0, length)).platform === 'darwin' ? [file] : [] } catch { return [] }
  })
  // 先处理较深目录中的动态库，再处理 ADB；普通数据与许可证不参与签名。
  return collect(root).sort((a, b) => b.split(sep).length - a.split(sep).length || a.localeCompare(b))
}

export function buildMacos({
  args = [], platform = process.platform, arch = process.arch, env = process.env,
  projectRoot = defaultProjectRoot, node = process.execPath, run = spawnSync, log = console.log,
} = {}) {
  if (platform !== 'darwin') {
    throw new Error('macOS .app 必须在 Mac 上构建；Windows 请使用 pnpm build:windows。')
  }
  if (args.length && (args.length !== 2 || args[0] !== '--arch')) {
    throw new Error('macOS 构建参数仅支持 --arch x64 或 --arch arm64。')
  }
  const targetArch = args[1] ?? arch
  if (!['x64', 'arm64'].includes(targetArch) || targetArch !== arch) {
    throw new Error(`请在对应架构的 Mac 上构建：当前 ${arch}，目标 ${targetArch}。`)
  }
  if (env.CARGO_BUILD_TARGET || env.CARGO_TARGET_DIR) {
    throw new Error('macOS 完整包使用本机架构和默认产物目录，请先移除 CARGO_BUILD_TARGET、CARGO_TARGET_DIR。')
  }

  projectRoot = realpathSync(projectRoot)
  const targetRoot = join(projectRoot, 'src-tauri/target')
  mkdirSync(targetRoot, { recursive: true })
  requireInside(projectRoot, targetRoot)
  const staging = mkdtempSync(join(targetRoot, 'macos-tools-'))
  const signingIdentity = env.APPLE_SIGNING_IDENTITY?.trim() || '-'
  const buildEnv = { ...env, APPLE_SIGNING_IDENTITY: signingIdentity }
  const execute = (file, arguments_, capture = false) => {
    const result = run(file, arguments_, {
      cwd: projectRoot, env: buildEnv, encoding: 'utf8', windowsHide: true,
      stdio: capture ? 'pipe' : 'inherit',
    })
    if (result.error || result.status !== 0) {
      throw new Error(`macOS 构建步骤失败：${file} ${arguments_.join(' ')}${result.error ? `\n${result.error.message}` : ''}`)
    }
    return String(result.stdout ?? '').trim()
  }
  const tools = (output, operation) => execute(node, [
    'scripts/package-screen-tools.mjs', '--platform', 'darwin', '--arch', targetArch,
    '--output', output, ...(operation ? [operation] : []),
  ])

  try {
    // 编译前确认 Mac 工具可运行，避免生成缺少设备管理工具的应用包。
    tools(staging)
    execute('pnpm', ['tauri', 'build', '--bundles', 'app', '--config', 'src-tauri/tauri.macos.conf.json'])
    const config = JSON.parse(readFileSync(join(projectRoot, 'src-tauri/tauri.conf.json'), 'utf8'))
    const appPath = join(targetRoot, 'release/bundle/macos', `${config.productName}.app`)
    const executable = join(appPath, 'Contents/MacOS/inxaiot-desk-buddy')
    if (!existsSync(executable)) throw new Error(`未找到 macOS 应用产物：${executable}`)
    requireInside(targetRoot, appPath)
    const binaryArch = execute('/usr/bin/lipo', ['-archs', executable], true)
    const expectedArch = targetArch === 'x64' ? 'x86_64' : 'arm64'
    if (binaryArch !== expectedArch) {
      throw new Error(`应用架构不匹配：预期 ${expectedArch}，实际 ${binaryArch || '无法读取'}。`)
    }

    const resources = join(appPath, 'Contents/Resources')
    mkdirSync(resources, { recursive: true })
    requireInside(appPath, realpathSync(resources))
    const destination = join(resources, 'tools')
    requireInside(appPath, destination)
    if (existsSync(destination)) rmSync(destination, { recursive: true })
    cpSync(join(staging, 'tools'), destination, { recursive: true, dereference: true })
    writeFileSync(join(destination, 'README-macos.txt'), [
      `INX 实施工作台 macOS ${targetArch} 随包工具`,
      '本目录仅包含支持当前 Mac 架构的 ADB、必要动态库及许可证。APK 信息由工作台自行解析，不依赖 Java 或 Rosetta。',
      '请保留完整应用包，不单独复制或替换工具文件。',
      '',
    ].join('\n'), 'utf8')

    // Resources/tools 不是标准嵌套代码目录，明确签每个 Mach-O，不依赖 --deep 发现工具。
    for (const binary of macosToolBinaries(destination)) {
      execute('/usr/bin/codesign', ['--force', '--sign', signingIdentity, binary])
    }
    // 工具签名会改变文件；先重算清单，再签应用外层，避免签后校验失效。
    tools(resources, '--refresh-manifest')
    execute('/usr/bin/codesign', ['--force', '--sign', signingIdentity, appPath])
    tools(resources, '--verify')
    execute('/usr/bin/codesign', ['--verify', '--deep', '--strict', appPath])
    log(`macOS ${targetArch} 完整应用包已生成：${appPath}`)
    return appPath
  } finally {
    requireInside(targetRoot, staging)
    rmSync(staging, { recursive: true, force: true })
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    buildMacos({ args: process.argv.slice(2) })
  } catch (error) {
    console.error(error instanceof Error ? error.message : error)
    process.exitCode = 1
  }
}
