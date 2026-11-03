import { describe, it, expect } from 'vitest'
import { diagHint, withDiagHint } from './diagHints'

describe('diagHints', () => {
  it('英文错误命中已有规则（大小写不敏感）', () => {
    expect(diagHint('Address already in use')).toContain('端口已被占用')
    expect(diagHint('CRC error')).toContain('CRC 校验失败')
  })

  // 审查#6：本地/后端拼装的中文错误串也要能命中建议
  it('中文错误串命中中文规则', () => {
    expect(diagHint('超时，未收到响应帧')).toContain('连接超时')
    expect(diagHint('从站 3 无响应')).toContain('设备未应答')
  })

  it('未命中返回空串，withDiagHint 原样透传', () => {
    expect(diagHint('某个未收录的失败')).toBe('')
    expect(withDiagHint('某个未收录的失败')).toBe('某个未收录的失败')
  })

  it('withDiagHint 命中时把建议拼在原文后', () => {
    const msg = withDiagHint('timed out')
    expect(msg).toContain('timed out')
    expect(msg).toContain('连接超时')
  })

  // http 层已归一化过、页面层再调一次：不能叠加成「（建议）（建议）」
  it('withDiagHint 幂等，重复调用不叠加括号', () => {
    const once = withDiagHint('bus bus1 not found')
    const twice = withDiagHint(once)
    expect(twice).toBe(once)
    expect(once.match(/目标不存在/g)).toHaveLength(1)
  })
})
