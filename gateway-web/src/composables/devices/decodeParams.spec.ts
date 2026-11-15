import { describe, it, expect, afterEach } from 'vitest'
import {
  BYTE_ORDERS,
  DATA_TYPES,
  applyByteOrders,
  applyDataTypes,
  usesNumericDecode,
} from './decodeParams'

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

/**
 * 三张下拉的取值集以 `GET /api/point-fields` 为准，前端常量只是兜底。
 * 这里钉住「后端新值不会被前端清单吃掉」——那正是之前字节序缺 2 个值
 * 导致下拉空白（配置里有值但选不中）的根因。
 */
describe('applyPointFields 后端字段字典', () => {
  const typesSnapshot = [...DATA_TYPES]
  const ordersSnapshot = BYTE_ORDERS.map((o) => ({ ...o }))

  afterEach(() => {
    DATA_TYPES.splice(0, DATA_TYPES.length, ...typesSnapshot)
    BYTE_ORDERS.splice(0, BYTE_ORDERS.length, ...ordersSnapshot.map((o) => ({ ...o })))
  })

  it('数据类型按后端顺序替换', () => {
    applyDataTypes(['u16', 'f32', 'f128'])
    expect(DATA_TYPES).toEqual(['u16', 'f32', 'f128'])
  })

  it('字节序已知值按本地顺序排列，后端新增值追加并保留原值作文案', () => {
    applyByteOrders(['new-order', 'ABCD', 'DCBA'])
    expect(BYTE_ORDERS.map((o) => o.value)).toEqual(['ABCD', 'DCBA', 'new-order'])
    expect(BYTE_ORDERS[0]!.label).toBe('ABCD（大端）')
    expect(BYTE_ORDERS[2]!.label).toBe('new-order')
  })

  it('空列表保留兜底，不清空可选项', () => {
    applyDataTypes([])
    applyByteOrders([])
    expect(DATA_TYPES).toEqual(typesSnapshot)
    expect(BYTE_ORDERS.map((o) => o.value)).toEqual(ordersSnapshot.map((o) => o.value))
  })
})
