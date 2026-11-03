import { describe, it, expect } from 'vitest'
import { buildZip, crc32 } from './zip'

const encoder = new TextEncoder()
const SIG_LOCAL = 0x04034b50
const SIG_CENTRAL = 0x02014b50
const SIG_EOCD = 0x06054b50

async function bytesOf(blob: Blob): Promise<Uint8Array> {
  return new Uint8Array(await blob.arrayBuffer())
}

/** 从 EOCD 反向定位中央目录，按需遍历条目，便于断言结构而非只比对字节 */
function readEndRecord(bytes: Uint8Array) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const sig = view.getUint32(bytes.length - 22, true)
  expect(sig).toBe(SIG_EOCD)
  return {
    entryCount: view.getUint16(bytes.length - 22 + 10, true),
    centralSize: view.getUint32(bytes.length - 22 + 12, true),
    centralOffset: view.getUint32(bytes.length - 22 + 16, true),
  }
}

/** 遍历中央目录，返回每项的（文件名, CRC, 压缩后长度, 本地头偏移） */
function readCentralDirectory(bytes: Uint8Array, offset: number, count: number) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const items: Array<{ name: string; crc: number; size: number; localOffset: number }> = []
  let p = offset
  for (let i = 0; i < count; i++) {
    expect(view.getUint32(p, true)).toBe(SIG_CENTRAL)
    const crc = view.getUint32(p + 16, true)
    const size = view.getUint32(p + 20, true)
    const nameLen = view.getUint16(p + 28, true)
    const localOffset = view.getUint32(p + 42, true)
    const name = new TextDecoder().decode(bytes.subarray(p + 46, p + 46 + nameLen))
    items.push({ name, crc, size, localOffset })
    p += 46 + nameLen
  }
  return items
}

describe('crc32', () => {
  it('与 zip 规范的标准校验值一致', () => {
    // "test" 是 CRC-32 教科书用例，值固定
    expect(crc32(encoder.encode('test'))).toBe(0xd87f7e0c)
    expect(crc32(new Uint8Array())).toBe(0)
  })
})

describe('buildZip', () => {
  it('生成 store 模式的双文件包，三处签名与条目数均正确', async () => {
    const entries = [
      { name: 'report.html', data: '<html>ok</html>' },
      { name: 'meta.json', data: '{"a":1}' },
    ]
    const bytes = await bytesOf(buildZip(entries, new Date(2026, 2, 1, 12, 0, 0)))

    const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
    expect(view.getUint32(0, true)).toBe(SIG_LOCAL)
    // 本地头：压缩方式 0（store）、UTF-8 文件名 flag
    expect(view.getUint16(8, true)).toBe(0)
    expect(view.getUint16(6, true)).toBe(0x0800)

    const end = readEndRecord(bytes)
    expect(end.entryCount).toBe(2)
    // 中央目录紧接本地区，长度应等于所有中央项之和
    const items = readCentralDirectory(bytes, end.centralOffset, end.entryCount)
    expect(items.map((i) => i.name)).toEqual(['report.html', 'meta.json'])
    expect(end.centralOffset + end.centralSize).toBe(bytes.length - 22)

    // 每个条目的 CRC 与长度要和原始数据对得上，且本地头偏移能指回 PK\x03\x04
    for (const [idx, item] of items.entries()) {
      const raw = entries[idx]!.data
      expect(item.crc).toBe(crc32(encoder.encode(raw)))
      expect(item.size).toBe(encoder.encode(raw).length)
      expect(view.getUint32(item.localOffset, true)).toBe(SIG_LOCAL)
    }
  })

  it('接受 Uint8Array 条目，二进制内容原样写入', async () => {
    const payload = new Uint8Array([0x00, 0xff, 0x10, 0x80])
    const bytes = await bytesOf(buildZip([{ name: 'a.bin', data: payload }]))
    const end = readEndRecord(bytes)
    const [item] = readCentralDirectory(bytes, end.centralOffset, end.entryCount)
    expect(item!.size).toBe(4)
    expect(item!.crc).toBe(crc32(payload))
    // 本地头 30B + 文件名 5B 之后就是数据
    expect(Array.from(bytes.subarray(item!.localOffset + 30 + 5, item!.localOffset + 30 + 9))).toEqual([0, 255, 16, 128])
  })

  it('空包只产出 EOCD，条目数为 0', async () => {
    const bytes = await bytesOf(buildZip([]))
    expect(bytes.length).toBe(22)
    expect(readEndRecord(bytes).entryCount).toBe(0)
  })
})
