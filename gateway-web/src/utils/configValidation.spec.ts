/**
 * configValidation 单测（P4：规则单一来源在后端）
 *
 * 前端只负责调用 /api/config/validate 并把字段路径映射为页签，
 * 因此这里只验证「透传 + 页签推导 + 阻断判断」，不再重复断言阈值规则。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'

const m = vi.hoisted(() => ({ validateConfigDraft: vi.fn() }))

vi.mock('@/api/config', () => ({
  validateConfigDraft: (...a: unknown[]) => m.validateConfigDraft(...a),
}))

import { validateConfig, hasBlockingIssues } from './configValidation'

const emptyReport = { valid: true, errors: 0, warnings: 0, issues: [] }

describe('configValidation（后端驱动）', () => {
  beforeEach(() => m.validateConfigDraft.mockReset())

  it('透传后端问题并按字段前缀映射配置页签', async () => {
    m.validateConfigDraft.mockResolvedValue({
      valid: false,
      errors: 2,
      warnings: 1,
      issues: [
        { level: 'error', field: 'buses[1].port_name', message: '总线端口重复' },
        { level: 'error', field: 'points[p1].bus', message: '引用的总线不存在' },
        { level: 'warning', field: 'telemetry.sample_ratio', message: '采样率越界' },
      ],
    })
    const issues = await validateConfig({} as never)
    expect(issues.map((i) => i.tab)).toEqual(['buses', 'points', 'telemetry'])
    expect(issues[0]!.message).toBe('总线端口重复')
    expect(issues[0]!.level).toBe('error')
  })

  it('无法映射前缀的字段不带定位页签', async () => {
    m.validateConfigDraft.mockResolvedValue({
      valid: false,
      errors: 1,
      warnings: 0,
      issues: [{ level: 'error', field: 'mqtt.broker', message: 'broker 非法' }],
    })
    const issues = await validateConfig({} as never)
    expect(issues[0]!.tab).toBeUndefined()
  })

  it('后端无 issues 时返回空列表', async () => {
    m.validateConfigDraft.mockResolvedValue(emptyReport)
    expect(await validateConfig({} as never)).toEqual([])
  })

  it('hasBlockingIssues 区分 error / warning', () => {
    expect(hasBlockingIssues([{ level: 'error', field: '', message: '' }])).toBe(true)
    expect(hasBlockingIssues([{ level: 'warning', field: '', message: '' }])).toBe(false)
    expect(hasBlockingIssues([])).toBe(false)
  })
})
