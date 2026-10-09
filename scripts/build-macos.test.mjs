import assert from 'node:assert/strict'
import { Buffer } from 'node:buffer'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, isAbsolute, join, relative, sep } from 'node:path'
import { fileURLToPath, URL } from 'node:url'
import { test } from 'node:test'
import { buildMacos } from './build-macos.mjs'

const fixtureRoot = fileURLToPath(new URL('../.review-tools/macos-build-tests/', import.meta.url))
mkdirSync(fixtureRoot, { recursive: true })

const binaryFiles = ['android/adb', 'android/lib64/libadb.dylib']
const signatureSteps = binaryFiles.map(file => `sign-tool:${file}`)
function machO(arch) {
  const bytes = Buffer.alloc(64)
  bytes.writeUInt32LE(0xfeedfacf, 0)
  bytes.writeUInt32LE(arch === 'arm64' ? 0x0100000c : 0x01000007, 4)
  return bytes
}

function fixture(t, arch = 'arm64', failAt = '') {
  const projectRoot = mkdtempSync(join(fixtureRoot, '中文 构建-'))
  const app = join(projectRoot, 'src-tauri/target/release/bundle/macos/INX 实施工作台.app')
  const resources = join(app, 'Contents/Resources')
  mkdirSync(join(app, 'Contents/MacOS'), { recursive: true })
  mkdirSync(join(resources, 'tools'), { recursive: true })
  writeFileSync(join(app, 'Contents/MacOS/inxaiot-desk-buddy'), 'mock app')
  writeFileSync(join(resources, 'tools/old-windows-tool.exe'), 'old artifact')
  writeFileSync(join(projectRoot, 'src-tauri/tauri.conf.json'), JSON.stringify({ productName: 'INX 实施工作台' }))
  const steps = []
  const calls = []
  const messages = []
  const signed = []
  let refreshDone = false
  const run = (file, args, options) => {
    calls.push({ file, args, options })
    let step
    if (args[0] === 'scripts/package-screen-tools.mjs') {
      assert.deepEqual(args.slice(1, 5), ['--platform', 'darwin', '--arch', arch])
      const output = args[args.indexOf('--output') + 1]
      step = args.includes('--refresh-manifest') ? 'manifest' : args.includes('--verify') ? 'tools-verify' : 'prepare'
      if (step === 'prepare' && step !== failAt) {
        for (const name of binaryFiles) {
          const binary = join(output, 'tools', name)
          mkdirSync(dirname(binary), { recursive: true })
          writeFileSync(binary, machO(arch))
        }
        writeFileSync(join(output, 'tools/android/NOTICE.txt'), 'license text')
        writeFileSync(join(output, 'tools/android-manifest.json'), JSON.stringify({ versions: { os:'darwin', architecture:arch } }))
      } else if (step === 'manifest') {
        assert.deepEqual([...signed].sort(), [...binaryFiles].sort())
        for (const name of binaryFiles) assert.equal(readFileSync(join(output, 'tools', name))[32], 1)
        refreshDone = true
      } else if (step === 'tools-verify') {
        assert.equal(refreshDone, true)
        assert.equal(existsSync(join(output, 'tools/old-windows-tool.exe')), false)
      }
    } else if (file === 'pnpm') {
      step = 'build'
    } else if (file === '/usr/bin/lipo') {
      step = 'arch'
    } else if (file === '/usr/bin/codesign') {
      if (args.includes('--verify')) {
        step = 'signature-verify'
        assert.deepEqual(args, ['--verify', '--deep', '--strict', app])
      } else if (args.at(-1) === app) {
        step = 'sign-outer'
        assert.equal(refreshDone, true)
      } else {
        const target = args.at(-1)
        const name = relative(join(resources, 'tools'), target).split(sep).join('/')
        assert.ok(binaryFiles.includes(name), `只签名 Mach-O 文件，不应签名资源：${name}`)
        assert.equal(args.includes('--deep'), false)
        step = `sign-tool:${name}`
        if (step !== failAt) {
          const bytes = readFileSync(target)
          bytes[32] = 1
          writeFileSync(target, bytes)
          signed.push(name)
        }
      }
    } else throw new Error(`非预期命令：${file}`)
    steps.push(step)
    return { status: step === failAt ? 1 : 0, stdout: file === '/usr/bin/lipo' ? arch === 'x64' ? 'x86_64\n' : 'arm64\n' : '' }
  }
  t.after(() => {
    const resolved = realpathSync(projectRoot)
    const within = relative(realpathSync(fixtureRoot), resolved)
    assert.ok(within && !within.startsWith('..') && !isAbsolute(within))
    rmSync(resolved, { recursive: true })
  })
  return {
    projectRoot, app, resources, steps, calls, messages, signed,
    options: { platform: 'darwin', arch, env: {}, projectRoot, node: 'mock-node', run, log: value => messages.push(value) },
  }
}

