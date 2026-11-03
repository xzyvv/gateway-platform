/**
 * 现场调试向导数据契约测试（只测数据，不测 UI）。
 *
 * 流程数据是新人照着执行的「作业指导书」，契约破坏时会直接导致
 * 页面选项串台、勾选进度错乱或「去处理」跳到不存在的路由。
 */
import { describe, expect, it } from 'vitest'
import { WIZARD_PROTOCOLS, debugWizardFlows } from './debugWizardFlows'

describe('debugWizardFlows 数据契约', () => {
  it('至少提供 7 条排查流程', () => {
    expect(debugWizardFlows.length).toBeGreaterThanOrEqual(7)
  })

  it('flow id 全局唯一', () => {
    const ids = debugWizardFlows.map((f) => f.id)
    expect(new Set(ids).size).toBe(ids.length)
  })

  it('每条流程至少 3 个检查项', () => {
    for (const flow of debugWizardFlows) {
      expect(flow.steps.length, `流程 ${flow.id}`).toBeGreaterThanOrEqual(3)
    }
  })

  it('step id 在同一条流程内唯一', () => {
    for (const flow of debugWizardFlows) {
      const ids = flow.steps.map((s) => s.id)
      expect(new Set(ids).size, `流程 ${flow.id}`).toBe(ids.length)
    }
  })

  it('处置入口只使用站内绝对路由', () => {
    for (const flow of debugWizardFlows) {
      for (const step of flow.steps) {
        if (!step.action) continue
        expect(step.action.to.startsWith('/'), `${flow.id}/${step.id}`).toBe(true)
        expect(step.action.label.length, `${flow.id}/${step.id}`).toBeGreaterThan(0)
      }
    }
  })

  it('protocol 只取允许的协议集合', () => {
    for (const flow of debugWizardFlows) {
      expect(WIZARD_PROTOCOLS, `流程 ${flow.id}`).toContain(flow.protocol)
    }
  })

  it('至少覆盖 5 种协议', () => {
    const covered = new Set(debugWizardFlows.map((f) => f.protocol))
    expect(covered.size).toBeGreaterThanOrEqual(5)
  })

  it('症状与说明文案非空', () => {
    for (const flow of debugWizardFlows) {
      expect(flow.symptom.trim(), `流程 ${flow.id}`).not.toBe('')
      expect(flow.intro.trim(), `流程 ${flow.id}`).not.toBe('')
    }
  })
})
