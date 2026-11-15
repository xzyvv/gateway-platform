import { describe, it, expect } from 'vitest'
import { BYTE_ORDERS, DATA_TYPES, usesNumericDecode } from './decodeParams'

/**
 * 数值解码族的判定口径必须与后端一致：
 * `gateway-core/src/config/validate/field.rs::uses_numeric_decode`。
 * 前端若少认一个协议，对应点位的「数据类型 / 字节序」在表单里被藏起来，
 * 用户只能绕 CSV 去填，编辑时还会被后端无条件覆盖。
 */
describe('decodeParams 数值解码族', () => {
  it('modbus / mc / fins / dlt645 / cjt188 需要数据类型与字节序', () => {
    for (const p of ['modbus', 'mc', 'fins', 'dlt645', 'cjt188']) {
      expect(usesNumericDecode(p)).toBe(true)
    }
  })

  it('大小写与首尾空白不影响判定', () => {
    expect(usesNumericDecode('DLT645')).toBe(true)
    expect(usesNumericDecode('  Cjt188 ')).toBe(true)
  })

  it('自带独立 data_type 语义的协议不套用统一解码', () => {
    for (const p of ['iec104', 'iec101', 's7comm', 'bacnet', 'opcua', 'snmp']) {
      expect(usesNumericDecode(p)).toBe(false)
    }
  })

  it('空值不判定为数值解码族', () => {
    expect(usesNumericDecode(undefined)).toBe(false)
    expect(usesNumericDecode(null)).toBe(false)
    expect(usesNumericDecode('')).toBe(false)
  })

  it('字节序补齐到后端 8 个合法值', () => {
    expect(BYTE_ORDERS.map((b) => b.value)).toEqual([
      'ABCD', 'DCBA', 'CDAB', 'BADC',
      'big-endian', 'little-endian',
      'big-endian-byte-swapped', 'little-endian-byte-swapped',
    ])
  })

  it('数据类型只用规范名，不含别名', () => {
    expect(DATA_TYPES).toEqual(['u16', 'i16', 'u32', 'i32', 'u64', 'i64', 'f32', 'f64'])
  })
})
