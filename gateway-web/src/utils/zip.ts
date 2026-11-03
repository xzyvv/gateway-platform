/**
 * 极简 ZIP 打包（store 模式，不压缩）
 *
 * 用途：一键诊断包需要一次下发多份文件（报告 HTML / 请求 HAR / meta JSON），
 * 逐个触发下载在浏览器里会被拦，必须打包成一个文件。
 *
 * 为什么自己写而不引入 jszip：只用到「把几份文本原样装进一个 zip」这一件事，
 * 引入 ~100KB 的通用压缩库不划算。store 模式（compression method 0）不需要实现
 * DEFLATE，只有 CRC32 + 三块固定结构的头部，约 100 行即可，且能完全单测。
 * 代价是体积不压缩——诊断包内容都是文本，稳赢的是「少一次依赖」。
 *
 * 兼容性：文件名按 UTF-8 写入并置 flag bit 11（0x0800），Windows 资源管理器、
 * macOS 归档工具、`unzip` 均可直接打开。
 */

export interface ZipEntry {
  /** 包内路径，使用 `/` 分隔（如 `logs/requests.har`） */
  name: string
  data: string | Uint8Array
}

const encoder = new TextEncoder()
/** CRC32 查表：首次使用时生成，之后复用 */
let crcTable: Uint32Array | null = null

function crcTableOf(): Uint32Array {
  if (crcTable) return crcTable
  const table = new Uint32Array(256)
  for (let i = 0; i < 256; i++) {
    let c = i
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
    table[i] = c >>> 0
  }
  crcTable = table
  return table
}

/** 标准 CRC-32（IEEE 802.3），与 zip 规范一致 */
export function crc32(bytes: Uint8Array): number {
  const table = crcTableOf()
  let c = 0xffffffff
  for (let i = 0; i < bytes.length; i++) {
    c = table[(c ^ bytes[i]!) & 0xff]! ^ (c >>> 8)
  }
  return (c ^ 0xffffffff) >>> 0
}

/** MS-DOS 时间格式：秒精度为 2s，年份基准 1980 */
function dosDateTime(d: Date): { time: number; date: number } {
  const time = (d.getHours() << 11) | (d.getMinutes() << 5) | Math.floor(d.getSeconds() / 2)
  const date = (((d.getFullYear() - 1980) & 0x7f) << 9) | ((d.getMonth() + 1) << 5) | d.getDate()
  return { time, date }
}

/**
 * 打包为 zip Blob。
 *
 * @param entries 包内文件（顺序即写入顺序）
 * @param mtime 所有条目的修改时间，默认当前时间
 */
export function buildZip(entries: ZipEntry[], mtime = new Date()): Blob {
  const { time, date } = dosDateTime(mtime)
  const chunks: Uint8Array[] = []
  const centrals: Uint8Array[] = []
  let offset = 0

  for (const entry of entries) {
    const nameBytes = encoder.encode(entry.name)
    const data = typeof entry.data === 'string' ? encoder.encode(entry.data) : entry.data
    const crc = crc32(data)
    const localOffset = offset

    // 本地文件头（30 字节）+ 文件名 + 数据
    const local = new Uint8Array(30)
    const lv = new DataView(local.buffer)
    lv.setUint32(0, 0x04034b50, true) // 签名 PK\x03\x04
    lv.setUint16(4, 20, true) // 解压所需版本 2.0
    lv.setUint16(6, 0x0800, true) // bit 11：文件名 UTF-8
    lv.setUint16(8, 0, true) // 压缩方式 0 = store
    lv.setUint16(10, time, true)
    lv.setUint16(12, date, true)
    lv.setUint32(14, crc, true)
    lv.setUint32(18, data.length, true)
    lv.setUint32(22, data.length, true) // store 模式两者相同
    lv.setUint16(26, nameBytes.length, true)
    lv.setUint16(28, 0, true) // extra 长度

    chunks.push(local, nameBytes, data)
    offset += local.length + nameBytes.length + data.length

    // 中央目录项（46 字节）+ 文件名
    const central = new Uint8Array(46)
    const cv = new DataView(central.buffer)
    cv.setUint32(0, 0x02014b50, true) // 签名 PK\x01\x02
    cv.setUint16(4, 20, true) // 制作版本
    cv.setUint16(6, 20, true) // 解压所需版本
    cv.setUint16(8, 0x0800, true)
    cv.setUint16(10, 0, true)
    cv.setUint16(12, time, true)
    cv.setUint16(14, date, true)
    cv.setUint32(16, crc, true)
    cv.setUint32(20, data.length, true)
    cv.setUint32(24, data.length, true)
    cv.setUint16(28, nameBytes.length, true)
    cv.setUint16(30, 0, true) // extra
    cv.setUint16(32, 0, true) // comment
    cv.setUint16(34, 0, true) // 起始磁盘号
    cv.setUint16(36, 0, true) // 内部属性
    cv.setUint32(38, 0, true) // 外部属性
    cv.setUint32(42, localOffset, true)
    centrals.push(central, nameBytes)
  }

  const centralOffset = offset
  let centralSize = 0
  for (const c of centrals) {
    chunks.push(c)
    centralSize += c.length
  }

  // 中央目录结束记录（22 字节）
  const end = new Uint8Array(22)
  const ev = new DataView(end.buffer)
  ev.setUint32(0, 0x06054b50, true) // 签名 PK\x05\x06
  ev.setUint16(4, 0, true) // 当前磁盘号
  ev.setUint16(6, 0, true) // 中央目录起始磁盘号
  ev.setUint16(8, entries.length, true)
  ev.setUint16(10, entries.length, true)
  ev.setUint32(12, centralSize, true)
  ev.setUint32(16, centralOffset, true)
  ev.setUint16(20, 0, true) // 注释长度
  chunks.push(end)

  return new Blob(chunks as BlobPart[], { type: 'application/zip' })
}
