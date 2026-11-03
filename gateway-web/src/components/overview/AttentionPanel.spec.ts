/**
 * 异常聚合面板的渲染与跳转契约。
 *
 * 面板在整个方案里承担的是「排障入口」职责，所以锁三件事：
 *   1. 无异常时显式说「一切正常」，不渲染空白（空白会被误读成没加载出来）
 *   2. 每条异常都带可点击的处理入口，且跳到派生层给出的目标路由
 *   3. error 与 warning 的数量分别统计，徽标文案随严重度变化
 *
 * 变异验证：
 *   - 把 `signals.length === 0` 的空态分支去掉 → 用例 1 失败
 *   - 把 go() 里的 router.push 改成 no-op → 用例 2 失败
 *   - 把 errorCount 的过滤条件改成统计全部 → 用例 3 失败
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount } from '@vue/test-utils'
import AttentionPanel from './AttentionPanel.vue'
import type { HealthSignal } from '@/composables/dashboard/useHealthSignals'

const m = vi.hoisted(() => ({ push: vi.fn() }))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: m.push }),
}))

function sig(patch: Partial<HealthSignal> = {}): HealthSignal {
  return {
    id: 'cpu-high',
    severity: 'warning',
    title: 'CPU 占用 65.0%',
    detail: '已超过 60% 预警线。',
    action: '查看运行指标',
    to: { path: '/ops', query: { tab: 'metrics-dashboard' } },
    ...patch,
  }
}

describe('AttentionPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('无异常时显示明确的无异常文案，而不是空白', () => {
    const wrapper = mount(AttentionPanel, { props: { signals: [] } })

    expect(wrapper.find('.attention-ok').exists()).toBe(true)
    expect(wrapper.text()).toContain('当前无异常')
    // 空态下不该出现徽标（0 项异常不需要提示）
    expect(wrapper.find('.sev-badge').exists()).toBe(false)
    expect(wrapper.find('.attention-list').exists()).toBe(false)
  })

  it('每条异常渲染标题、影响说明与可点击的处理入口', () => {
    const wrapper = mount(AttentionPanel, { props: { signals: [sig()] } })

    expect(wrapper.find('.attention-ok').exists()).toBe(false)
    expect(wrapper.findAll('.attention-item')).toHaveLength(1)
    expect(wrapper.text()).toContain('CPU 占用 65.0%')
    expect(wrapper.text()).toContain('已超过 60% 预警线。')
    expect(wrapper.text()).toContain('查看运行指标')
  })

  it('点击处理入口跳到派生层给出的目标路由', async () => {
    const wrapper = mount(AttentionPanel, { props: { signals: [sig()] } })

    await wrapper.get('.item-action').trigger('click')

    expect(m.push).toHaveBeenCalledWith({ path: '/ops', query: { tab: 'metrics-dashboard' } })
  })

  it('error 与 warning 分别统计，徽标只报异常数', () => {
    const wrapper = mount(AttentionPanel, {
      props: {
        signals: [
          sig({ id: 'a', severity: 'error' }),
          sig({ id: 'b', severity: 'error' }),
          sig({ id: 'c', severity: 'warning' }),
        ],
      },
    })

    expect(wrapper.get('.sev-badge').text()).toContain('2 项异常')
    expect(wrapper.findAll('.attention-item.sev-error')).toHaveLength(2)
    expect(wrapper.findAll('.attention-item.sev-warning')).toHaveLength(1)
  })

  it('只有 warning 时徽标改报提醒数', () => {
    const wrapper = mount(AttentionPanel, {
      props: { signals: [sig({ severity: 'warning' })] },
    })
    expect(wrapper.get('.sev-badge').text()).toContain('1 项提醒')
  })
})