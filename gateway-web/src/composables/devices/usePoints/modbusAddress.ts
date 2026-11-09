/**
 * Modbus 地址推导
 *
 * 采集实际按 `address_num`（0 基寄存器偏移）+ `function_code` 寻址，
 * `address` 只是 5 位展示串。表单里两者必须联动，否则用户改地址却不生效。
 */

/** 地址区段首位数字 */
export type ModbusRegionHead = '0' | '1' | '3' | '4'

export interface ModbusRegion {
  /** 区段首位数字 */
  head: ModbusRegionHead
  /** 该区段的读功能码 */
  functionCode: number
  /** 是否支持写入（1xxxx 离散输入 / 4xxxx 输入寄存器为只读） */
  writable: boolean
}

export const MODBUS_REGIONS: Record<ModbusRegionHead, ModbusRegion> = {
  '0': { head: '0', functionCode: 1, writable: true },
  '1': { head: '1', functionCode: 2, writable: false },
  '3': { head: '3', functionCode: 3, writable: true },
  '4': { head: '4', functionCode: 4, writable: false },
}

/**
 * 功能码 → 区段首位数字。
 *
 * 写功能码（5/15 线圈、6/16 寄存器）在读路径上也必须归到对应区段，
 * 否则历史配置里存着 6/16 的点位打开编辑会显示成错误的区段。
 */
export function headOfFunctionCode(fc?: number | null): ModbusRegionHead {
  switch (fc) {
    case 1:
    case 5:
    case 15:
      return '0'
    case 2:
      return '1'
    case 4:
      return '4'
    default:
      return '3'
  }
}

/**
 * 由功能码 + 0 基寄存器偏移生成 5 位展示地址。
 *
 * 偏移超出 0..=9998（即 1 基地址 1..=9999）时返回空串——5 位串装不下，
 * 由调用方退回手填。
 */
export function modbusAddressOf(
  fc?: number | null,
  addressNum?: number | null,
): string {
  const offset = Math.trunc(addressNum ?? 0) + 1
  if (offset < 1 || offset > 9999) return ''
  return `${headOfFunctionCode(fc)}${String(offset).padStart(4, '0')}`
}

export interface ParsedModbusAddress {
  head: ModbusRegionHead
  /** 1 基地址序号（1..=9999） */
  offset: number
  /** 该区段的读功能码 */
  functionCode: number
  /** 0 基寄存器偏移（写入 address_num） */
  addressNum: number
}

/** 解析 5 位展示地址；非法串返回 null */
export function parseModbusAddress(addr?: string | null): ParsedModbusAddress | null {
  const s = (addr ?? '').trim()
  if (!/^[0134]\d{4}$/.test(s)) return null
  const head = s[0] as ModbusRegionHead
  const raw = parseInt(s.slice(1), 10)
  if (!Number.isFinite(raw) || raw < 0 || raw > 9999) return null
  // 0 号地址按 1 号处理：后端 address_num 为 u16，不能出现 -1
  const offset = Math.max(1, raw)
  return {
    head,
    offset,
    functionCode: MODBUS_REGIONS[head].functionCode,
    addressNum: offset - 1,
  }
}
