import { describe, it, expect } from 'vitest'
import { parseDLT645, parseCJT188, sampleErrorFrames, sampleFrames } from './frameParser'

/**
 * 回归测试：帧解析器能从 DLT645 / CJT188 帧中读出错误码（P2 #3a）。
 *
 * 现场排障的路径是「抓到一帧 → 看不懂 ERR 字节什么意思」，
 * 所以解析器必须直接把错误码翻译成人话，而不是只把字节摆出来。
 */

/** 按字段名取解析结果中的某一项 */
function fieldValue(fields: { name: string; value: string }[], name: string): string | undefined {
  return fields.find((f) => f.name === name)?.value
}

describe('DLT645 异常应答帧的错误码解读', () => {
  it('异常应答帧：解析出 ERR 位标志并逐条列出置位的错误', () => {
    const r = parseDLT645(sampleErrorFrames.dlt645!)
    expect(r.valid).toBe(true)

    // 控制码 D1H：D7=1（从站应答）、D6=1（异常应答）
    expect(fieldValue(r.fields, '控制码')).toBe('0xd1')
    expect(r.fields.find((f) => f.name === '控制码')?.desc).toContain('异常应答')

    // ERR = 0x05 → bit0（其他错误）+ bit2（密码错/未授权）
    expect(fieldValue(r.fields, '错误信息字 ERR')).toBe('0x05')
    expect(fieldValue(r.fields, 'ERR bit0')).toBe('其他错误')
    expect(fieldValue(r.fields, 'ERR bit2')).toContain('密码错')
    // 未置位的位不应出现
    expect(fieldValue(r.fields, 'ERR bit1')).toBeUndefined()
  })

  it('正常应答帧：不解读出 ERR（避免把正常帧误判成错误帧）', () => {
    // 控制码 0x91：D7=1（从站应答）、D6=0（正常应答）
    const bytes = [0x68, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0x68, 0x91, 0x02, 0x33, 0x33]
    const sum = bytes.reduce((s, b) => s + b, 0) & 0xff
    const hex = [...bytes, sum, 0x16].map((b) => b.toString(16).padStart(2, '0')).join(' ')
    const r = parseDLT645(hex)
    expect(fieldValue(r.fields, '错误信息字 ERR')).toBeUndefined()
    expect(r.fields.find((f) => f.name === '控制码')?.desc).toContain('正常应答')
  })

  it('主站请求帧：不解读出 ERR', () => {
    // 请求帧控制码 D7=0、D6=0
    const r = parseDLT645(sampleFrames.dlt645!)
    expect(fieldValue(r.fields, '错误信息字 ERR')).toBeUndefined()
    expect(r.fields.find((f) => f.name === '控制码')?.desc).toContain('主站请求帧')
  })

  it('ERR 为 0x00（异常应答但无定义位）时给出兜底说明', () => {
    // 68 ... 68 D1 01 00 CS 16
    const bytes = [0x68, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0x68, 0xd1, 0x01, 0x00]
    const sum = bytes.reduce((s, b) => s + b, 0) & 0xff
    const hex = [...bytes, sum, 0x16].map((b) => b.toString(16).padStart(2, '0')).join(' ')
    const r = parseDLT645(hex)
    expect(fieldValue(r.fields, '错误信息字 ERR')).toBe('0x00')
    const detail = r.fields.find((f) => f.name === '错误详情')
    expect(detail?.value).toContain('未置任何已知错误位')
    expect(detail?.desc).toContain('其他错误')
  })
})

describe('CJT188 异常应答帧的状态解读', () => {
  it('异常应答帧：解析出状态 ST 并解出诊断码', () => {
    const r = parseCJT188(sampleErrorFrames.cjt188!)
    expect(r.valid).toBe(true)

    // 控制码 C1H：D7=1（从站应答）、D6=1（异常应答）
    expect(fieldValue(r.fields, '控制码')).toBe('0xc1')
    expect(r.fields.find((f) => f.name === '控制码')?.desc).toContain('异常应答')

    // 数据域 = SER(0x01) + ST(00 04)
    expect(fieldValue(r.fields, '序列号 SER')).toBe('0x01')
    expect(fieldValue(r.fields, '状态 ST')).toBe('00 04')
    // JD = 0x04 → 电池电压低于 3.3V
    expect(fieldValue(r.fields, '诊断码 JD 0x04')).toContain('3.3V')
  })

  it('状态 ST 第一字节置电池欠压位时列出该位', () => {
    // ST = 04 00 → 第1字节 bit2 电池电压欠压，JD 正常
    const bytes = [0x68, 0x10, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xc1, 0x03, 0x01, 0x04, 0x00]
    const sum = bytes.reduce((s, b) => s + b, 0) & 0xff
    const hex = [...bytes, sum, 0x16].map((b) => b.toString(16).padStart(2, '0')).join(' ')
    const r = parseCJT188(hex)
    expect(fieldValue(r.fields, '状态 ST')).toBe('04 00')
    expect(fieldValue(r.fields, 'ST bit2')).toContain('电池')
  })

  it('状态全正常时给出「正常」结论而非留白', () => {
    const bytes = [0x68, 0x10, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xc1, 0x03, 0x01, 0x00, 0x00]
    const sum = bytes.reduce((s, b) => s + b, 0) & 0xff
    const hex = [...bytes, sum, 0x16].map((b) => b.toString(16).padStart(2, '0')).join(' ')
    const r = parseCJT188(hex)
    expect(fieldValue(r.fields, '状态解读')).toContain('正常')
  })

  it('主站请求帧：不做状态解读', () => {
    const r = parseCJT188(sampleFrames.cjt188!)
    expect(fieldValue(r.fields, '状态 ST')).toBeUndefined()
  })
})

describe('示例帧完整性', () => {
  it('异常示例帧的校验和正确（现场按此帧复现不会因校验失败卡住）', () => {
    for (const hex of [sampleErrorFrames.dlt645!, sampleErrorFrames.cjt188!]) {
      const bytes = hex.split(/\s+/).map((h) => parseInt(h, 16))
      const sum = bytes.reduce((s, b, i) => (i < bytes.length - 2 ? s + b : s), 0) & 0xff
      expect(sum, `帧 ${hex} 校验和不符`).toBe(bytes[bytes.length - 2])
      // 帧尾必须是 0x16
      expect(bytes[bytes.length - 1]).toBe(0x16)
    }
  })
})
