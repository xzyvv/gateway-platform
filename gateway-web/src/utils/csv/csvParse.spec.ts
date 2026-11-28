import { describe, it, expect } from 'vitest'
import { parseCsv, parseCsvRows } from './csvParse'

/**
 * RFC 4180 CSV 解析器的回归护栏（P1-14）。
 *
 * 背景：原实现各处用 `split(',')` 逐行切列，字段含逗号时列被切坏——
 * 后面的字段整体左移一位，设备名/点位名被截断，且报错完全误导
 * （总线名落到协议列 → 报「协议须为 modbus-tcp」）。
 */
describe('parseCsv（RFC 4180）', () => {
  it('普通 CSV 按逗号切列', () => {
    const rows = parseCsv('a,b,c\n1,2,3')
    expect(rows).toEqual([['a', 'b', 'c'], ['1', '2', '3']])
  })

  it('双引号包裹的字段可含逗号，不再被切成两列', () => {
    const rows = parseCsv('"电表,1号",dev1,TCP-1')
    expect(rows[0]).toEqual(['电表,1号', 'dev1', 'TCP-1'])
  })

  it('双引号内的转义（""）还原为一个双引号', () => {
    const rows = parseCsv('"他说""好""",dev1')
    expect(rows[0]![0]).toBe('他说"好"')
  })

  it('引号内的换行属于字段内容，不另起一行', () => {
    const rows = parseCsv('"第一行\n第二行",dev1')
    expect(rows).toHaveLength(1)
    expect(rows[0]![0]).toBe('第一行\n第二行')
  })

  it('字段首尾空白被去除，但引号内的空白保留', () => {
    const rows = parseCsv('  a  ," b ",c')
    expect(rows[0]).toEqual(['a', ' b ', 'c'])
  })

  it('CRLF 与 LF 都能正确分行', () => {
    expect(parseCsv('a,b\r\n1,2')).toEqual([['a', 'b'], ['1', '2']])
    expect(parseCsv('a,b\n1,2')).toEqual([['a', 'b'], ['1', '2']])
  })

  it('空行被跳过（skipEmpty 默认开）', () => {
    expect(parseCsv('a\n\n\nb')).toEqual([['a'], ['b']])
    expect(parseCsv('a\n\n\nb', { skipEmpty: false })).toHaveLength(4)
  })

  it('末尾无换行时最后一行仍被解析', () => {
    expect(parseCsv('a\n1,2')).toEqual([['a'], ['1', '2']])
  })

  it('物理行号随行递增（引号内的换行不另计行）', () => {
    const rows = parseCsvRows('h1\n"x\ny",v\nz')
    expect(rows.map((r) => r.line)).toEqual([1, 2, 4])
  })

  it('空输入返回空数组', () => {
    expect(parseCsv('')).toEqual([])
    expect(parseCsvRows('')).toEqual([])
  })

  it('列数不齐的行仍被返回（字段数如实反映，交给调用方判错）', () => {
    const rows = parseCsv('a,b,c\n1,2')
    expect(rows[1]).toEqual(['1', '2'])
  })
})
