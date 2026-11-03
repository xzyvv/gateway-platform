import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DiskHealthChart from './DiskHealthChart.vue'

// Mock echarts
const mockChart = {
  setOption: vi.fn(),
  resize: vi.fn(),
  dispose: vi.fn(),
}
vi.mock('@/utils/echarts', () => ({
  default: { init: () => mockChart, graphic: { LinearGradient: class {} } },
  // 页面统一从 initChart 初始化（暗色主题注入包装），mock 与 default.init 同返回
  initChart: () => mockChart,
  // 卸载走 disposeChart（同步摘除 liveCharts 注册表）
  disposeChart: vi.fn((c: { dispose: () => void }) => c?.dispose?.()),
}))

// Mock API
const mockGetDiskHistory = vi.fn()
vi.mock('@/api/status', () => ({
  getDiskHistory: () => mockGetDiskHistory(),
}))

/** 读取组件内部 loading（script setup 无公开类型，沿用仓库既有 setupState 约定） */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function loadingOf(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState.loading
}

describe('DiskHealthChart.vue — D10 磁盘健康', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  /**
   * 回归：开启自动刷新后每 30s 触发一次 load，原实现每轮都置 loading=true，
   * 而 loading 绑在整块的 v-loading 上 → 磁盘健康面板每隔 30 秒被蒙层覆盖一次。
   */
  it('自动刷新(30s)为静默路径：轮询请求挂起期间不进入 loading', async () => {
    // 只伪造 setInterval，保留真实 setTimeout（flushPromises 依赖它）
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    try {
      mockGetDiskHistory.mockResolvedValue({ count: 0, samples: [] })
      const wrapper = mount(DiskHealthChart)
      await flushPromises()
      expect(loadingOf(wrapper)).toBe(false)

      // 开启自动刷新（默认关闭）
      const toggle = wrapper.findAll('button').find((b) => b.text().includes('自动刷新'))
      expect(toggle).toBeTruthy()
      await toggle!.trigger('click')

      // 定时器轮次：请求挂起期间不得置 loading，也不得出现蒙层/骨架（已有内容必须保留）
      let resolveNext!: (v: unknown) => void
      mockGetDiskHistory.mockReturnValue(new Promise((r) => { resolveNext = r }))
      vi.advanceTimersByTime(30_000)
      expect(loadingOf(wrapper)).toBe(false)
      expect(wrapper.find('.el-loading-mask').exists()).toBe(false)
      expect(wrapper.find('.sk-chart-rect').exists()).toBe(false)
      expect(wrapper.text()).toContain('暂无磁盘采样数据')

      resolveNext({ count: 0, samples: [] })
      await flushPromises()
      expect(loadingOf(wrapper)).toBe(false)
      wrapper.unmount()
    } finally {
      vi.useRealTimers()
    }
  })

  it('首屏加载中渲染骨架屏，已有数据刷新不闪骨架', async () => {
    let resolveFirst!: (v: unknown) => void
    mockGetDiskHistory.mockReturnValue(new Promise((r) => { resolveFirst = r }))
    const wrapper = mount(DiskHealthChart)
    await flushPromises()

    // 首屏无数据：用等结构骨架占位，而不是覆盖整块的转圈蒙层
    expect(wrapper.find('.sk-chart-rect').exists()).toBe(true)
    expect(wrapper.find('.el-loading-mask').exists()).toBe(false)
    expect(wrapper.find('.chart').exists()).toBe(false)

    resolveFirst({ count: 1, samples: [{ ts: 1700000000000, free_pct: 50 }] })
    await flushPromises()
    await nextTick()
    expect(wrapper.find('.sk-chart-rect').exists()).toBe(false)
    expect(wrapper.text()).toContain('50%')

    // 已有数据时手动刷新：loading 为 true 但真实内容不得被骨架替换
    let resolveRefresh!: (v: unknown) => void
    mockGetDiskHistory.mockReturnValue(new Promise((r) => { resolveRefresh = r }))
    const refreshBtn = wrapper.findAll('button').find((b) => b.text().trim() === '刷新')
    expect(refreshBtn).toBeTruthy()
    await refreshBtn!.trigger('click')
    expect(loadingOf(wrapper)).toBe(true)
    expect(wrapper.find('.sk-chart-rect').exists()).toBe(false)
    expect(wrapper.text()).toContain('50%')

    resolveRefresh({ count: 1, samples: [{ ts: 1700000000000, free_pct: 50 }] })
    await flushPromises()
    expect(loadingOf(wrapper)).toBe(false)
    wrapper.unmount()
  })

  it('加载成功后渲染概要卡片与当前剩余空间', async () => {
    mockGetDiskHistory.mockResolvedValue({
      count: 3,
      samples: [
        { ts: 1700000000000, free_pct: 80.0 },
        { ts: 1700000060000, free_pct: 75.5 },
        { ts: 1700000120000, free_pct: 70.2 },
      ],
    })
    const wrapper = mount(DiskHealthChart)
    await flushPromises()
    await nextTick()

    // 当前剩余空间取最后一条采样
    expect(wrapper.text()).toContain('70%')
    // 采样点数量
    expect(wrapper.text()).toContain('3')
    // 危险阈值来自常量 DANGER_THRESHOLD=10
    expect(wrapper.text()).toContain('10%')
    // 调用了 echarts.setOption
    expect(mockChart.setOption).toHaveBeenCalled()
  })

  it('采样为空时显示空状态', async () => {
    mockGetDiskHistory.mockResolvedValue({ count: 0, samples: [] })
    const wrapper = mount(DiskHealthChart)
    await flushPromises()

    expect(wrapper.text()).toContain('暂无磁盘采样数据')
    expect(mockChart.setOption).not.toHaveBeenCalled()
  })

  it('API 失败时提示错误', async () => {
    mockGetDiskHistory.mockRejectedValue(new Error('network'))
    const wrapper = mount(DiskHealthChart)
    await flushPromises()

    // ElMessage 为全局提示，这里验证组件未崩溃且仍可渲染
    expect(wrapper.exists()).toBe(true)
  })

  it('剩余空间低于 30% 时数字为警告色，低于 10% 为危险色', async () => {
    mockGetDiskHistory.mockResolvedValue({
      count: 1,
      samples: [{ ts: 1700000000000, free_pct: 25 }],
    })
    const wrapper = mount(DiskHealthChart)
    await flushPromises()
    await nextTick()

    const valueEl = wrapper.find('.summary-value')
    // 25% 应走 warning 分支
    expect(valueEl.attributes('style')).toContain('warning')
  })

  it('组件卸载时释放 echarts 实例', async () => {
    mockGetDiskHistory.mockResolvedValue({
      count: 1,
      samples: [{ ts: 1700000000000, free_pct: 50 }],
    })
    const wrapper = mount(DiskHealthChart)
    await flushPromises()
    await nextTick()

    wrapper.unmount()
    expect(mockChart.dispose).toHaveBeenCalled()
  })
})
