import { Buffer } from 'node:buffer'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'

// 只重封装已有ICO中的PNG字节，不重新绘制、缩放或改变产品图标。
export function prepareMacIcons(projectRoot) {
  const ico = readFileSync(join(projectRoot, 'src-tauri/icons/icon.ico'))
  if (ico.length < 6 || ico.readUInt16LE(0) !== 0 || ico.readUInt16LE(2) !== 1) {
    throw new Error('应用ICO格式无效')
  }
  const count = ico.readUInt16LE(4)
  if (ico.length < 6 + count * 16) throw new Error('应用ICO目录不完整')
  let png
  for (let index = 0; index < count; index += 1) {
    const entry = 6 + index * 16
    const size = ico.readUInt32LE(entry + 8)
    const offset = ico.readUInt32LE(entry + 12)
    if (offset < 6 + count * 16 || offset + size > ico.length) throw new Error('应用ICO数据越界')
    const candidate = ico.subarray(offset, offset + size)
    if (ico[entry] === 0 && ico[entry + 1] === 0 && candidate.length >= 26 &&
        candidate.subarray(0, 8).equals(Buffer.from([137, 80, 78, 71, 13, 10, 26, 10])) &&
        candidate.readUInt32BE(16) === 256 && candidate.readUInt32BE(20) === 256 && candidate[25] === 6) {
      png = candidate
      break
    }
  }
  if (!png) throw new Error('应用ICO必须包含256×256 RGBA PNG，不能生成Mac图标')
  const header = Buffer.alloc(16)
  header.write('icns', 0, 'ascii')
  header.writeUInt32BE(16 + png.length, 4)
  header.write('ic08', 8, 'ascii')
  header.writeUInt32BE(8 + png.length, 12)
  const output = join(projectRoot, 'src-tauri/target/macos-assets')
  mkdirSync(output, { recursive: true })
  writeFileSync(join(output, 'icon.png'), png)
  writeFileSync(join(output, 'icon.icns'), Buffer.concat([header, png]))
  return output
}