for (const arch of ['arm64', 'x64']) {
  test(`macOS ${arch} 完整包先准备工具，内部签名后更新清单，再完成外层签名与验证`, t => {
    const f = fixture(t, arch)
    assert.equal(buildMacos({ ...f.options, args: ['--arch', arch] }), f.app)
    assert.deepEqual(f.steps.slice(0, 3), ['prepare', 'build', 'arch'])
    assert.deepEqual(f.steps.slice(3, 3 + binaryFiles.length).sort(), [...signatureSteps].sort())
    assert.deepEqual(f.steps.slice(3 + binaryFiles.length), ['manifest', 'sign-outer', 'tools-verify', 'signature-verify'])
    assert.equal(f.signed[0], 'android/lib64/libadb.dylib')
    assert.equal(f.calls.filter(call => call.file === '/usr/bin/codesign' && call.args.includes('--sign') && call.args.at(-1) === f.app).length, 1)
    assert.equal(f.calls.every(call => call.options.env.APPLE_SIGNING_IDENTITY === '-'), true)
    assert.equal(f.calls.every(call => call.options.cwd === f.projectRoot && !call.options.shell), true)
    assert.match(f.messages.at(-1), /完整应用包已生成/)
    const note = readFileSync(join(f.resources, 'tools/README-macos.txt'), 'utf8')
    assert.equal(note.includes('不依赖 Java 或 Rosetta'), true)
    assert.equal(existsSync(join(f.resources, 'tools/java')), false)
    assert.equal(existsSync(join(f.resources, 'tools/android-build')), false)
    assert.equal(readdirSync(join(f.projectRoot, 'src-tauri/target')).some(name => name.startsWith('macos-tools-')), false)
  })
}

test('指定签名身份用于 Tauri 构建及最终签名', t => {
  const f = fixture(t)
  buildMacos({ ...f.options, env: { APPLE_SIGNING_IDENTITY: 'Developer ID Application: Test' } })
  assert.equal(f.calls.every(call => call.options.env.APPLE_SIGNING_IDENTITY === 'Developer ID Application: Test'), true)
  const signatures = f.calls.filter(call => call.file === '/usr/bin/codesign' && call.args.includes('--sign'))
  assert.equal(signatures.every(call => call.args[call.args.indexOf('--sign') + 1] === 'Developer ID Application: Test'), true)
})

for (const step of ['prepare', 'build', ...signatureSteps, 'manifest', 'sign-outer', 'tools-verify', 'signature-verify']) {
  test(`${step} 失败时立即停止，不报告完成并清理本次暂存工具`, t => {
    const f = fixture(t, 'arm64', step)
    assert.throws(() => buildMacos(f.options), /macOS 构建步骤失败/)
    assert.equal(f.steps.at(-1), step)
    assert.equal(f.messages.some(message => message.includes('完整应用包已生成')), false)
    assert.equal(readdirSync(join(f.projectRoot, 'src-tauri/target')).some(name => name.startsWith('macos-tools-')), false)
  })
}

test('最终应用架构与目标不一致时不复制或签名工具', t => {
  const f = fixture(t)
  const run = (file, args, options) => file === '/usr/bin/lipo' ? { status: 0, stdout: 'x86_64' } : f.options.run(file, args, options)
  assert.throws(() => buildMacos({ ...f.options, run }), /应用架构不匹配/)
  assert.equal(f.signed.length, 0)
  assert.equal(existsSync(join(f.resources, 'tools/old-windows-tool.exe')), true)
})

test('拒绝 Windows、不同目标架构和覆盖输出目录的构建参数', () => {
  const run = () => assert.fail('前置校验失败时不应执行构建命令')
  assert.throws(() => buildMacos({ platform: 'win32', run }), /必须在 Mac/)
  assert.throws(() => buildMacos({ platform: 'darwin', arch: 'x64', args: ['--arch', 'arm64'], run }), /对应架构/)
  assert.throws(() => buildMacos({ platform: 'darwin', arch: 'arm64', args: ['--target', 'x86_64-apple-darwin'], run }), /仅支持/)
  assert.throws(() => buildMacos({ platform: 'darwin', arch: 'arm64', env: { CARGO_BUILD_TARGET: 'x86_64-apple-darwin' }, run }), /CARGO_BUILD_TARGET/)
  assert.throws(() => buildMacos({ platform: 'darwin', arch: 'arm64', env: { CARGO_TARGET_DIR: 'other' }, run }), /CARGO_TARGET_DIR/)
})
