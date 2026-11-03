import { describe, it, expect } from 'vitest'
import { QUALITY_TEXT, qualityMap, qualityText, qualityTagType } from './qualityLabels'

/**
 * 质量文案单一来源的契约：
 * 全站（实时/历史/质量页/设备影子）对同一状态必须呈现同一措辞。
 * 后端不同接口的 key 命名不一致（小写 vs 首字母大写），映射表必须两边都覆盖。
 */
describe('qualityLabels 统一文案', () => {
  it('同一状态的大小写 key 文案一致', () => {
    expect(qualityText('good')).toBe('良好')
    expect(qualityText('Good')).toBe('良好')
    expect(qualityText('bad')).toBe('坏值')
    expect(qualityText('Bad')).toBe('坏值')
    expect(qualityText('uncertain')).toBe('不确定')
    expect(qualityText('Uncertain')).toBe('不确定')
  })

  it('stale 与 old 都表示「过期」', () => {
    expect(qualityText('stale')).toBe('过期')
    expect(qualityText('old')).toBe('过期')
  })

  it('聚合值有独立文案', () => {
    expect(qualityText('Aggregated')).toBe('聚合')
  })

  it('未知值原样返回，不吞掉后端新增状态', () => {
    expect(qualityText('someNewState')).toBe('someNewState')
    expect(qualityText('')).toBe('')
  })

  it('qualityMap 与 QUALITY_TEXT 同源', () => {
    expect(qualityMap.Good).toBe(QUALITY_TEXT.good)
    expect(qualityMap.Bad).toBe(QUALITY_TEXT.bad)
  })
})

describe('qualityTagType', () => {
  it('good→success / bad→danger，大小写不敏感', () => {
    expect(qualityTagType('good')).toBe('success')
    expect(qualityTagType('Good')).toBe('success')
    expect(qualityTagType('bad')).toBe('danger')
    expect(qualityTagType('Bad')).toBe('danger')
  })

  it('过期类(old)→info，不确定/ stale→warning', () => {
    expect(qualityTagType('old')).toBe('info')
    expect(qualityTagType('Aggregated')).toBe('info')
    expect(qualityTagType('uncertain')).toBe('warning')
    expect(qualityTagType('stale')).toBe('warning')
  })
})
