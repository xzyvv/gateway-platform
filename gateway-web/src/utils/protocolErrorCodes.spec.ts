import { describe, it, expect } from 'vitest'
import {
  buildProtocolErrorEntries,
  decodeCjt188St,
  decodeDlt645Err,
  DLT645_ERR_BITS,
  CJT188_JD_ENTRIES,
  CJT188_ST_BITS,
  PROTOCOL_ERROR_NOTES,
} from './protocolErrorCodes'

/**
 * 回归测试：DLT645 / CJT188 异常码知识库（P2 #3a）。
 *
 * 关键点：两者的错误表达机制与 Modbus 完全不同——
 *   - DLT645 的 ERR 是**位标志**（一个字节可同时表示多个错误）
 *   - CJT188 的状态 ST 是「位标志字节 + 诊断码字节」两字节结构
 * 建模错了会导致现场按错误码排查时得到相反结论。
 */

describe('DLT645 错误信息字 ERR（位标志）', () => {
  it('覆盖规范定义的 bit0~bit6（bit7 保留）', () => {
    expect(DLT645_ERR_BITS).toHaveLength(7)
    expect(DLT645_ERR_BITS.map((b) => b.bit)).toEqual([0, 1, 2, 3, 4, 5, 6])
  })

  it('bit0 为「其他错误」（规范兜底位）', () => {
    const bit0 = DLT645_ERR_BITS.find((b) => b.bit === 0)!
    expect(bit0.name).toBe('其他错误')
  })

  it('bit2 为「密码错/未授权」', () => {
    const bit2 = DLT645_ERR_BITS.find((b) => b.bit === 2)!
    expect(bit2.name).toContain('密码')
  })

  it('掩码与位序号一致（mask = 1 << bit）', () => {
    for (const b of DLT645_ERR_BITS) {
      expect(b.mask).toBe(1 << b.bit)
    }
  })

  it('解码单个置位：0x04 → bit2 密码错', () => {
    const bits = decodeDlt645Err(0x04)
    expect(bits).toHaveLength(1)
    expect(bits[0]!.bit).toBe(2)
  })

  it('解码多个置位：0x05 → bit0 + bit2 同时成立', () => {
    const bits = decodeDlt645Err(0x05)
    expect(bits.map((b) => b.bit)).toEqual([0, 2])
  })

  it('解码 0x00 → 无错误位', () => {
    expect(decodeDlt645Err(0x00)).toHaveLength(0)
  })

  it('解码保留位 bit7 时不误报（规范未定义，归其他错误）', () => {
    // bit7 为保留位，置位不应被解读成某个具体错误
    const bits = decodeDlt645Err(0x80)
    expect(bits).toHaveLength(0)
  })

  it('每个码位都有原因与处理建议（现场要能直接照做）', () => {
    for (const b of DLT645_ERR_BITS) {
      expect(b.meaning.length, `bit${b.bit} 缺含义`).toBeGreaterThan(0)
      expect(b.cause.length, `bit${b.bit} 缺原因`).toBeGreaterThan(0)
      expect(b.solution.length, `bit${b.bit} 缺处理建议`).toBeGreaterThan(0)
    }
  })
})

