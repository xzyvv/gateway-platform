import { describe, it, expect } from 'vitest'
import {
  buildHistoryCsv,
  csvEscapeField,
  csvEscapeFieldOptionalQuote,
  sanitizeCsvCell,
  toCsv,
} from '../utils/exportCsv'
import type { DataPoint } from '@/api/types'

function makePoint(overrides: Partial<DataPoint> = {}): DataPoint {
  return {
    point_id: 'p1',
    device_id: 'dev1',
    value: 23.5,
    raw_value: 235,
    raw_type: 'int',
    quality: 'good',
    timestamp: 1700000000000,
    ...overrides,
  }
}

describe('toCsv — 实时数据导出 CSV（功能7）', () => {
  it('空数组返回仅含 BOM + 表头的 CSV', () => {
    const csv = toCsv([])
    expect(csv.startsWith('\uFEFF')).toBe(true)
    const body = csv.slice(1) // 去掉 BOM
    expect(body).toBe('point_id,device_id,value,raw_value,raw_type,quality,timestamp,time')
  })

  it('表头字段顺序固定', () => {
    const csv = toCsv([makePoint()])
    const body = csv.slice(1)
    const header = body.split('\n')[0]
    expect(header).toBe('point_id,device_id,value,raw_value,raw_type,quality,timestamp,time')
  })

  it('单条数据正确拼接各字段', () => {
    const csv = toCsv([makePoint()])
    const lines = csv.slice(1).split('\n')
    const row = lines[1]
    expect(row).toContain('p1')
    expect(row).toContain('dev1')
    expect(row).toContain('23.5')
    expect(row).toContain('235')
    expect(row).toContain('int')
    expect(row).toContain('good')
    expect(row).toContain('1700000000000')
  })

  it('time 列被双引号包裹（含中文/逗号）', () => {
    const csv = toCsv([makePoint()])
    const row = csv.slice(1).split('\n')[1]!
    // time 是最后一列，应被双引号包裹
    expect(row).toMatch(/"[^"]*"$/)
  })

  it('多条数据每行一条记录', () => {
    const csv = toCsv([
      makePoint({ point_id: 'p1' }),
      makePoint({ point_id: 'p2' }),
      makePoint({ point_id: 'p3' }),
    ])
    const lines = csv.slice(1).split('\n')
    expect(lines).toHaveLength(4) // 表头 + 3 条数据
  })

  it('timestamp 为 0/空时 time 列为空字符串', () => {
    const csv = toCsv([makePoint({ timestamp: 0 })])
    const row = csv.slice(1).split('\n')[1]!
    expect(row.endsWith('""')).toBe(true)
  })
})

// ============================================================
// W-7 修复：CSV 公式注入防护
// ============================================================

describe('sanitizeCsvCell — 公式注入脱敏（W-7）', () => {
  it('null / undefined 返回空串（不抛错）', () => {
    expect(sanitizeCsvCell(null)).toBe('')
    expect(sanitizeCsvCell(undefined)).toBe('')
  })

  it('数字类型正确字符串化', () => {
    expect(sanitizeCsvCell(0)).toBe('0')
    expect(sanitizeCsvCell(23.5)).toBe('23.5')
    // -100 首字符是 - 触发公式防护，加单引号前缀
    expect(sanitizeCsvCell(-100)).toBe("'-100")
  })

  it('普通字符串保持不变（不主动加引号前缀）', () => {
    expect(sanitizeCsvCell('p1')).toBe('p1')
    expect(sanitizeCsvCell('设备-A')).toBe('设备-A')
    expect(sanitizeCsvCell('hello world')).toBe('hello world')
  })

  it('以 = 开头的公式被前缀单引号', () => {
    expect(sanitizeCsvCell('=cmd|/c calc')).toBe("'=cmd|/c calc")
    expect(sanitizeCsvCell('=A1+B1')).toBe("'=A1+B1")
  })

  it('以 + - @ 开头也被前缀单引号', () => {
    expect(sanitizeCsvCell('+1+1')).toBe("'+1+1")
    expect(sanitizeCsvCell('-2*3')).toBe("'-2*3")
    expect(sanitizeCsvCell('@SUM(1+9)')).toBe("'@SUM(1+9)")
  })

  it('以 \\t (制表符) 开头也被前缀单引号（Excel 把 \\t+等号也当公式）', () => {
    expect(sanitizeCsvCell('\t=cmd|/c calc')).toBe("'\t=cmd|/c calc")
  })

  it('以 \\r (回车) 开头也被前缀单引号', () => {
    expect(sanitizeCsvCell('\r=cmd')).toBe("'\r=cmd")
  })

  it('内部 " 仍双写（即使不包外层）', () => {
    // 首字符 s 不在 FORMULA_TRIGGERS → 不加单引号前缀；只双写内部 "
    expect(sanitizeCsvCell('say "hi"')).toBe('say ""hi""')
    // 首字符 = 在 FORMULA_TRIGGERS → 加单引号前缀 + 双写内部 "
    expect(sanitizeCsvCell('=cmd|"/c"')).toBe(`'=cmd|""/c""`)
  })

  it('空字符串保持空串', () => {
    expect(sanitizeCsvCell('')).toBe('')
  })
})

