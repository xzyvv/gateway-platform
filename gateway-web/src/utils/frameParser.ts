/**
 * 协议帧解析工具
 * 支持 Modbus RTU / DLT645 / CJT188 帧解析
 */

import { extractErrorMessage } from './errorHandler'
import { decodeCjt188St, decodeDlt645Err } from './protocolErrorCodes'

export interface ParsedField {
  name: string
  value: string
  desc?: string
}

export interface ParseResult {
  protocol: string
  valid: boolean
  error?: string
  fields: ParsedField[]
  rawHex: string
}

function hexToBytes(hex: string): number[] {
  const clean = hex.replace(/\s+/g, '').replace(/0x/gi, '')
  if (clean.length % 2 !== 0) throw new Error('HEX 长度必须为偶数')
  const bytes: number[] = []
  for (let i = 0; i < clean.length; i += 2) {
    bytes.push(parseInt(clean.slice(i, i + 2), 16))
  }
  return bytes
}

function crc16Modbus(data: number[]): number {
  let crc = 0xFFFF
  for (const b of data) {
    crc ^= b
    for (let i = 0; i < 8; i++) {
      if (crc & 1) crc = (crc >> 1) ^ 0xA001
      else crc >>= 1
    }
  }
  return crc
}

/** Modbus RTU 解析 */
export function parseModbusRTU(hex: string): ParseResult {
  const fields: ParsedField[] = []
  try {
    const bytes = hexToBytes(hex)
    if (bytes.length < 5) return { protocol: 'modbus-rtu', valid: false, error: '帧长度不足（至少 5 字节）', fields, rawHex: hex }

    const slaveId = bytes[0]!
    const funcCode = bytes[1]!
    fields.push({ name: '从站地址', value: `${slaveId} (0x${slaveId.toString(16).padStart(2, '0')})`, desc: slaveId <= 247 ? '单播' : '广播' })

    const funcMap: Record<number, string> = {
      1: '读线圈', 2: '读离散输入', 3: '读保持寄存器', 4: '读输入寄存器',
      5: '写单个线圈', 6: '写单个寄存器', 15: '写多个线圈', 16: '写多个寄存器',
    }
    fields.push({ name: '功能码', value: `${funcCode} (0x${funcCode.toString(16).padStart(2, '0')})`, desc: funcMap[funcCode]! || '未知' })

    // CRC 校验
    const payload = bytes.slice(0, -2)
    const crcReceived = (bytes[bytes.length - 1]! << 8) | bytes[bytes.length - 2]!
    const crcCalc = crc16Modbus(payload)
    fields.push({ name: 'CRC 校验', value: crcReceived === crcCalc ? '通过' : '失败', desc: `收到: 0x${crcReceived.toString(16).padStart(4, '0')} 计算: 0x${crcCalc.toString(16).padStart(4, '0')}` })

    // 解析数据区
    if (bytes.length > 4) {
      // 帧内实际可用的数据字节数：去掉 地址/功能码/字节数（3 字节）与 CRC（2 字节）
      const dataBytesAvailable = Math.max(0, bytes.length - 5)
      if (funcCode === 3 || funcCode === 4) {
        // 读响应
        if (bytes.length >= 5) {
          const byteCount = bytes[2]!
          fields.push({ name: '字节数', value: `${byteCount}` })
          // 按帧实际长度裁剪：畸形帧的 byteCount 可达 255，直接按声明值遍历会越界读出伪造寄存器值
          const declaredRegs = Math.floor(byteCount / 2)
          const regCount = Math.min(declaredRegs, Math.floor(dataBytesAvailable / 2))
          if (regCount < declaredRegs) {
            fields.push({ name: '数据不足', value: `声明 ${declaredRegs} 个寄存器，帧内仅 ${regCount} 个`, desc: '帧被截断或字节数字段错误' })
          }
          fields.push({ name: '寄存器数', value: `${regCount}` })
          for (let i = 0; i < regCount; i++) {
            const val = (bytes[3 + i * 2]! << 8) | bytes[4 + i * 2]!
            fields.push({ name: `寄存器[${i}]`, value: `${val} (0x${val.toString(16).padStart(4, '0')})` })
          }
        }
      } else if (funcCode === 1 || funcCode === 2) {
        const byteCount = bytes[2]!
        fields.push({ name: '字节数', value: `${byteCount}` })
        const count = Math.min(byteCount, dataBytesAvailable)
        if (count < byteCount) {
          fields.push({ name: '数据不足', value: `声明 ${byteCount} 字节，帧内仅 ${count} 字节`, desc: '帧被截断或字节数字段错误' })
        }
        for (let i = 0; i < count; i++) {
          const b = bytes[3 + i]!
          fields.push({ name: `线圈/离散[${i * 8}-${i * 8 + 7}]`, value: b.toString(2).padStart(8, '0') })
        }
      } else if (funcCode === 5 || funcCode === 6) {
        if (bytes.length < 6) {
          fields.push({ name: '数据不足', value: `帧长度 ${bytes.length} 字节`, desc: '写单个线圈/寄存器需要 6 字节' })
        } else {
          const addr = (bytes[2]! << 8) | bytes[3]!
          const val = (bytes[4]! << 8) | bytes[5]!
          fields.push({ name: '地址', value: `${addr}` })
          fields.push({ name: '值', value: `${val} (0x${val.toString(16).padStart(4, '0')})` })
        }
      } else if (funcCode === 15 || funcCode === 16) {
        if (bytes.length < 6) {
          fields.push({ name: '数据不足', value: `帧长度 ${bytes.length} 字节`, desc: '写多个线圈/寄存器需要至少 6 字节' })
        } else {
          const addr = (bytes[2]! << 8) | bytes[3]!
          const qty = (bytes[4]! << 8) | bytes[5]!
          fields.push({ name: '起始地址', value: `${addr}` })
          fields.push({ name: '数量', value: `${qty}` })
        }
      }
    }

    return { protocol: 'modbus-rtu', valid: crcReceived === crcCalc, fields, rawHex: hex }
  } catch (e) {
    return { protocol: 'modbus-rtu', valid: false, error: extractErrorMessage(e, '解析失败'), fields, rawHex: hex }
  }
}