describe('CJT188 状态 ST', () => {
  it('第一字节位标志覆盖阀门与电池', () => {
    const names = CJT188_ST_BITS.map((b) => b.name).join(' ')
    expect(names).toContain('阀门')
    expect(names).toContain('电池')
  })

  it('解码第一字节：0x04 → 电池电压欠压', () => {
    const { bits } = decodeCjt188St([0x04, 0x00])
    expect(bits).toHaveLength(1)
    expect(bits[0]!.name).toContain('电池')
  })

  it('解码诊断码低4位：JD=0x04 → 电池电压低于 3.3V', () => {
    const { diagnostics } = decodeCjt188St([0x00, 0x04])
    expect(diagnostics).toHaveLength(1)
    expect(diagnostics[0]!.code).toBe('0x04')
  })

  it('诊断码低4位互斥：0x01 与 0x02 不同时命中', () => {
    expect(decodeCjt188St([0x00, 0x01]).diagnostics[0]!.code).toBe('0x01')
    expect(decodeCjt188St([0x00, 0x02]).diagnostics[0]!.code).toBe('0x02')
  })

  it('诊断码高4位可并存：0x30 → 供水温度超150℃ + 回水温度传感器故障', () => {
    // 0x30 = 0x10（供水温度传感器故障）| 0x20（供水温度超150℃）
    const { diagnostics } = decodeCjt188St([0x00, 0x30])
    const codes = diagnostics.map((d) => d.code)
    expect(codes).toContain('0x10')
    expect(codes).toContain('0x20')
    // 0x40（回水温度传感器故障）未置位，不应出现
    expect(codes).not.toContain('0x40')
  })

  it('高低4位可同时置位：0x14 → 电池欠压 + 供水温度传感器故障', () => {
    // 0x14 = 低4位 0x04（电池电压低于3.3V）| 高4位 0x10（供水温度传感器故障）
    const { diagnostics } = decodeCjt188St([0x00, 0x14])
    const codes = diagnostics.map((d) => d.code)
    expect(codes).toContain('0x04')
    expect(codes).toContain('0x10')
  })

  it('JD=0x00 表示正常，无诊断项', () => {
    const { bits, diagnostics } = decodeCjt188St([0x00, 0x00])
    expect(bits).toHaveLength(0)
    expect(diagnostics).toHaveLength(0)
  })

  it('每条诊断码都有原因与处理建议', () => {
    for (const d of CJT188_JD_ENTRIES) {
      expect(d.meaning.length, `${d.code} 缺含义`).toBeGreaterThan(0)
      expect(d.cause.length, `${d.code} 缺原因`).toBeGreaterThan(0)
      expect(d.solution.length, `${d.code} 缺处理建议`).toBeGreaterThan(0)
    }
  })
})

describe('码表转表格行', () => {
  it('DLT645 按 bit 逐行输出，码形如 ERR bit0', () => {
    const rows = buildProtocolErrorEntries('dlt645')
    expect(rows).toHaveLength(7)
    expect(rows[0]!.code).toBe('ERR bit0')
    expect(rows[0]!.name).toBe('其他错误')
  })

  it('CJT188 输出位标志 + 诊断码两类行', () => {
    const rows = buildProtocolErrorEntries('cjt188')
    expect(rows.length).toBe(CJT188_ST_BITS.length + CJT188_JD_ENTRIES.length)
    expect(rows.some((r) => r.code.startsWith('ST bit'))).toBe(true)
    expect(rows.some((r) => r.code.startsWith('JD '))).toBe(true)
  })

  it('每行都有完整的四段文案（表格直接渲染）', () => {
    for (const p of ['dlt645', 'cjt188'] as const) {
      for (const r of buildProtocolErrorEntries(p)) {
        expect(r.code.length, `${p} ${r.code} 缺码`).toBeGreaterThan(0)
        expect(r.name.length, `${p} ${r.code} 缺名称`).toBeGreaterThan(0)
        expect(r.meaning.length, `${p} ${r.code} 缺含义`).toBeGreaterThan(0)
        expect(r.cause.length, `${p} ${r.code} 缺原因`).toBeGreaterThan(0)
        expect(r.solution.length, `${p} ${r.code} 缺建议`).toBeGreaterThan(0)
      }
    }
  })
})

describe('协议机制说明', () => {
  it('两种协议都说明了错误表达机制（用户最容易误解的地方）', () => {
    expect(PROTOCOL_ERROR_NOTES.dlt645).toContain('位标志')
    expect(PROTOCOL_ERROR_NOTES.cjt188).toContain('厂商定义')
  })
})
