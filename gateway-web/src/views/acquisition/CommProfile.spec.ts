/**
 * 通信画像页测试（N-5 合并后）：表格 / 矩阵两种形态共用同一份 stats-report 数据
 *
 * 回归目标：原「通信矩阵」独立页签已合并进本页。若形态切换失效或矩阵拿不到
 * 设备-总线映射，现场表现就是点「矩阵」后空白或所有格子都是「-」。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import CommProfile from './CommProfile.vue'

const mockGetStatsReport = vi.fn()
const mockGetConfig = vi.fn()
const mockListPointQuality = vi.fn()

// W2-#14：本页新增「查看设备健康评分」跳转，需注入 router 才能断言跳转目标
const { pushMock } = vi.hoisted(() => ({ pushMock: vi.fn() }))
vi.mock('vue-router', () => ({
  useRouter: () => ({ push: pushMock }),
}))

vi.mock('@/api/diagnostics', () => ({
  getStatsReportView: (...args: unknown[]) => mockGetStatsReport(...args),
}))

vi.mock('@/api/config', () => ({
  getConfig: (...args: unknown[]) => mockGetConfig(...args),
}))

// 原「时延分析」页合并后，本页还依赖点位质量以累计连续失败点位数
vi.mock('@/api/dataQuality', () => ({
  listPointQuality: (...args: unknown[]) => mockListPointQuality(...args),
}))

const CONFIG = {
  buses: [{ port_name: 'COM1' }, { port_name: 'COM2' }],
  devices: [
    { id: 'd1', bus: 'COM1' },
    { id: 'd2', bus: 'COM2' },
  ],
  points: [
    { device_id: 'd1', interval: 1 },
    { device_id: 'd2', interval: 2 },
  ],
}

function report(deviceStats: Record<string, unknown>[]) {
  return {
    period: 'day',
    startAt: 1_700_000_000_000,
    endAt: 1_700_086_400_000,
    deviceStats,
    busStats: [],
    errorDist: [],
    summary: { totalReads: 0, successRate: null, avgLatency: null, totalDataPoints: 0 },
  }
}

const STATS = [
  { deviceId: 'd1', deviceName: '电表A', protocol: 'modbus', totalReads: 100, success: 96, failed: 4, successRate: 96, avgLatency: 20, dataPoints: 10 },
  { deviceId: 'd2', deviceName: '电表B', protocol: 'modbus', totalReads: 0, success: 0, failed: 0, successRate: null, avgLatency: null, dataPoints: 0 },
]

describe('CommProfile.vue — 表格 / 矩阵形态切换', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetStatsReport.mockResolvedValue(report(STATS))
    mockGetConfig.mockResolvedValue(CONFIG)
    mockListPointQuality.mockResolvedValue([])
  })

  it('默认表格形态展示设备画像', async () => {
    const wrapper = mount(CommProfile)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('电表A')
    expect(text).toContain('优化建议')
    // 双向跳转：画像 → 健康评分（口径区分入口）
    const jump = wrapper.findAll('button').find((b) => b.text().includes('查看设备健康评分'))
    expect(jump).toBeTruthy()
    await jump!.trigger('click')
    expect(pushMock).toHaveBeenCalledWith('/devices?tab=health-score')
    wrapper.unmount()
  })

  it('切到矩阵形态后按设备-总线出格子，成功率落在对应总线列', async () => {
    const wrapper = mount(CommProfile)
    await flushPromises()

    const matrixBtn = wrapper.findAll('.el-radio-button').find((el) => el.text().includes('矩阵'))
    expect(matrixBtn).toBeTruthy()
    await matrixBtn!.find('input').setValue(true)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('采集成功率')
    expect(text).toContain('电表A')
    expect(text).toContain('电表B')
    // 设备 d1 在 COM1、d2 在 COM2
    expect(text).toContain('96%')
    // 交叉格（设备不在该总线）显示「-」，不能冒充 0%
    expect(text).toContain('-')
    wrapper.unmount()
  })

  it('无采集数据时矩阵形态给出空态而不是空表格', async () => {
    mockGetStatsReport.mockResolvedValue(report([]))
    const wrapper = mount(CommProfile)
    await flushPromises()

    const matrixBtn = wrapper.findAll('.el-radio-button').find((el) => el.text().includes('矩阵'))
    expect(matrixBtn).toBeTruthy()
    await matrixBtn!.find('input').setValue(true)
    await flushPromises()

    expect(wrapper.text()).toContain('暂无矩阵数据')
    wrapper.unmount()
  })
})