/** DLT645 解析 */
export function parseDLT645(hex: string): ParseResult {
  const fields: ParsedField[] = []
  try {
    const bytes = hexToBytes(hex)
    if (bytes.length < 12) return { protocol: 'dlt645', valid: false, error: '帧长度不足', fields, rawHex: hex }

    if (bytes[0]! !== 0x68) return { protocol: 'dlt645', valid: false, error: '帧头错误（应为 0x68）', fields, rawHex: hex }

    // 地址域（6 字节，BCD 码，低字节在前）
    const addrBytes = bytes.slice(1, 7).reverse()
    const addr = addrBytes.map((b) => b.toString(16).padStart(2, '0')).join('')
    fields.push({ name: '设备地址', value: addr, desc: 'BCD 编码' })

    if (bytes[7]! !== 0x68) return { protocol: 'dlt645', valid: false, error: '第二个帧头错误', fields, rawHex: hex }

    const ctrl = bytes[8]!
    const isResponse = (ctrl & 0x80) !== 0
    // D6 位 = 从站异常应答标志（0 正常应答 / 1 异常应答）
    const isAbnormal = (ctrl & 0x40) !== 0
    fields.push({
      name: '控制码',
      value: `0x${ctrl.toString(16).padStart(2, '0')}`,
      desc: isResponse ? (isAbnormal ? '从站异常应答帧' : '从站正常应答帧') : '主站请求帧',
    })

    const dataLen = bytes[9]!
    fields.push({ name: '数据长度', value: `${dataLen}` })

    if (bytes.length >= 10 + dataLen + 2) {
      const data = bytes.slice(10, 10 + dataLen)

      // 异常应答帧：L=01H 时数据域唯一字节即为错误信息字 ERR（位标志，未加 33H）
      if (isResponse && isAbnormal && dataLen === 1) {
        const errByte = data[0]!
        const bits = decodeDlt645Err(errByte)
        fields.push({
          name: '错误信息字 ERR',
          value: `0x${errByte.toString(16).padStart(2, '0')}`,
          desc: `位标志（0x${errByte.toString(2).padStart(8, '0')}）`,
        })
        // 逐条列出置位的错误位：ERR 可同时表示多个错误
        for (const b of bits) {
          fields.push({
            name: `ERR bit${b.bit}`,
            value: b.name,
            desc: b.meaning,
          })
        }
        if (!bits.length) {
          fields.push({ name: '错误详情', value: '未置任何已知错误位', desc: '属规范未定义情形，归为其他错误' })
        }
      }
      // 数据域每个字节减 0x33
      const decoded = data.map((b) => (b - 0x33 + 256) % 256)
      fields.push({ name: '数据域(原始)', value: data.map((b) => b.toString(16).padStart(2, '0')).join(' ') })
      fields.push({ name: '数据域(减33H)', value: decoded.map((b) => b.toString(16).padStart(2, '0')).join(' ') })

      // 校验和
      const checksum = bytes.reduce((s, b, i) => i < bytes.length - 2 ? s + b : s, 0) & 0xFF
      const csReceived = bytes[bytes.length - 2]!
      fields.push({ name: '校验和', value: checksum === csReceived ? '通过' : '失败', desc: `收到: 0x${csReceived.toString(16).padStart(2, '0')} 计算: 0x${checksum.toString(16).padStart(2, '0')}` })

      if (bytes[bytes.length - 1]! !== 0x16) {
        fields.push({ name: '帧尾', value: '错误', desc: '应为 0x16' })
      } else {
        fields.push({ name: '帧尾', value: '0x16', desc: '正确' })
      }
    }

    return { protocol: 'dlt645', valid: true, fields, rawHex: hex }
  } catch (e) {
    return { protocol: 'dlt645', valid: false, error: extractErrorMessage(e, '解析失败'), fields, rawHex: hex }
  }
}

