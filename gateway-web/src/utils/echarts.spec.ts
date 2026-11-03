import { describe, it, expect, beforeEach, vi } from 'vitest'

/**
 * #1 修复（暗色主题运行时切换）核心单测。
 *
 * 实际跑不起来完整 echarts：jsdom 没有完整 Canvas API + 测量 API，
 * `echarts.init(realCanvas)` 在测试环境会抛 "getContext is not a function"
 * 之类的错。本 spec 直接 mock `echarts/core` 后只验证我们的注册表与
 * 主题切换回调逻辑，独立于真实 echarts 渲染。
 */

// initMock 每次调用都返回**新对象**——避免 Set 去重把同引用当成同一图表
const initMock = vi.fn((_dom: unknown, _theme?: string) => {
  return {
    getOption: vi.fn(() => ({ xAxis: { type: 'category' } })),
    setOption: vi.fn(),
    resize: vi.fn(),
    dispose: vi.fn(),
  }
})

// 单独注册用的两个 chart（用于直接 registerChart/unregisterChart 的测试）
const mockChartA = {
  getOption: vi.fn(() => ({ xAxis: { type: 'category' } })),
  setOption: vi.fn(),
  dispose: vi.fn(),
}
const mockChartB = {
  getOption: vi.fn(() => ({ yAxis: { type: 'value' } })),
  setOption: vi.fn(),
  dispose: vi.fn(),
}

vi.mock('echarts/core', () => ({
  default: {
    init: (...args: unknown[]) => initMock(args[0], args[1] as string | undefined),
    registerTheme: vi.fn(),
    use: vi.fn(),
    graphic: { LinearGradient: class {} },
  },
  init: (...args: unknown[]) => initMock(args[0], args[1] as string | undefined),
  registerTheme: vi.fn(),
  use: vi.fn(),
  graphic: { LinearGradient: class {} },
}))

vi.mock('echarts/charts', () => ({
  LineChart: 'LineChart',
  BarChart: 'BarChart',
  CustomChart: 'CustomChart',
  GraphChart: 'GraphChart',
}))
vi.mock('echarts/components', () => ({
  GridComponent: 'g',
  TooltipComponent: 't',
  LegendComponent: 'l',
  DataZoomComponent: 'z',
  TitleComponent: 'tt',
  MarkLineComponent: 'm',
}))
vi.mock('echarts/renderers', () => ({ CanvasRenderer: 'cv' }))

import * as echartsMod from './echarts'
const {
  initChart,
  registerChart,
  unregisterChart,
  applyEchartsTheme,
  __liveChartCountForTests,
  __resetLiveChartsForTests,
} = echartsMod

function makeDom(): HTMLDivElement {
  return document.createElement('div')
}

