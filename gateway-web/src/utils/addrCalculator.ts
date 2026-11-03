/**
 * Modbus 地址计算器（协议调试工具）
 *
 * 把 5 位「区地址」换算成 0-based 系统地址，并生成对应的 Modbus RTU
 * 请求报文（含真实 CRC16）。
 * 注意：listFuncCodes() 在页面中为同步调用，不能改成返回 Promise。
 */
export interface FuncCodeItem {
  code: string
  name: string
}

export interface CalcResult {
  /** 用户输入的十进制区地址，如 40001 */
  manualAddr: string
  /** 换算后的 0-based 系统地址 */
  systemAddr: string
  funcCode: string
  funcName: string
  /** Modbus RTU 请求帧（十六进制，含 CRC） */
  requestHex: string
  note: string
}

interface Zone {
  base: number
  label: string
}

/** 5 位区地址约定：区前缀 → (0-based 换算基数, 区说明) */
const ZONES: Record<string, Zone> = {
  '0': { base: 1, label: '线圈（0xxxx）' },
  '1': { base: 10001, label: '离散量输入（1xxxx）' },
  '3': { base: 30001, label: '输入寄存器（3xxxx）' },
  '4': { base: 40001, label: '保持寄存器（4xxxx）' },
}

/** 功能码 → 所属地址区前缀 */
const FC_ZONE: Record<string, string> = {
  '01': '0',
  '05': '0',
  '15': '0',
  '02': '1',
  '04': '3',
  '03': '4',
  '06': '4',
  '16': '4',
}

const FUNC_CODES: FuncCodeItem[] = [
  { code: '01', name: '读线圈' },
  { code: '02', name: '读离散量输入' },
  { code: '03', name: '读保持寄存器' },
  { code: '04', name: '读输入寄存器' },
  { code: '05', name: '写单线圈' },
  { code: '06', name: '写单保持寄存器' },
  { code: '15', name: '写多线圈' },
  { code: '16', name: '写多保持寄存器' },
]

/** Modbus RTU CRC16（多项式 0xA001） */
function crc16(bytes: number[]): number {
  let crc = 0xffff
  for (const b of bytes) {
    crc ^= b
    for (let i = 0; i < 8; i++) crc = crc & 1 ? (crc >> 1) ^ 0xa001 : crc >> 1
  }
  return crc & 0xffff
}

function toHex(bytes: number[]): string {
  return bytes.map((b) => b.toString(16).padStart(2, '0')).join(' ').toUpperCase()
}

export function listFuncCodes(): FuncCodeItem[] {
  return FUNC_CODES.map((f) => ({ ...f }))
}

export async function calculate(
  manualAddr: string,
  funcCode: string,
  count: number,
  slaveId: number,
): Promise<CalcResult> {
  await new Promise((r) => setTimeout(r, 100))

  const fc = FUNC_CODES.find((f) => f.code === funcCode)
  if (!fc) throw new Error(`不支持的功能码：${funcCode}`)

  const raw = manualAddr.trim()
  if (!/^\d+$/.test(raw)) {
    throw new Error('地址格式无效，请输入如 40001 的十进制地址')
  }
  // 必须按原始字符串判长度：parseInt('00001') → 1，会把 0xxxx/1xxxx 区的
  // 前导零吃掉，导致合法的 5 位区地址被判成「位数不足」而无法计算。
  const digits = raw
  if (digits.length !== 5 || !(digits[0]! in ZONES)) {
    throw new Error('请输入 5 位区地址，区前缀须为 0/1/3/4（如 00001、10001、30001、40001）')
  }
  const n = Number.parseInt(digits, 10)

  const zonePrefix = digits[0]!
  const expected = FC_ZONE[fc.code]!
  if (zonePrefix !== expected) {
    throw new Error(
      `功能码 ${fc.code}（${fc.name}）应使用${ZONES[expected]!.label}地址，当前输入属于${ZONES[zonePrefix]!.label}`,
    )
  }

  const zone = ZONES[zonePrefix]!
  const systemAddr = n - zone.base

  // 组 RTU 请求帧：从站号 + 功能码 + 起始地址(2B 大端) + 数量(2B 大端) + CRC16(低字节在前)
  const payload = [
    slaveId & 0xff,
    Number.parseInt(fc.code, 16),
    (systemAddr >> 8) & 0xff,
    systemAddr & 0xff,
    (count >> 8) & 0xff,
    count & 0xff,
  ]
  const crc = crc16(payload)
  const requestHex = toHex([...payload, crc & 0xff, (crc >> 8) & 0xff])

  return {
    manualAddr: String(n),
    systemAddr: String(systemAddr),
    funcCode: fc.code,
    funcName: fc.name,
    requestHex,
    note: `${zone.label} 地址 ${n} 已换算为 0-based 系统地址 ${systemAddr}；报文按 Modbus RTU 组帧（含 CRC16 校验），仅作调试参考`,
  }
}
