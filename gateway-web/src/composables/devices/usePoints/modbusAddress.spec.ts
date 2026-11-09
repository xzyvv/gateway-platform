import { describe, it, expect } from 'vitest'
import { headOfFunctionCode, modbusAddressOf, parseModbusAddress } from './modbusAddress'

describe('modbusAddress（区段 ↔ 功能码 ↔ 0 基偏移）', () => {
  it('功能码 → 区段：写功能码也要归到可读区段', () => {
    expect(headOfFunctionCode(1)).toBe('0')
    expect(headOfFunctionCode(2)).toBe('1')
    expect(headOfFunctionCode(3)).toBe('3')
    expect(headOfFunctionCode(4)).toBe('4')
    expect(headOfFunctionCode(5)).toBe('0')
    expect(headOfFunctionCode(15)).toBe('0')
    expect(headOfFunctionCode(6)).toBe('3')
    expect(headOfFunctionCode(16)).toBe('3')
    // 缺省按保持寄存器处理（与后端 fc.unwrap_or(3) 一致）
    expect(headOfFunctionCode(null)).toBe('3')
  })

  it('功能码 + 0 基偏移 → 5 位展示地址', () => {
    expect(modbusAddressOf(4, 0)).toBe('40001')
    expect(modbusAddressOf(3, 10)).toBe('30011')
    expect(modbusAddressOf(1, 4)).toBe('00005')
    expect(modbusAddressOf(2, 0)).toBe('10001')
  })

  it('偏移超出 9998 时返回空串（5 位装不下，交给调用方退回手填）', () => {
    expect(modbusAddressOf(3, 9998)).toBe('39999')
    expect(modbusAddressOf(3, 9999)).toBe('')
    expect(modbusAddressOf(3, -1)).toBe('')
  })

  it('解析 5 位地址 → 功能码 + 0 基偏移', () => {
    expect(parseModbusAddress('40001')).toEqual({
      head: '4', offset: 1, functionCode: 4, addressNum: 0,
    })
    expect(parseModbusAddress('30011')?.addressNum).toBe(10)
    // 00000 按 1 号处理：后端 address_num 为 u16，不能出现 -1
    expect(parseModbusAddress('00000')?.addressNum).toBe(0)
  })

  it('非法地址返回 null（区段不在 0/1/3/4、长度不对、空值）', () => {
    expect(parseModbusAddress('')).toBeNull()
    expect(parseModbusAddress('20001')).toBeNull()
    expect(parseModbusAddress('4001')).toBeNull()
    expect(parseModbusAddress('400011')).toBeNull()
    expect(parseModbusAddress(null)).toBeNull()
  })
})
