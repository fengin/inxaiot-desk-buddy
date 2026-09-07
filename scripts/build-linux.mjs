import { spawnSync } from 'node:child_process'
import console from 'node:console'
import process from 'node:process'
import { fileURLToPath, URL } from 'node:url'

if (process.platform !== 'linux') {
  console.error('Linux 可执行文件必须在 Linux 上构建；Windows 请使用 pnpm build:windows。')
  process.exit(1)
}

const projectRoot = fileURLToPath(new URL('..', import.meta.url))
const result = spawnSync('pnpm', [
  'tauri', 'build', '--no-bundle'
], { cwd: projectRoot, stdio: 'inherit' })
if (result.error) {
  console.error('无法启动 Linux 构建，请检查 pnpm、Rust 和 Tauri 的 Linux 系统依赖。')
}
process.exit(result.status ?? 1)
