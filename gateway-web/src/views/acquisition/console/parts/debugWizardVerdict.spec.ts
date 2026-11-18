import { describe, it, expect } from 'vitest'
import {
  computeNextActions,
  computeProbeStats,
  computeVerdictText,
  computeVerdictTone,
  type VerdictInput,
} from './debugWizardVerdict'
import type { ProbeResult } from './debugWizardProbes'

/**
 * 汇总结论判定的契约。
 *
 * 汇总结论是现场交接班的唯一结论来源，判定顺序不能被后续改动悄悄改错——
 * 尤其是「勾选全完成 ≠ 自动检查全跑过」这条：说成「未发现异常」会让现场收工。
 */

function probe(tone: ProbeResult['tone']): ProbeResult {
  return { tone, message: '' }
}

function input(overrides: Partial<VerdictInput> = {}): VerdictInput {
  return {
    doneCount: 0,
    stepTotal: 5,
    probeTotal: 2,
    probes: [],
    ...overrides,
  }
}

describe('debugWizardVerdict — 探针统计', () => {
  it('按结论分类计数', () => {
    const stats = computeProbeStats([probe('danger'), probe('warning'), probe('success')])
    expect(stats.done).toBe(3)
    expect(stats.danger).toBe(1)
    expect(stats.warning).toBe(1)
    expect(stats.success).toBe(1)
  })
})

describe('debugWizardVerdict — 结论色调', () => {
  it('严重问题优先于一切（即使检查项全勾选）', () => {
    const i = input({ doneCount: 5 })
    const stats = computeProbeStats([probe('danger')])
    expect(computeVerdictTone(i, stats)).toBe('danger')
  })

  it('异常次之', () => {
    const i = input({ doneCount: 5 })
    const stats = computeProbeStats([probe('warning')])
    expect(computeVerdictTone(i, stats)).toBe('warning')
  })

  it('还有未完成检查项时是 info', () => {
    const i = input({ doneCount: 2 })
    expect(computeVerdictTone(i, computeProbeStats([]))).toBe('info')
  })

  it('勾选全完成但自动检查未跑完时仍是 info（不能报 success）', () => {
    const i = input({ doneCount: 5 })
    const stats = computeProbeStats([probe('success')])
    expect(computeVerdictTone(i, stats)).toBe('info')
  })

  it('全部完成且自动检查全通过才是 success', () => {
    const i = input({ doneCount: 5 })
    const stats = computeProbeStats([probe('success'), probe('success')])
    expect(computeVerdictTone(i, stats)).toBe('success')
  })
})

describe('debugWizardVerdict — 结论文案', () => {
  it('严重问题文案点名数量并要求先处置', () => {
    const stats = computeProbeStats([probe('danger'), probe('danger')])
    const text = computeVerdictText(input(), stats)
    expect(text).toContain('2 项严重问题')
    expect(text).toContain('请先按步骤内的提示处置')
  })

  it('异常文案建议先处理', () => {
    const stats = computeProbeStats([probe('warning')])
    expect(computeVerdictText(input(), stats)).toContain('1 项异常')
  })

  it('未完成时给出剩余数量', () => {
    const text = computeVerdictText(input({ doneCount: 2 }), computeProbeStats([]))
    expect(text).toContain('还有 3 项检查未完成')
  })

  it('勾选全完成但自动检查未执行时如实说明（不报「未发现异常」）', () => {
    const stats = computeProbeStats([probe('success')])
    const text = computeVerdictText(input({ doneCount: 5 }), stats)
    expect(text).toContain('检查项已全部确认，但还有 1 项自动检查未执行')
    expect(text).not.toContain('未发现异常')
  })

  it('全部完成且检查全通过才给通过结论', () => {
    const stats = computeProbeStats([probe('success'), probe('success')])
    const text = computeVerdictText(input({ doneCount: 5 }), stats)
    expect(text).toBe('全部检查项已完成，自动检查未发现异常')
  })
})

describe('debugWizardVerdict — 下一步入口', () => {
  it('按路由去重，同一页签只出现一次', () => {
    const actions = computeNextActions([
      { action: { label: 'A', to: '/x' } },
      { action: { label: 'A2', to: '/x' } },
      { action: { label: 'B', to: '/y' } },
    ])
    expect(actions.map((a) => a.to)).toEqual(['/x', '/y'])
    expect(actions[0]?.label).toBe('A')
  })

  it('没有 action 的步骤不产出入口', () => {
    expect(computeNextActions([{}, { action: undefined }])).toEqual([])
  })
})
