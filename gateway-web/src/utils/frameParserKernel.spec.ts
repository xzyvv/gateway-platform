import { describe, it, expect } from 'vitest'
import { inferParseProtocol, parseFrame, parseResultToText } from './frameParser'

describe('帧解析内核（C4 离线/在线共用）', () => {
  it('总线协议 → 解析内核映射只覆盖帧结构一致的协议', () => {
    expect(inferParseProtocol('modbus-rtu')).toBe('modbus-rtu')
    expect(inferParseProtocol('modbus')).toBe('modbus-rtu')
    expect(inferParseProtocol('dlt645')).toBe('dlt645')
    expect(inferParseProtocol('cjt188')).toBe('cjt188')
    // MBAP 头与 RTU 帧结构不同，不能混用内核
    expect(inferParseProtocol('modbus-tcp')).toBeNull()
    expect(inferParseProtocol('s7comm')).toBeNull()
    expect(inferParseProtocol(undefined)).toBeNull()
  })

  it('同一内核解析离线样本帧，返回逐字段结果', () => {
    const r = parseFrame('modbus-rtu', '01 03 00 00 00 0A C5 CD')
    expect(r.protocol).toBe('modbus-rtu')
    expect(r.fields.some((f) => f.name === '从站地址')).toBe(true)
    expect(r.fields.some((f) => f.name === '功能码')).toBe(true)
    expect(r.fields.some((f) => f.name === 'CRC 校验')).toBe(true)
  })

  it('结果文本包含原始帧与全部字段（复制给研发复现用）', () => {
    const r = parseFrame('dlt645', '68 AA AA AA AA AA AA 68 11 04 33 33 33 33 0E 16')
    const text = parseResultToText(r)
    expect(text).toContain('原始HEX:')
    for (const f of r.fields) expect(text).toContain(`${f.name}:`)
  })
})
