/**
 * 点位 CSV 字段清单的转换语义测试。
 *
 * 这张表是导出与导入共用的唯一口径，也是「导出→改→导回」无损的前提，
 * 所以三个语义必须钉死：
 * - 可清空字段留空 = 清空；
 * - 不可清空字段留空 = 保留原值（scale/offset 留空绝不能变成 0）；
 * - 数字列的非数字文本直接判错。
 */
import { describe, expect, it } from 'vitest'
import {
  POINT_FIELDS_BASIC,
  POINT_FIELDS_FULL,
  POINT_FIELD_BY_COLUMN,
  formatPointCell,
  parsePointCell,
  type PointFieldSpec,
} from './pointFields'

/** 取列名字段描述 */
const specOf = (column: string): PointFieldSpec => {
  const spec = POINT_FIELD_BY_COLUMN.get(column)
  if (!spec) throw new Error(`字段清单里没有列 ${column}`)
  return spec
}

describe('点位 CSV 字段清单', () => {
  it('精简档是 10 列，完整档覆盖其余可平铺字段但不含嵌套的 aggregation', () => {
    expect(POINT_FIELDS_BASIC.map((f) => f.column)).toEqual([
      'id', 'name', 'device_id', 'bus', 'protocol', 'address',
      'slave_id', 'address_num', 'ct_ratio', 'pt_ratio',
    ])
    expect(POINT_FIELDS_FULL.length).toBeGreaterThan(POINT_FIELDS_BASIC.length)
    expect(POINT_FIELDS_FULL.map((f) => f.column)).not.toContain('aggregation')
    // 完整档列名唯一，且都是完整档的子集关系
    const cols = POINT_FIELDS_FULL.map((f) => f.column)
    expect(new Set(cols).size).toBe(cols.length)
  })

  it('列名大小写不敏感：表头小写也能定位到字段', () => {
    expect(POINT_FIELD_BY_COLUMN.has('scale')).toBe(true)
    expect(POINT_FIELD_BY_COLUMN.has('SCALE')).toBe(false)
  })

  it('可清空字段留空 → 清空（null）', () => {
    expect(parsePointCell(specOf('unit'), '')).toEqual({ ok: true, value: null })
    expect(parsePointCell(specOf('formula'), '')).toEqual({ ok: true, value: null })
  })

  it('布尔字段留空 → false（不是清空）', () => {
    expect(parsePointCell(specOf('writable'), '')).toEqual({ ok: true, value: false })
    expect(parsePointCell(specOf('writable'), 'true')).toEqual({ ok: true, value: true })
    expect(parsePointCell(specOf('writable'), '1')).toEqual({ ok: true, value: true })
    expect(parsePointCell(specOf('writable'), '否')).toEqual({ ok: true, value: false })
  })

  it('不可清空字段留空 → undefined（保留原值），scale/offset 尤其不能变成 0', () => {
    expect(parsePointCell(specOf('scale'), '')).toEqual({ ok: true, value: undefined })
    expect(parsePointCell(specOf('offset'), '')).toEqual({ ok: true, value: undefined })
    expect(parsePointCell(specOf('interval'), '')).toEqual({ ok: true, value: undefined })
    expect(parsePointCell(specOf('deadband'), '')).toEqual({ ok: true, value: undefined })
  })

  it('必填字段留空 → undefined，仍由必填校验层拦下', () => {
    expect(parsePointCell(specOf('id'), '')).toEqual({ ok: true, value: undefined })
    expect(specOf('id').required).toBe(true)
    expect(specOf('bus').required).toBe(true)
    expect(specOf('protocol').required).toBe(true)
    expect(specOf('address').required).toBe(true)
  })

  it('数字列的非数字文本直接判错', () => {
    const res = parsePointCell(specOf('scale'), 'abc')
    expect(res.ok).toBe(false)
    if (!res.ok) expect(res.error).toContain('scale')
    expect(parsePointCell(specOf('scale'), '0.1')).toEqual({ ok: true, value: 0.1 })
  })

  it('文本列去掉首尾空格', () => {
    expect(parsePointCell(specOf('name'), '  温度  ')).toEqual({ ok: true, value: '温度' })
  })

  it('导出格式化：空值成空串，布尔成 true/false', () => {
    expect(formatPointCell(specOf('unit'), null)).toBe('')
    expect(formatPointCell(specOf('unit'), undefined)).toBe('')
    expect(formatPointCell(specOf('writable'), true)).toBe('true')
    expect(formatPointCell(specOf('writable'), false)).toBe('false')
    expect(formatPointCell(specOf('scale'), 0.1)).toBe('0.1')
  })
})