/** CJT188 解析 */
export function parseCJT188(hex: string): ParseResult {
  const fields: ParsedField[] = []
  try {
    const bytes = hexToBytes(hex)
    if (bytes.length < 11) return { protocol: 'cjt188', valid: false, error: '帧长度不足', fields, rawHex: hex }

    if (bytes[0]! !== 0x68) return { protocol: 'cjt188', valid: false, error: '帧头错误（应为 0x68）', fields, rawHex: hex }

    const meterType = bytes[1]!
    const typeMap: Record<number, string> = { 0x10: '冷水表', 0x11: '热水表', 0x12: '直饮水表', 0x20: '热量表' }
    fields.push({ name: '仪表类型', value: `0x${meterType.toString(16).padStart(2, '0')}`, desc: typeMap[meterType]! || '未知' })

    // 地址域 7 字节
    const addrBytes = bytes.slice(2, 9).reverse()
    const addr = addrBytes.map((b) => b.toString(16).padStart(2, '0')).join('')
    fields.push({ name: '仪表地址', value: addr })

    const ctrl = bytes[9]!
    // D7 = 传送方向（0 主站发出 / 1 从站应答）；D6 = 通讯正常(0) / 异常(1)
    const isResponse = (ctrl & 0x80) !== 0
    const isAbnormal = (ctrl & 0x40) !== 0
    fields.push({
      name: '控制码',
      value: `0x${ctrl.toString(16).padStart(2, '0')}`,
      desc: isResponse ? (isAbnormal ? '从站异常应答帧' : '从站正常应答帧') : '主站请求帧',
    })

    const dataLen = bytes[10]!
    fields.push({ name: '数据长度', value: `${dataLen}` })

    if (bytes.length >= 11 + dataLen + 2) {
      const data = bytes.slice(11, 11 + dataLen)

      // 异常应答帧：数据域 = SER(1B) + 状态ST(2B)，L=03H
      // 正常应答帧：数据域末尾 2 字节也是状态 ST（读计量数据时）
      let stBytes: number[] | null = null
      if (isResponse && isAbnormal && dataLen >= 3) {
        stBytes = data.slice(dataLen - 2)
        fields.push({ name: '序列号 SER', value: `0x${data[0]!.toString(16).padStart(2, '0')}` })
      } else if (isResponse && !isAbnormal && dataLen >= 2) {
        // 正常应答的状态 ST 在数据域末尾（具体位置随数据标识不同，
        // 此处取末尾两字节作为尽力解读）
        stBytes = data.slice(dataLen - 2)
      }

      if (stBytes) {
        const { bits, diagnostics } = decodeCjt188St(stBytes)
        fields.push({
          name: '状态 ST',
          value: stBytes.map((b) => b.toString(16).padStart(2, '0')).join(' '),
          desc: '第1字节位标志 + 第2字节诊断码 JD',
        })
        for (const b of bits) {
          fields.push({ name: `ST bit${b.bit}`, value: b.name, desc: b.meaning })
        }
        for (const d of diagnostics) {
          fields.push({ name: `诊断码 JD ${d.code}`, value: d.name, desc: d.meaning })
        }
        if (!bits.length && !diagnostics.length) {
          fields.push({ name: '状态解读', value: '阀门/电池正常，诊断信息码正常' })
        }
      }

      const checksum = bytes.reduce((s, b, i) => i < bytes.length - 2 ? s + b : s, 0) & 0xFF
      const csReceived = bytes[bytes.length - 2]!
      fields.push({ name: '校验和', value: checksum === csReceived ? '通过' : '失败' })
      if (bytes[bytes.length - 1]! === 0x16) fields.push({ name: '帧尾', value: '0x16 正确' })
    }

    return { protocol: 'cjt188', valid: true, fields, rawHex: hex }
  } catch (e) {
    return { protocol: 'cjt188', valid: false, error: extractErrorMessage(e, '解析失败'), fields, rawHex: hex }
  }
}

