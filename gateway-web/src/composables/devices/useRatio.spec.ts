import { describe, it, expect } from 'vitest'
import { ratioScopeOf } from './useRatio'

describe('ratioScopeOf（变比随单位的生效范围）', () => {
  it('电压只吃 PT（大小写/前缀不敏感）', () => {
    expect(ratioScopeOf('V')).toBe('pt')
    expect(ratioScopeOf('kV')).toBe('pt')
    expect(ratioScopeOf('MV')).toBe('pt')
  })

  it('电流只吃 CT', () => {
    expect(ratioScopeOf('A')).toBe('ct')
    expect(ratioScopeOf('kA')).toBe('ct')
    expect(ratioScopeOf('mA')).toBe('ct')
  })

  it('功率/电量两者都吃', () => {
    expect(ratioScopeOf('kW')).toBe('both')
    expect(ratioScopeOf('kVarh')).toBe('both')
  })

  it('功率因数两者都不吃', () => {
    expect(ratioScopeOf('PF')).toBe('none')
    expect(ratioScopeOf('cosφ')).toBe('none')
    expect(ratioScopeOf('COSΦ')).toBe('none')
    expect(ratioScopeOf('功率因数')).toBe('none')
  })

  it('单位为空或未知时保持兼容行为（CT × PT）', () => {
    expect(ratioScopeOf(undefined)).toBe('both')
    expect(ratioScopeOf('')).toBe('both')
    expect(ratioScopeOf('℃')).toBe('both')
  })
})