describe('echarts — #1 修复：图表注册表 + 主题运行时切换', () => {
  beforeEach(() => {
    initMock.mockClear()
    mockChartA.getOption.mockClear()
    mockChartA.setOption.mockClear()
    mockChartB.getOption.mockClear()
    mockChartB.setOption.mockClear()
    __resetLiveChartsForTests()
    document.documentElement.classList.remove('dark')
  })

  it('initChart 自动把图表加入活动注册表（每次返回新对象，避免 Set 去重）', () => {
    initChart(makeDom())
    initChart(makeDom())
    expect(__liveChartCountForTests()).toBe(2)
  })

  it('手动 registerChart / unregisterChart 维护注册表', () => {
    registerChart(mockChartA as never)
    registerChart(mockChartB as never)
    expect(__liveChartCountForTests()).toBe(2)
    unregisterChart(mockChartA as never)
    expect(__liveChartCountForTests()).toBe(1)
    unregisterChart(mockChartA as never) // 幂等
    expect(__liveChartCountForTests()).toBe(1)
  })

  it('applyEchartsTheme：所有活动图表被 setOption 重绘', () => {
    registerChart(mockChartA as never)
    registerChart(mockChartB as never)

    applyEchartsTheme(true)

    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    expect(mockChartB.setOption).toHaveBeenCalledTimes(1)
    const aArg = mockChartA.setOption.mock.calls[0]?.[1]
    const bArg = mockChartB.setOption.mock.calls[0]?.[1]
    // 第二个参数 = true（notMerge 强制重绘）
    expect(aArg).toBe(true)
    expect(bArg).toBe(true)
  })

  it('applyEchartsTheme：暗色状态由 isDarkMode() 自动判定（不带参数）', () => {
    registerChart(mockChartA as never)
    document.documentElement.classList.add('dark')
    applyEchartsTheme() // 不传参 → 读 isDarkMode → true

    expect(mockChartA.getOption).toHaveBeenCalled()
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    document.documentElement.classList.remove('dark')
  })

  it('applyEchartsTheme：失效实例被自动从注册表移除', () => {
    const brokenChart = {
      getOption: vi.fn(() => ({ xAxis: { type: 'category' } })),
      setOption: vi.fn(() => {
        throw new Error('Component is disposed')
      }),
      resize: vi.fn(),
      dispose: vi.fn(),
    }
    registerChart(mockChartA as never)
    registerChart(brokenChart as never)
    expect(__liveChartCountForTests()).toBe(2)

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {})

    applyEchartsTheme(true)

    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    expect(brokenChart.setOption).toHaveBeenCalledTimes(1)
    expect(__liveChartCountForTests()).toBe(1)
    warnSpy.mockRestore()
  })

  it('applyEchartsTheme：listener 内 unregisterChart 不影响本次迭代（拷贝 + snapshot）', () => {
    const aWithSelfUnreg = {
      getOption: vi.fn(() => ({})),
      setOption: vi.fn(),
    }
    registerChart(aWithSelfUnreg as never)
    registerChart(mockChartB as never)
    expect(__liveChartCountForTests()).toBe(2)

    aWithSelfUnreg.setOption.mockImplementation(() => {
      // 模拟 listener 内部反向注销自己（已被 snapshot 出来不影响本轮）
      unregisterChart(aWithSelfUnreg as never)
    })

    applyEchartsTheme(true)

    expect(aWithSelfUnreg.setOption).toHaveBeenCalledTimes(1)
    expect(mockChartB.setOption).toHaveBeenCalledTimes(1)
    expect(__liveChartCountForTests()).toBe(1)
  })

  it('initChart 在暗色模式下应把 "gw-dark" 主题传给底层 init', () => {
    document.documentElement.classList.add('dark')
    initChart(makeDom())
    expect(initMock.mock.calls[0]?.[1]).toBe('gw-dark')
    document.documentElement.classList.remove('dark')
    initChart(makeDom())
    // 浅色不再传 undefined：canvas 不解析 var() 后，浅色轴/图例值由 gw-light
    // 主题显式钉死（chartTheme.CHART_LIGHT），防止 echarts 默认值跨版本漂移
    expect(initMock.mock.calls[1]?.[1]).toBe('gw-light')
  })

  it('applyEchartsTheme：同一实例连续两次应用相同主题，setOption 只执行一次', () => {
    registerChart(mockChartA as never)

    applyEchartsTheme(true)
    applyEchartsTheme(true)

    expect(mockChartA.getOption).toHaveBeenCalledTimes(1)
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)

    // 主题真的变化时才再次重绘
    applyEchartsTheme(false)
    expect(mockChartA.setOption).toHaveBeenCalledTimes(2)
  })

  it('applyEchartsTheme：多图表按实例比较（WeakMap，不是全局单标志）', () => {
    registerChart(mockChartA as never)
    registerChart(mockChartB as never)

    applyEchartsTheme(true)
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    expect(mockChartB.setOption).toHaveBeenCalledTimes(1)

    // 重复同一主题：两个实例都被跳过
    applyEchartsTheme(true)
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    expect(mockChartB.setOption).toHaveBeenCalledTimes(1)

    // 图中应用过暗色后，新增注册的实例（无记录）首次调用必须被应用
    const lateChart = {
      getOption: vi.fn(() => ({ xAxis: { type: 'category' } })),
      setOption: vi.fn(),
      isDisposed: vi.fn(() => false),
    }
    registerChart(lateChart as never)
    applyEchartsTheme(true)
    expect(lateChart.setOption).toHaveBeenCalledTimes(1)
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
  })

  it('applyEchartsTheme：isDisposed 的实例被跳过并移出注册表', () => {
    const disposedChart = {
      getOption: vi.fn(() => ({})),
      setOption: vi.fn(),
      isDisposed: vi.fn(() => true),
    }
    registerChart(mockChartA as never)
    registerChart(disposedChart as never)

    applyEchartsTheme(true)

    expect(disposedChart.setOption).not.toHaveBeenCalled()
    expect(mockChartA.setOption).toHaveBeenCalledTimes(1)
    expect(__liveChartCountForTests()).toBe(1)
  })
})