describe('csvEscapeField — 总是双引号包裹（W-7）', () => {
  it('普通字符串 → 始终外层包裹', () => {
    expect(csvEscapeField('p1')).toBe('"p1"')
    expect(csvEscapeField('hello')).toBe('"hello"')
  })

  it('null / undefined → 空双引号', () => {
    expect(csvEscapeField(null)).toBe('""')
    expect(csvEscapeField(undefined)).toBe('""')
  })

  it('含逗号/换行的字符串 → 加引号 + 内部原样', () => {
    expect(csvEscapeField('a,b')).toBe('"a,b"')
    expect(csvEscapeField('line1\nline2')).toBe('"line1\nline2"')
  })

  it('含双引号 → 双写后再外层包裹', () => {
    expect(csvEscapeField('say "hi"')).toBe('"say ""hi"""')
  })

  it('公式注入防护：= 开头加单引号后整体包裹', () => {
    expect(csvEscapeField('=cmd|/c calc')).toBe(`"'=cmd|/c calc"`)
    expect(csvEscapeField('@SUM(A1)')).toBe(`"'@SUM(A1)"`)
  })

  it('公式 + 含双引号：防护 + RFC 4180 双写', () => {
    // =cmd|"/c" → sanitize: '=cmd|""/c"" → csvEscapeField: "'=cmd|""/c"""
    expect(csvEscapeField('=cmd|"/c"')).toBe(`"'=cmd|""/c"""`)
  })
})

describe('csvEscapeFieldOptionalQuote — 按需包裹（W-7）', () => {
  it('普通字符串不加引号', () => {
    expect(csvEscapeFieldOptionalQuote('p1')).toBe('p1')
    expect(csvEscapeFieldOptionalQuote('设备-A')).toBe('设备-A')
  })

  it('含逗号加引号', () => {
    expect(csvEscapeFieldOptionalQuote('a,b')).toBe('"a,b"')
  })

  it('含双引号加引号 + 内部双写', () => {
    expect(csvEscapeFieldOptionalQuote('say "hi"')).toBe('"say ""hi"""')
  })

  it('含换行加引号', () => {
    expect(csvEscapeFieldOptionalQuote('line1\nline2')).toBe('"line1\nline2"')
  })

  it('公式注入防护：= 开头加单引号前缀（按需再加外层）', () => {
    expect(csvEscapeFieldOptionalQuote('=A1+B1')).toBe("'=A1+B1")
  })

  it('公式 + 含逗号：单引号前缀 + 整体加引号', () => {
    expect(csvEscapeFieldOptionalQuote('=foo,bar')).toBe(`"'=foo,bar"`)
  })
})

describe('toCsv — 公式注入防护（W-7）', () => {
  it('point_id 以 = 开头时单引号前缀在数据中', () => {
    const csv = toCsv([makePoint({ point_id: '=cmd|"/c calc"!A0' })])
    const body = csv.slice(1)
    // point_id 字段以 " 包裹（因为含 "），单引号在 " 内
    expect(body).toContain(`'=cmd|""/c calc""!A0`)
  })

  it('device_id / raw_type / quality 同样受保护', () => {
    const csv = toCsv([
      makePoint({
        device_id: '@SUM(A1)',
        raw_type: '=1+1',
        quality: '+good',
      }),
    ])
    const body = csv.slice(1)
    expect(body).toContain("'@SUM(A1)")
    expect(body).toContain("'=1+1")
    expect(body).toContain("'+good")
  })

  it('设备名以 - 开头时仍以单引号前缀', () => {
    const csv = toCsv([makePoint({ device_id: '-离线标记' })])
    const body = csv.slice(1)
    expect(body).toContain("'-离线标记")
  })

  it('普通数据不被错误加引号前缀（向后兼容）', () => {
    const csv = toCsv([makePoint()])
    const body = csv.slice(1)
    // 不应出现任何不必要的单引号前缀
    expect(body).not.toMatch(/,'/)
    expect(body.startsWith('point_id,')).toBe(true)
  })

  it('导出仍含 BOM + 表头（既有功能未破坏）', () => {
    const csv = toCsv([makePoint()])
    expect(csv.startsWith('\uFEFF')).toBe(true)
    expect(csv.slice(1).split('\n')[0]).toBe(
      'point_id,device_id,value,raw_value,raw_type,quality,timestamp,time',
    )
  })
})

describe('buildHistoryCsv — 公式注入防护（W-7）', () => {
  it('含 = 公式的值在双引号包裹的单元格中仍以单引号前缀', () => {
    // Excel 即便看到 "=cmd" 这种被双引号包裹的字符串，仍会执行公式——
    // 单引号前缀是关键防线。
    const csv = buildHistoryCsv([['=cmd|"/c calc"!A0', '正常', '100', 'good']])
    // =cmd|"/c calc"!A0 → sanitize: '=cmd|""/c calc""!A0
    //                      → csvEscapeField: "'=cmd|""/c calc""!A0"
    expect(csv).toContain(`"'=cmd|""/c calc""!A0"`)
  })

  it('@ 开头也被防护', () => {
    const csv = buildHistoryCsv([['@SUM(A1:A9)', 'point1', '1', 'good']])
    expect(csv).toContain(`"'@SUM(A1:A9)"`)
  })

  it('普通单元格保持双引号包裹（既有行为）', () => {
    const csv = buildHistoryCsv([['2024-01-01 12:00:00', '温度', '25.5', 'good']])
    const lines = csv.split('\n')
    expect(lines[1]).toBe('"2024-01-01 12:00:00","温度","25.5","good"')
  })

  it('表头不受公式注入防护影响', () => {
    const csv = buildHistoryCsv([['a', 'b', 'c', 'd']])
    const lines = csv.split('\n')
    expect(lines[0]).toBe('"时间","点位","值","质量"')
  })

  it('空字符串单元格保持 ""', () => {
    const csv = buildHistoryCsv([['', '', '', '']])
    const lines = csv.split('\n')
    expect(lines[1]).toBe('"","","",""')
  })

  it('纯数字单元格也能正确处理', () => {
    const csv = buildHistoryCsv([[1, 2, 3, 4]])
    const lines = csv.split('\n')
    expect(lines[1]).toBe('"1","2","3","4"')
  })
})
