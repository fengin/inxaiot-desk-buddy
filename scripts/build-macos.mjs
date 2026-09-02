import { spawnSync } from 'node:child_process'
import console from 'node:console'
import process from 'node:process'
import { fileURLToPath, URL } from 'node:url'
import { prepareMacIcons } from './macos-icons.mjs'

if (process.platform !== 'darwin') {
  console.error('macOS .app 必须在 Mac 上构建；Windows 请使用 pnpm build:windows。')
  process.exit(1)
}

const projectRoot = fileURLToPath(new URL('..', import.meta.url))
prepareMacIcons(projectRoot)
const result = spawnSync('pnpm', [
  'tauri', 'build', '--bundles', 'app', '--config', 'src-tauri/tauri.macos.conf.json',
], { cwd: projectRoot, stdio: 'inherit' })
if (result.error) {
  console.error('无法启动 macOS 构建，请检查 pnpm 和 Tauri 构建依赖。')
}
process.exit(result.status ?? 1)
