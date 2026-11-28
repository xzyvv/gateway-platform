/**
 * HEX 转储工具（审查#7）：把「01 03 00 00」这类帧字符串整理成
 * 偏移 | HEX | ASCII 三栏行，供抓包/回放详情做双栏对照展示。
 */

import { byteToAscii, hexToBytes } from './hexCodec'

export interface HexDumpRow {
  /** 行首偏移（4 位大写 hex） */
  offset: string
  /** 本行字节的 HEX（大写两位） */
  bytes: string[]
  /** 本行字节的 ASCII 渲染：可打印字符原样，其余用 '.' */
  ascii: string
}

// DD-47：字节解析口径已收敛到 hexCodec.ts（全项目单一真源）。
// 本模块保留 re-export，对外符号与调用方 import 路径均不变。
export { hexToBytes, byteToAscii } from './hexCodec'

/** 按每行 perRow 字节切分，生成三栏行 */
export function hexDumpRows(hex: string, perRow = 16): HexDumpRow[] {
  const bytes = hexToBytes(hex)
  const rows: HexDumpRow[] = []
  for (let i = 0; i < bytes.length; i += perRow) {
    const slice = bytes.slice(i, i + perRow)
    rows.push({
      offset: i.toString(16).padStart(4, '0').toUpperCase(),
      bytes: slice.map((b) => b.toString(16).padStart(2, '0').toUpperCase()),
      ascii: slice.map(byteToAscii).join(''),
    })
  }
  return rows
}
