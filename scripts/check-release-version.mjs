import fs from 'node:fs'
import console from 'node:console'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const packageVersion = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version
const tauriVersion = JSON.parse(fs.readFileSync(path.join(root, 'src-tauri/tauri.conf.json'), 'utf8')).version
const cargo = fs.readFileSync(path.join(root, 'src-tauri/Cargo.toml'), 'utf8')
const cargoVersion = cargo.match(/^version\s*=\s*"([^"]+)"/m)?.[1]

if (!packageVersion || !tauriVersion || !cargoVersion) {
  throw new Error('无法读取package、Tauri或Cargo应用版本')
}
if (new Set([packageVersion, tauriVersion, cargoVersion]).size !== 1) {
  throw new Error(`应用版本不一致：package=${packageVersion}，tauri=${tauriVersion}，cargo=${cargoVersion}`)
}

const tag = process.argv[2]?.trim()
if (tag && tag !== `v${packageVersion}`) {
  throw new Error(`发布标签${tag}与应用版本${packageVersion}不一致，应使用v${packageVersion}`)
}

console.log(`应用版本校验通过：${packageVersion}${tag ? `，标签=${tag}` : ''}`)
