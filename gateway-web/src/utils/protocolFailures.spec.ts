/**
 * 协议失败分类文案的唯一映射。
 *
 * 这个文件存在的理由就是「只能有一份」：此前单步调试与报文编辑各写了一份表，
 * 同一个 `bus_offline` 一边叫「总线离线」一边叫「总线不可用」，用户会当成两种故障。
 * 所以除了常规的取值断言，这里额外锁两条契约：
 *   1. `api/protocolTools/types.ts` 里 `SendResult.error` 声明的全部分类都必须有中文；
 *   2. 两个调用方（单步调试、报文编辑）必须拿到同一份文案（防回归）。
 */
import { describe, it, expect } from 'vitest'
import { PROTOCOL_FAILURE_LABELS, protocolFailureLabel } from './protocolFailures'

/** 来自 `api/protocolTools/types.ts` 的 `SendResult.error` 注释（契约 §5） */
const CONTRACT_CODES = [
  'timeout',
  'crc',
  'exception',
  'bus_offline',
  'bus_busy',
  'invalid_frame',
  'bus_not_found',
] as const

describe('protocolFailureLabel', () => {
  it('契约声明的每个失败分类都有中文标签', () => {
    for (const code of CONTRACT_CODES) {
      expect(PROTOCOL_FAILURE_LABELS[code], `缺少 ${code} 的中文标签`).toBeTruthy()
      expect(protocolFailureLabel(code)).toBe(PROTOCOL_FAILURE_LABELS[code])
    }
  })

  it('空值返回空串，便于调用方直接拼进模板', () => {
    expect(protocolFailureLabel(null)).toBe('')
    expect(protocolFailureLabel(undefined)).toBe('')
    expect(protocolFailureLabel('')).toBe('')
  })

  it('未知机器码原样透出，不退化为空白', () => {
    // 后端新增分类而前端未跟版时，运维至少能看到原始标识去查日志
    expect(protocolFailureLabel('parity_error')).toBe('parity_error')
  })

  it('bus_offline / bus_busy 的措辞指向不同处置动作', () => {
    // bus_offline = 端口打不开（查线/查参数），bus_busy = 采集占用（去暂停采集）。
    // 两者文案必须能区分，否则用户会把「去暂停采集」做成「去换根线」。
    expect(protocolFailureLabel('bus_offline')).toBe('总线不可用')
    expect(protocolFailureLabel('bus_busy')).toBe('总线被采集占用')
  })

  it('单步调试与报文编辑拿到的是同一份文案（防两份表再次分叉）', async () => {
    const { errorLabel } = await import('@/views/acquisition/packet/parts/stepDebugTypes')
    const { protocolFailureLabel: shared } = await import('./protocolFailures')
    for (const code of CONTRACT_CODES) {
      expect(errorLabel(code)).toBe(shared(code))
    }
  })
})
