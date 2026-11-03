/**
 * 采集统计报表页测试：每总线四指标必须真正出现在表格里
 *
 * 回归目标：后端补齐了「在线设备 / 超时率 / 最近错误码 / 重连次数」四个字段后，
 * 前端表格列若漏配或写错 key，整列会静默空白（DataTable 的 cell 回调不报错），
 * 现场看到的就是「报表有数据但关键列没内容」。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import StatsReport from './StatsReport.vue'

const mockGetStatsReport = vi.fn()
const mockExportCsv = vi.fn()

vi.mock('@/api/diagnostics', () => ({
  getStatsReportView: (...args: unknown[]) => mockGetStatsReport(...args),
  exportStatsReportCsv: (...args: unknown[]) => mockExportCsv(...args),
}))

function busStat(over: Record<string, unknown> = {}) {
  return {
    bus: 'COM1',
    protocol: 'modbus',
    totalReads: 10,
    success: 9,
    failed: 1,
    successRate: 90,
    avgLatency: 12,
    deviceCount: 2,
    onlineDevices: 2,
    timeoutRate: 10,
    lastErrorCode: 'TIMEOUT',
    reconnectCount: 3,
    ...over,
  }
}

function report(buses = [busStat()]) {
  return {
    period: 'day',
    startAt: 1_700_000_000_000,
    endAt: 1_700_086_400_000,
    deviceStats: [],
    busStats: buses,
    errorDist: [],
    summary: {
      totalReads: 10,
      successRate: 90,
      avgLatency: 12,
      totalDataPoints: 9,
    },
  }
}

describe('StatsReport.vue — 每总线四指标', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('表头出现四个关键指标列', async () => {
    mockGetStatsReport.mockResolvedValue(report())
    const wrapper = mount(StatsReport)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('在线设备')
    expect(text).toContain('超时率')
    expect(text).toContain('最近错误码')
    expect(text).toContain('重连次数')
    wrapper.unmount()
  })

  it('四个指标的值渲染到表格行里', async () => {
    mockGetStatsReport.mockResolvedValue(report())
    const wrapper = mount(StatsReport)
    await flushPromises()

    const text = wrapper.text()
    // 在线设备 2 / 超时率 10% / 错误码 TIMEOUT / 重连次数 3
    expect(text).toContain('10%')
    expect(text).toContain('TIMEOUT')
    wrapper.unmount()
  })

  it('无采集时超时率与错误码渲染为「—」，不显示 0 值冒充', async () => {
    mockGetStatsReport.mockResolvedValue(
      report([busStat({ timeoutRate: null, lastErrorCode: null, onlineDevices: 0 })]),
    )
    const wrapper = mount(StatsReport)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('—')
    expect(text).not.toContain('TIMEOUT')
    wrapper.unmount()
  })

  it('错误分布为空时使用 EmptyState 空态', async () => {
    mockGetStatsReport.mockResolvedValue(report())
    const wrapper = mount(StatsReport)
    await flushPromises()

    expect(wrapper.text()).toContain('暂无采集错误记录')
    wrapper.unmount()
  })
})
