import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

/**
 * D1：运维 › 总线负载（BusLoad）行为测试。
 *
 * 这页的结论会被用来判断「总线是不是快满了」，最危险的是把「没采样」渲染成 0。
 * 钉住的核心：
 * - 无采样（收发速率≈0）时错误率/延迟降级为「—」，并显式标注「无采样」；
 * - null 一律渲染「—」，不伪造 0 或「—%」；
 * - 使用率分档（<60 正常 / 60-79 警告 / ≥80 危险）；
 * - 单总线下钻只重取这一条，失败时保留旧值并可见报错；
 * - 图表数据未变化时不重复 setOption，卸载时释放实例。
 */

vi.mock('@/api/diagnostics', () => ({
  listBusLoadsView: (...a: unknown[]) => listBusLoads(...a),
  refreshBusLoadsView: (...a: unknown[]) => refreshBusLoads(...a),
  getBusLoadView: (...a: unknown[]) => getBusLoad(...a),
}))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/composables/core/usePolling', () => ({ usePolling: () => ({ start: startPoll }) }))
vi.mock('@/utils/echarts', () => ({
  default: {},
  initChart: (...a: unknown[]) => initChart(...a),
  disposeChart: (...a: unknown[]) => disposeChart(...a),
}))

const showToast = vi.hoisted(() => vi.fn())
const startPoll = vi.hoisted(() => vi.fn())
const initChart = vi.hoisted(() => vi.fn())
const disposeChart = vi.hoisted(() => vi.fn())
const listBusLoads = vi.hoisted(() => vi.fn())
const refreshBusLoads = vi.hoisted(() => vi.fn())
const getBusLoad = vi.hoisted(() => vi.fn())

import BusLoad from './BusLoad.vue'

const setOption = vi.fn()

const BUS_OK = {
  bus: 'COM1',
  type: 'serial',
  utilization: 85,
  txRate: 120.5,
  rxRate: 100.2,
  collisionRate: 2.5,
  avgLatency: 8.4,
  backlog: 3,
  history: [{ time: '2026-11-03T10:00:00', tx: 120, rx: 100 }],
}
const BUS_WARN = {
  ...BUS_OK,
  bus: 'COM2',
  utilization: 65,
  txRate: 10,
  rxRate: 10,
  collisionRate: 0.5,
  avgLatency: 40,
  history: [{ time: '2026-11-03T10:00:00', tx: 10, rx: 10 }],
}
const BUS_IDLE = {
  bus: 'COM3',
  type: 'serial',
  utilization: 6.2,
  txRate: 0,
  rxRate: 0,
  collisionRate: 100,
  avgLatency: null,
  backlog: null,
  history: [{ time: '2026-11-03T10:00:00', tx: 0, rx: 0 }],
}
const BUS_NULL = {
  bus: 'TCP-1',
  type: 'tcp',
  utilization: null,
  txRate: null,
  rxRate: null,
  collisionRate: null,
  avgLatency: null,
  backlog: null,
  history: [],
}

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(BusLoad)
}

