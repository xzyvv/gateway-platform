import { describe, it, expect } from 'vitest'
import { hexToBytes, hexDumpRows, byteToAscii } from './hexDump'

describe('hexToBytes', () => {
  it('解析空格分隔 hex，大小写均可', () => {
    expect(hexToBytes('01 03 0a ff')).toEqual([1, 3, 10, 255])
  })
  it('容忍多余空白与换行', () => {
    expect(hexToBytes('  01  03\n00 ')).toEqual([1, 3, 0])
  })
  it('非法输入返回空数组', () => {
    expect(hexToBytes('')).toEqual([])
    expect(hexToBytes('01 0')).toEqual([]) // 奇数长度
    expect(hexToBytes('01 GG')).toEqual([]) // 非 hex 字符
  })
})

describe('hexDumpRows', () => {
  it('默认按 16 字节切行并生成偏移', () => {
    const hex = Array.from({ length: 17 }, (_, i) => i.toString(16).padStart(2, '0')).join(' ')
    const rows = hexDumpRows(hex)
    expect(rows).toHaveLength(2)
    expect(rows[0]!.offset).toBe('0000')
    expect(rows[0]!.bytes).toHaveLength(16)
    expect(rows[1]!.offset).toBe('0010')
    expect(rows[1]!.bytes).toEqual(['10'])
  })
  it('ASCII 列：可打印原样，不可打印用 .', () => {
    // 0x48 0x65 0x6C 0x6C 0x6F = "Hello"；0x00 / 0xFF 不可打印
    const rows = hexDumpRows('48 65 6C 6C 6F 00 FF')
    expect(rows[0]!.ascii).toBe('Hello..')
  })
  it('HEX 统一大写；空输入无行；支持自定义每行字节数', () => {
    expect(hexDumpRows('ab cd')[0]!.bytes).toEqual(['AB', 'CD'])
    expect(hexDumpRows('')).toEqual([])
    const rows = hexDumpRows('01 02 03 04', 2)
    expect(rows.map((r) => r.offset)).toEqual(['0000', '0002'])
  })
  it('byteToAscii 边界：0x20/0x7E 可打印，之外为 .', () => {
    expect(byteToAscii(0x20)).toBe(' ')
    expect(byteToAscii(0x7e)).toBe('~')
    expect(byteToAscii(0x1f)).toBe('.')
    expect(byteToAscii(0x7f)).toBe('.')
  })
})
