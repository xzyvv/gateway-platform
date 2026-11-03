/**
 * HEX 转储工具（审查#7）：把「01 03 00 00」这类帧字符串整理成
 * 偏移 | HEX | ASCII 三栏行，供抓包/回放详情做双栏对照展示。
 */

export interface HexDumpRow {
  /** 行首偏移（4 位大写 hex） */
  offset: string
  /** 本行字节的 HEX（大写两位） */
  bytes: string[]
  /** 本行字节的 ASCII 渲染：可打印字符原样，其余用 '.' */
  ascii: string
}

/** 解析 HEX 字符串为字节数组；非法输入（空/奇数长度/含非 hex 字符）返回空数组 */
export function hexToBytes(hex: string): number[] {
  const clean = (hex || '').replace(/\s+/g, '')
  if (!clean || clean.length % 2 !== 0 || /[^0-9a-fA-F]/.test(clean)) return []
  const out: number[] = []
  for (let i = 0; i < clean.length; i += 2) out.push(parseInt(clean.slice(i, i + 2), 16))
  return out
}

export function byteToAscii(b: number): string {
  return b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : '.'
}

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
