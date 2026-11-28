/**
 * HEX 文本 ↔ 字节数组：全项目单一真源（DD-47）。
 *
 * 历史上三处各自实现，口径分裂：
 *   - hexDump.ts     ：非 hex / 奇数长度 → 返回 []（展示层要静默降级）
 *   - frameParser.ts ：奇数长度 → 抛错；非 hex → 产生 NaN（帧解析却拿到坏字节）
 *   - hexFrame.ts    ：只有 normalizeHex / isValidHex，没有字节转换
 *
 * 统一口径：**非法输入一律返回空数组**。理由：
 *   - 展示层（HexAsciiView）实时预览，抛错会打断渲染；
 *   - 解析层的三个 parse 函数本就有 `bytes.length < N` 检查，空数组会落到
 *     「帧长度不足」的合理降级，而且消除了原先非 hex 产生 NaN 的隐患；
 *   - 「去 0x 前缀」是宽容特性，保留（frameParser 原实现就有）。
 *
 * 对外符号保持兼容：hexFrame / hexDump 从本模块 re-export，调用方无需改动 import。
 */

/** 去掉所有空白与 0x 前缀（大小写不敏感），得到紧凑的 HEX 串 */
function compactHex(input: string): string {
  return (input ?? '').replace(/\s+/g, '').replace(/0x/gi, '')
}

/**
 * HEX 字符串 → 字节数组。
 *
 * 空串 / 奇数长度 / 含非 [0-9A-F] 字符 → 返回空数组（不抛错、不产生 NaN）。
 * 合法输入按两两一组解析，如 '01 03 0a ff' → [1, 3, 10, 255]。
 */
export function hexToBytes(hex: string): number[] {
  const clean = compactHex(hex)
  if (!clean || clean.length % 2 !== 0 || /[^0-9a-fA-F]/.test(clean)) return []
  const out: number[] = []
  for (let i = 0; i < clean.length; i += 2) out.push(Number.parseInt(clean.slice(i, i + 2), 16))
  return out
}

/** 展示/发送统一格式：去首尾空白、连续空白压成一个空格、转大写 */
export function normalizeHex(input: string): string {
  return input.trim().replace(/\s+/g, ' ').toUpperCase()
}

/**
 * 合法帧：去掉空格后为偶数长度且仅含 0-9A-F。
 * 空串视为不合法——调用方据此禁用「发送」按钮；奇数字节和非十六进制字符都必须在本地拦下。
 */
export function isValidHex(input: string): boolean {
  const compact = normalizeHex(input).replace(/ /g, '')
  return compact.length > 0 && compact.length % 2 === 0 && /^[0-9A-F]+$/.test(compact)
}

/**
 * 是否为 Modbus 写帧（FC05/06/15/16）。口径与后端 `judge_modbus` 的
 * `0x05|0x06|0x0F|0x10` 一致（DD-30）。仅对 Modbus RTU 帧结构成立。
 */
export function isWriteFrame(hex: string): boolean {
  const compact = normalizeHex(hex).replace(/ /g, '')
  const fc = Number.parseInt(compact.slice(2, 4), 16)
  return fc === 5 || fc === 6 || fc === 15 || fc === 16
}

/** 单字节 → ASCII：可打印字符原样，其余用 '.' */
export function byteToAscii(b: number): string {
  return b >= 0x20 && b <= 0x7e ? String.fromCharCode(b) : '.'
}