export type ParseProtocol = 'modbus-rtu' | 'dlt645' | 'cjt188'

/**
 * 总线协议名 → 帧解析内核的协议 key（C4：离线样本与在线响应共用一套内核）。
 *
 * 只映射帧结构与 RTU 一致的总线：
 * - `modbus-tcp` 帧是 MBAP 头 + PDU，与 `parseModbusRTU` 的「从站地址 + 功能码 + CRC」
 *   结构不同，映射过去会把事务号当从站地址，故返回 null（由页面用后端 `parsed` 文案兜底）。
 * - 其它协议（s7comm / opcua 等）同理返回 null，不假装能解析。
 */
export function inferParseProtocol(busProtocol: string | undefined | null): ParseProtocol | null {
  const p = (busProtocol ?? '').trim().toLowerCase()
  if (p === 'modbus' || p.startsWith('modbus-rtu')) return 'modbus-rtu'
  if (p.startsWith('dlt645')) return 'dlt645'
  if (p.startsWith('cjt188')) return 'cjt188'
  return null
}

/** 解析结果 → 纯文本：研发复现问题时需要「原始帧 + 逐字段解析」一起看 */
export function parseResultToText(r: ParseResult): string {
  const lines = [
    `协议: ${r.protocol.toUpperCase()}`,
    `校验: ${r.valid ? '通过' : '失败'}`,
    `原始HEX: ${r.rawHex}`,
  ]
  for (const f of r.fields) lines.push(`${f.name}: ${f.value}${f.desc ? `（${f.desc}）` : ''}`)
  if (r.error) lines.push(`错误: ${r.error}`)
  return lines.join('\n')
}

export function parseFrame(protocol: ParseProtocol, hex: string): ParseResult {
  switch (protocol) {
    case 'modbus-rtu': return parseModbusRTU(hex)
    case 'dlt645': return parseDLT645(hex)
    case 'cjt188': return parseCJT188(hex)
  }
}

export const sampleFrames = {
  'modbus-rtu': '01 03 00 00 00 0A C5 CD',
  'dlt645': '68 AA AA AA AA AA AA 68 11 04 33 33 33 33 0E 16',
  'cjt188': '68 10 AA AA AA AA AA AA AA 01 03 00 00 00 16',
}

/**
 * 异常应答样例帧：用于演示错误码解读。
 *
 * - DLT645：`68 ... 68 D1 01 ERR CS 16`，控制码 D1H 的 D6 位=1（异常应答），
 *   L=01H，数据域为 ERR 位标志。这里取 ERR=0x05（bit0 其他错误 + bit2 密码错/未授权）
 * - CJT188：`68 10 ... C1 03 SER ST0 ST1 CS 16`，控制码 C1H 的 D6 位=1，
 *   L=03H，数据域 = SER(0x01) + ST(00 04)，ST 第一字节=0x00（阀门/电池正常），
 *   第二字节 JD=0x04（电池电压低于 3.3V，必须更换电池）
 */
export const sampleErrorFrames = {
  'dlt645': '68 AA AA AA AA AA AA 68 D1 01 05 A3 16',
  'cjt188': '68 10 AA AA AA AA AA AA AA C1 03 01 00 04 E7 16',
}
