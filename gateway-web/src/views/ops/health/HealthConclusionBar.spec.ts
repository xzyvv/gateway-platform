/**
 * 健康结论条契约（R2）。
 *
 * 定位：网关级结论只有一个权威页（运维 › 健康报告），其它页面用这一行摘要 + 深链引用。
 * 这里锁住三件事：分级计数与问题清单同源、没有快照时如实显示「暂无数据」、深链路由固定。
 */
import { describe, it, expect, vi } from 'vitest'
import { mount } from '@vue/test-utils'

const m = vi.hoisted(() => ({ push: vi.fn() }))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: m.push }) }))

import HealthConclusionBar from './HealthConclusionBar.vue'
import type { HealthProblem } from '@/api/status'

const statusOk = { value: 'ok' as const, label: '正常', type: 'success' as const }

function makeProblems(): HealthProblem[] {
  return [
    { level: 'critical', code: 'MQTT_DISCONNECTED', message: 'MQTT 未连接', hint: '检查 broker' },
    { level: 'warning', code: 'QUEUE_BACKLOG', message: '队列积压', hint: '检查上行' },
    { level: 'warning', code: 'BUS_TIMEOUT', message: '总线超时', hint: '检查串口' },
    { level: 'info', code: 'OTA_PENDING', message: '有固件待升级', hint: '升级后观察' },
  ] as HealthProblem[]
}

describe('HealthConclusionBar', () => {
  it('渲染综合状态与分级计数', () => {
    const wrapper = mount(HealthConclusionBar, {
      props: { problems: makeProblems(), status: statusOk },
    })
    expect(wrapper.text()).toContain('正常')
    expect(wrapper.text()).toContain('严重 1')
    expect(wrapper.text()).toContain('警告 2')
    expect(wrapper.text()).toContain('提示 1')
  })

  it('无快照时如实显示「暂无数据」，不伪造 0 级结论', () => {
    const wrapper = mount(HealthConclusionBar, {
      props: {
        problems: [],
        status: { value: 'unknown' as const, label: '暂无数据', type: 'info' as const },
      },
    })
    expect(wrapper.text()).toContain('暂无数据')
    expect(wrapper.text()).toContain('严重 0')
  })

  it('渲染一行摘要与生成时间', () => {
    const wrapper = mount(HealthConclusionBar, {
      props: { problems: [], status: statusOk, summaryText: '3 条总线 · 12/14 台设备在线', generatedAtMs: 1700000000000 },
    })
    expect(wrapper.text()).toContain('3 条总线 · 12/14 台设备在线')
    expect(wrapper.text()).toMatch(/生成于 \d{2}:\d{2}:\d{2}/)
  })

  it('未给摘要/时间时不渲染空占位', () => {
    const wrapper = mount(HealthConclusionBar, { props: { problems: [], status: statusOk } })
    expect(wrapper.find('.hcb-summary').exists()).toBe(false)
    expect(wrapper.find('.hcb-generated').exists()).toBe(false)
  })

  it('点「查看健康报告」深链到运维 › 健康报告页签', async () => {
    const wrapper = mount(HealthConclusionBar, { props: { problems: [], status: statusOk } })
    await wrapper.get('.hcb-link').trigger('click')
    expect(m.push).toHaveBeenCalledWith({ path: '/ops', query: { tab: 'health-report' } })
  })
})
