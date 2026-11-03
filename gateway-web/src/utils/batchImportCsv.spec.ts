import { describe, it, expect } from 'vitest'
import { parseCSV, validateImport } from './batchImportCsv'

/** 列顺序：设备名称,设备ID,所属总线,协议,从站ID,模板,地址（末列留空即由从站ID推导） */
const HEADER = '设备名称,设备ID,所属总线,协议,从站ID,模板,地址'

describe('batchImportCsv 形状冒烟（随模块自 mock 迁入）', () => {
  it('解析与校验形状', () => {
    const rows = parseCSV(`${HEADER}\n仪表-01,dev1,TCP-1,modbus-tcp,1,模板A,\n,dev2,RTU-1,modbus-rtu,2,模板A,`)
    expect(rows).toHaveLength(2)
    for (const r of rows) {
      expect(['line', 'name', 'id', 'bus', 'protocol', 'slaveId', 'template', 'address', 'valid'].every((k) => k in r)).toBe(true)
    }
    const res = validateImport(rows)
    expect(res.total).toBe(2)
    expect(res.invalid).toBe(1)
    expect(res.rows[1]!.error).toContain('缺少设备名称')
  })
})

/**
 * CSV 解析的回归护栏（P1-14）。
 *
 * 背景：原实现用 `split(',')` 逐行切列，设备名含逗号时字段整体左移——
 * 总线名落到协议列，报错变成「协议须为 modbus-tcp」，用户完全看不出
 * 是自己的文件被切坏了；逗号出现在最后一列时更糟：整行通过校验，
 * 多余字段被无声丢弃。
 */
describe('batchImportCsv CSV 解析（P1-14）', () => {
  it('设备名含逗号且已用双引号包裹：字段不再错位，该行有效', () => {
    const rows = parseCSV(`${HEADER}\n"电表,1号",dev1,TCP-1,modbus-tcp,1,模板A,`)
    expect(rows[0]!.name).toBe('电表,1号')
    expect(rows[0]!.id).toBe('dev1')
    expect(rows[0]!.bus).toBe('TCP-1')
    expect(rows[0]!.protocol).toBe('modbus-tcp')
    const res = validateImport(rows)
    expect(res.valid).toBe(1)
    expect(res.rows[0]!.error).toBeUndefined()
  })

  it('设备名含逗号但没加引号：明确报「列数不符」，而不是误导性的协议错误', () => {
    const rows = parseCSV(`${HEADER}\n电表,1号,dev1,TCP-1,modbus-tcp,1`)
    expect(rows[0]!.error).toContain('列数应为 7')
    const res = validateImport(rows)
    expect(res.valid).toBe(0)
    expect(res.invalid).toBe(1)
    // 根因（列数）在前，连带错误在后
    expect(res.rows[0]!.error!.indexOf('列数应为 7')).toBeLessThan(
      res.rows[0]!.error!.indexOf('需要填写表地址'),
    )
  })

  it('列数不符的行不会被静默丢弃：仍在结果里且带错误原因', () => {
    const rows = parseCSV(`${HEADER}\n电表,1号,dev1,TCP-1,modbus-tcp,1`)
    const res = validateImport(rows)
    expect(res.total).toBe(1)
    expect(res.rows).toHaveLength(1)
    expect(res.rows[0]!.error).toBeTruthy()
  })

  it('逗号出现在最后一列时不再「整行通过校验」（旧实现会静默吞掉多余字段）', () => {
    const rows = parseCSV(`${HEADER}\n仪表-01,dev1,TCP-1,modbus-tcp,1,模板A,多余,多余`)
    const res = validateImport(rows)
    expect(res.valid).toBe(0)
    expect(res.rows[0]!.error).toContain('列数应为 7')
  })

  it('正常行不受影响', () => {
    const rows = parseCSV(`${HEADER}\n仪表-01,dev1,TCP-1,modbus-tcp,1,模板A,`)
    const res = validateImport(rows)
    expect(res.valid).toBe(1)
    expect(res.rows[0]!.slaveId).toBe(1)
  })

  it('模板不存在 / 协议不一致会被本地拦下（后端同样会拒绝）', () => {
    const rows = parseCSV(`${HEADER}\n仪表-01,dev1,TCP-1,modbus-tcp,1,不存在模板,\n仪表-02,dev2,TCP-1,modbus-tcp,2,DLT模板,`)
    const res = validateImport(rows, [
      { name: 'DLT模板', protocol: 'dlt645' },
      { name: 'Modbus模板', protocol: 'modbus-tcp' },
    ])
    expect(res.invalid).toBe(2)
    expect(res.rows[0]!.error).toContain('不存在')
    expect(res.rows[1]!.error).toContain('协议是 dlt645')
  })
})
