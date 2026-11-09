import { describe, it, expect } from 'vitest'
import { parseImportPoints } from '../utils/importPoints'

/** 一个完整合法的点位 fixture */
function validPoint(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    id: 'p1',
    bus: 'bus1',
    protocol: 'modbus',
    address: '40001',
    interval: 10,
    on_change: false,
    ...over,
  }
}

describe('parseImportPoints — 基础 JSON 解析', () => {
  it('空字符串返回错误', () => {
    const r = parseImportPoints('')
    expect(r.valid).toEqual([])
    expect(r.error).toBe('请输入 JSON 数组')
  })

  it('仅空白字符返回错误', () => {
    const r = parseImportPoints('   \n  ')
    expect(r.error).toBe('请输入 JSON 数组')
  })

  it('非法 JSON 返回格式错误', () => {
    const r = parseImportPoints('{not valid json')
    expect(r.error).toBe('JSON 格式错误，请检查')
    expect(r.valid).toEqual([])
  })

  it('非数组 JSON 返回错误', () => {
    const r = parseImportPoints('{"id": "p1"}')
    expect(r.error).toBe('请输入 JSON 数组')
  })

  it('null/undefined 元素被忽略', () => {
    const r = parseImportPoints(
      JSON.stringify([null, undefined, validPoint()]),
    )
    expect(r.valid).toHaveLength(1)
    expect(r.valid[0]!.id).toBe('p1')
  })
})

describe('parseImportPoints — Bug 2.9 字段类型校验', () => {
  it('缺少必填字符串字段 id/bus/protocol/address \u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([{ bus: 'b', protocol: 'modbus', address: '1', interval: 10, on_change: false }]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/id 字段必填/)
  })

  it('interval 字段类型错误（字符串）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ interval: 'abc' })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/interval/)
  })

  it('interval < 1 \u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ interval: 0 })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/interval/)
  })

  it('slave_id 越界（>247）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ slave_id: 300 })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/slave_id/)
  })

  it('slave_id 越界（<1）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ slave_id: 0 })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/slave_id/)
  })

  it('数值字段类型错误（字符串）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ scale: '1.5' })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/scale/)
  })

  it('on_change 字段类型错误（字符串）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ on_change: 'yes' })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/on_change/)
  })

  it('bus 字段类型错误（数字）\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ bus: 123 })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/bus/)
  })

  it('部分有效 + 部分失败 \u2192 错误信息提示失败数', () => {
    const r = parseImportPoints(JSON.stringify([
      validPoint({ id: 'p1' }),
      { id: 'p2', bus: 'b' /* \u7f3a protocol/address/interval/on_change */ },
    ]))
    expect(r.valid).toHaveLength(1)
    expect(r.valid[0]!.id).toBe('p1')
    expect(r.error).toMatch(/1 \u6761\u70b9\u4f4d\u6821\u9a8c\u5931\u8d25/)
  })

  it('完整合法点位\u2192 通过', () => {
    const r = parseImportPoints(JSON.stringify([
      validPoint({ id: 'p1', name: '\u6e29\u5ea6', slave_id: 1, scale: 0.1 }),
    ]))
    expect(r.valid).toHaveLength(1)
    expect(r.error).toBeUndefined()
  })

  it('name 为空字符串\u2192 拒收', () => {
    const r = parseImportPoints(JSON.stringify([validPoint({ name: '' })]))
    expect(r.valid).toHaveLength(0)
    expect(r.error).toMatch(/name/)
  })

  it('CT/PT 变比 <= 0 \u2192 拒收（0 会把一次侧值乘成 0）', () => {
    for (const key of ['ct_ratio', 'pt_ratio']) {
      const r = parseImportPoints(JSON.stringify([validPoint({ [key]: 0 })]))
      expect(r.valid).toHaveLength(0)
      expect(r.error).toMatch(new RegExp(key))
      const neg = parseImportPoints(JSON.stringify([validPoint({ [key]: -1 })]))
      expect(neg.valid).toHaveLength(0)
    }
    const ok = parseImportPoints(JSON.stringify([validPoint({ ct_ratio: 30, pt_ratio: 100 })]))
    expect(ok.valid).toHaveLength(1)
    expect(ok.error).toBeUndefined()
  })
})
