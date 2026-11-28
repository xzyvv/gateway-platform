import { describe, it, expect } from 'vitest'
import { listFuncCodes, calculate } from './addrCalculator'

describe('addrCalculator 形状冒烟（随模块自 mock 迁入）', () => {
  it('功能码表与换算结果形状', async () => {
    const codes = listFuncCodes()
    expect(codes).toHaveLength(8)
    for (const f of codes) {
      expect(['code', 'name'].every((k) => k in f)).toBe(true)
    }
    const r = await calculate('40001', '03', 2, 1)
    expect(r.systemAddr).toBe('0')
    expect(Object.keys(r).sort()).toEqual(['funcCode', 'funcName', 'manualAddr', 'note', 'requestHex', 'systemAddr'])
  })
})