describe('BusLoad 总线负载', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    setOption.mockClear()
    initChart.mockReturnValue({ setOption })
    listBusLoads.mockResolvedValue([BUS_OK, BUS_WARN, BUS_IDLE])
    refreshBusLoads.mockResolvedValue([BUS_OK, BUS_WARN, BUS_IDLE])
    getBusLoad.mockResolvedValue({ ...BUS_OK, utilization: 42 })
  })

  it('无数据时不渲染任何总线卡片', async () => {
    listBusLoads.mockResolvedValue([])
    const wrapper = mountPage()
    await settle()

    expect(wrapper.findAll('.bus-card')).toHaveLength(0)
  })

  it('无采样总线：错误率/延迟显示「—」并标注「无采样」，不渲染成 100% 错误', async () => {
    listBusLoads.mockResolvedValue([BUS_IDLE])
    const wrapper = mountPage()
    await settle()

    const card = wrapper.find('.bus-card')
    expect(card.text()).toContain('无采样')
    expect(card.text()).toContain('使用率 6.2%')
    // 两个指标都为「—」，且不出现「—%」或 100.00%
    const errorMetric = card.findAll('.metric')[2]!
    const latencyMetric = card.findAll('.metric')[3]!
    expect(errorMetric.text()).toBe('错误率—')
    expect(latencyMetric.text()).toContain('—')
    expect(card.text()).not.toContain('100.00%')
    expect(card.text()).not.toContain('—%')
  })

  it('有采样总线：速率/错误率/延迟按精度格式化，使用率分档着色', async () => {
    const wrapper = mountPage()
    await settle()

    const cards = wrapper.findAll('.bus-card')
    expect(cards[0]!.text()).toContain('120.5 包/s')
    expect(cards[0]!.text()).toContain('100.2 包/s')
    expect(cards[0]!.text()).toContain('2.50%')
    expect(cards[0]!.text()).toContain('8 ms')
    expect(cards[0]!.find('.utilization').classes()).toContain('critical')
    expect(cards[0]!.find('.utilization').text()).toBe('使用率 85.0%')
    expect(cards[0]!.findAll('.metric')[2]!.find('.metric-value').classes()).toContain('text-warning')

    expect(cards[1]!.find('.utilization').classes()).toContain('warning')
    expect(cards[1]!.findAll('.metric')[2]!.find('.metric-value').classes()).not.toContain('text-warning')
  })

  it('字段为 null 时统一渲染「—」，不伪造 0 或「—%」', async () => {
    listBusLoads.mockResolvedValue([BUS_NULL])
    const wrapper = mountPage()
    await settle()

    const card = wrapper.find('.bus-card')
    expect(card.find('.utilization').text()).toBe('使用率 —')
    expect(card.find('.utilization').classes()).toContain('unknown')
    expect(card.text()).not.toContain('—%')
    const metrics = card.findAll('.metric')
    expect(metrics[0]!.text()).toContain('—')
    expect(metrics[4]!.text()).toBe('数据积压—')
  })

  it('列表加载失败：给出可见错误，页面不崩', async () => {
    listBusLoads.mockRejectedValue(new Error('list 500'))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.exists()).toBe(true)
    expect(wrapper.findAll('.bus-card')).toHaveLength(0)
    expect(showToast).toHaveBeenCalledWith(new Error('list 500'), '加载总线负载失败')
  })

  it('刷新按钮走刷新接口并更新卡片；失败时报「刷新失败」', async () => {
    const wrapper = mountPage()
    await settle()
    refreshBusLoads.mockResolvedValueOnce([{ ...BUS_OK, utilization: 30 }])
    await wrapper.find('.busload-toolbar button').trigger('click')
    await settle()

    expect(refreshBusLoads).toHaveBeenCalledOnce()
    expect(wrapper.find('.bus-card').text()).toContain('使用率 30.0%')

    refreshBusLoads.mockRejectedValueOnce(new Error('refresh 500'))
    await wrapper.find('.busload-toolbar button').trigger('click')
    await settle()
    expect(showToast).toHaveBeenCalledWith(new Error('refresh 500'), '刷新失败')
  })

  it('单总线下钻只请求这一条，其它总线不受影响', async () => {
    const wrapper = mountPage()
    await settle()
    expect(wrapper.findAll('.bus-card')).toHaveLength(3)

    await wrapper.findAll('.bus-refresh')[0]!.trigger('click')
    await settle()

    expect(getBusLoad).toHaveBeenCalledTimes(1)
    expect(getBusLoad).toHaveBeenCalledWith('COM1')
    expect(listBusLoads).toHaveBeenCalledTimes(1)
    expect(refreshBusLoads).not.toHaveBeenCalled()
    expect(wrapper.findAll('.bus-card')[0]!.text()).toContain('使用率 42.0%')
    expect(wrapper.findAll('.bus-card')[1]!.text()).toContain('使用率 65.0%')
  })

  it('单总线刷新失败：保留旧值并点名是哪条总线', async () => {
    getBusLoad.mockRejectedValue(new Error('timeout'))
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('.bus-refresh')[0]!.trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('timeout'), '加载总线 COM1 负载失败')
    expect(wrapper.findAll('.bus-card')[0]!.text()).toContain('使用率 85.0%')
  })

  it('图表：骨架只注入一次，数据未变化时不再重复 setOption', async () => {
    const wrapper = mountPage()
    await settle()
    // 首屏：每张图 1 次骨架 + 1 次局部数据
    expect(initChart).toHaveBeenCalledTimes(3)
    expect(setOption).toHaveBeenCalledTimes(6)

    setOption.mockClear()
    refreshBusLoads.mockResolvedValueOnce([BUS_OK, BUS_WARN, BUS_IDLE])
    await wrapper.find('.busload-toolbar button').trigger('click')
    await settle()
    expect(setOption).not.toHaveBeenCalled()
  })

  it('卸载时释放全部图表实例，并启动常驻轮询', async () => {
    const wrapper = mountPage()
    await settle()
    expect(startPoll).toHaveBeenCalledOnce()

    wrapper.unmount()
    expect(disposeChart).toHaveBeenCalledTimes(3)
  })
})
