import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DataTable from '@/components/DataTable.vue'

/**
 * D1：运维监控 › 健康报告（网关健康结论唯一页）行为测试。
 *
 * R2 收敛后，这一页要承担"现场只看这一页就能判断网关好不好"的职责，所以钉住：
 *   1. 关键数字不能显示成 NaN% / undefined（缺失即显示「—」）；
 *   2. 队列背压要能被一眼看到（预警阈值来自 healthVerdict，与交付自检同源）；
 *   3. 首次加载中显示「读取中」，不能显示成「暂无数据」；
 *   4. 问题清单复用共享组件（与自诊断/网关信息同一份分级与文案）。
 */

const { state } = vi.hoisted(() => ({
  state: {
    report: null as Record<string, unknown> | null,
    loading: false,
    overall: { label: '读取中', type: 'info', value: 'unknown' },
    refresh: vi.fn(),
  },
}))

vi.mock('@/composables/system/useHealthReport', async () => {
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return {
    useHealthReport: () => ({
      report: ref(state.report),
      loading: ref(state.loading),
      overallStatus: ref(state.overall),
      refresh: state.refresh,
    }),
  }
})

import HealthReport from './HealthReport.vue'
import HealthProblemList from './HealthProblemList.vue'

function report(over: Record<string, unknown> = {}) {
  return {
    generated_at_ms: Date.UTC(2026, 0, 1, 0, 0, 0),
    summary: {
      total_devices: 4,
      collecting_devices: 3,
      online_devices: 3,
      bus_count: 2,
      queue_len: 12,
      total_reads: 100,
      success_rate: 98.5,
      disk_level: 'ok',
      disk_free_pct: 72.5,
      disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
    },
    components: { mqtt: 'ok', modbus: 'degraded', iec104: 'down' },
    buses: [
      { bus: 'bus-1', protocol: 'modbus', online_devices: 2, total_devices: 2, total_reads: 60, success_rate: 99.2, avg_latency_ms: 12, timeout_rate: 0.4, last_error_code: null, reconnect_count: 0 },
      { bus: 'bus-2', protocol: 'iec104', online_devices: 1, total_devices: 2, total_reads: 40, success_rate: null, avg_latency_ms: null, timeout_rate: null, last_error_code: null, reconnect_count: 2 },
    ],
    problems: [{ severity: 'critical', title: '队列积压', hint: '检查北向上行' }],
    ...over,
  }
}

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('HealthReport 健康结论页', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    state.report = report()
    state.loading = false
    state.overall = { label: '正常', type: 'success', value: 'ok' }
    state.refresh.mockResolvedValue(undefined)
  })

  it('汇总卡片展示设备在线、队列、成功率、总线/采集次数', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('3/4')
    expect(text).toContain('12 条')
    expect(text).toContain('98.5%')
    expect(text).toContain('2 / 100')
  })

  it('缺失的比率显示「—」，不得出现 NaN%', async () => {
    state.report = report({
      summary: {
        total_devices: 0,
        collecting_devices: 0,
        online_devices: 0,
        bus_count: 0,
        queue_len: 0,
        total_reads: 0,
        success_rate: null,
        disk_level: 'ok',
        disk_free_pct: null,
        disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
      },
    })
    const wrapper = mount(HealthReport)
    await settle()
    expect(wrapper.text()).toContain('—')
    expect(wrapper.text()).not.toContain('NaN')
    expect(wrapper.text()).not.toContain('undefined')
  })

  it('队列存量达到预警阈值时标黄提示', async () => {
    state.report = report({
      summary: {
        total_devices: 4,
        collecting_devices: 3,
        online_devices: 3,
        bus_count: 2,
        queue_len: 10000,
        total_reads: 100,
        success_rate: 98.5,
        disk_level: 'ok',
        disk_free_pct: 72.5,
        disk_thresholds: { warn: 25, critical: 15, blocking: 10 },
      },
    })
    const wrapper = mount(HealthReport)
    await settle()
    const values = wrapper.findAll('.mcard .v')
    // 第二张卡是「队列存量」
    expect(values[1]?.classes()).toContain('warn')
  })

  it('组件状态按 ok / degraded / down 渲染', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('mqtt：ok')
    expect(text).toContain('modbus：degraded')
    expect(text).toContain('iec104：down')
  })

  it('总线明细按行渲染，缺失值统一显示「—」', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const table = wrapper.findComponent(DataTable)
    const rows = (table.props() as unknown as { data: Array<Record<string, unknown>> }).data
    expect(rows).toHaveLength(2)
    expect(rows[0]?.bus).toBe('bus-1')
    expect(wrapper.text()).toContain('bus-2')
  })

  it('问题清单复用共享组件，分级与文案与自诊断一致', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const list = wrapper.findComponent(HealthProblemList)
    expect(list.exists()).toBe(true)
    expect(list.props('problems')).toEqual([
      { severity: 'critical', title: '队列积压', hint: '检查北向上行' },
    ])
  })

  it('首次加载中显示「读取中」，不显示成「暂无数据」', async () => {
    state.report = null
    state.loading = true
    state.overall = { label: '读取中', type: 'info', value: 'unknown' }
    const wrapper = mount(HealthReport)
    await settle()
    expect(wrapper.text()).toContain('读取中')
    expect(wrapper.text()).not.toContain('暂无数据')
  })

  it('手动刷新强制绕过缓存', async () => {
    const wrapper = mount(HealthReport)
    await settle()
    const buttons = wrapper.findAll('button')
    const refreshBtn = buttons.find((b) => b.text() === '刷新')
    expect(refreshBtn).toBeTruthy()
    await refreshBtn!.trigger('click')
    await settle()
    expect(state.refresh).toHaveBeenCalledWith(true)
  })
})
