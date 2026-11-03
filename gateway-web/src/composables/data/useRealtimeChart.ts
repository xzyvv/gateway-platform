import { computed, ref } from 'vue'
import echarts, { initChart, disposeChart, APPEND_THROTTLE_MS } from '@/utils/echarts'
import { tooltipStyle, defaultGrid, valueAxisStyle, categoryAxisStyle, legendStyle } from '@/utils/chartStyle'
import { formatTimeOfDay } from '@/utils/datetime'
import { logError } from '@/utils/logger'
import { compactAxis } from '@/utils/realtimeFormat'
import type { DataPoint } from '@/api/types'

/** 每个点位保留的历史点位数（图表窗口 60 点） */
const HISTORY_CAP = 60
/** 图表只画前 5 个点位 */
const TOP_N = 5

const COLORS = ['var(--color-primary)', 'var(--color-success)', 'var(--color-warning)', 'var(--color-danger)', 'var(--text-secondary)']
const AREA_COLORS = [
  ['rgba(99, 102, 241, 0.25)', 'rgba(99, 102, 241, 0.02)'],
  ['rgba(103, 194, 58, 0.2)', 'rgba(103, 194, 58, 0.02)'],
  ['rgba(230, 162, 60, 0.2)', 'rgba(230, 162, 60, 0.02)'],
  ['rgba(245, 108, 108, 0.2)', 'rgba(245, 108, 108, 0.02)'],
  ['rgba(144, 147, 153, 0.2)', 'rgba(144, 147, 153, 0.02)'],
]

interface ChartOptions {
  /** 当前参与绘制的点位（前 5 个），由调用方按收藏偏好过滤后给出 */
  getTopPoints: () => { point_id: string }[]
  /** 校准视口高度（顶部安全横幅高度取决于挂载后的 DOM，挂载时与 resize 时都要重算） */
  refreshViewport: () => void
}

/**
 * 实时数据页的趋势图（ECharts 实例 + 100ms 节流重绘 + 历史缓冲）。
 *
 * BUG-K 修复要点保留在此：原实现依赖 ECharts `appendData` 增量刷新，但 series 用的是
 * 内联 `data` 数组，`appendData` 必抛 TypeError 且被降级成 warn → 趋势图永久空白。
 * 现在每个节流窗口用 `setOption` 从 historyMap 的最终态全量重绘（5 series × 60 点，
 * diff 成本可忽略），并用 `replaceMerge: ['series']` 保证 top5 集合变化时旧 series 被移除。
 */
export function useRealtimeChart(options: ChartOptions) {
  const chartRef = ref<HTMLElement>()
  const chartError = ref<string | null>(null)

  let chart: echarts.ECharts | null = null
  let resizeHandler: (() => void) | null = null
  let flushTimer: ReturnType<typeof setTimeout> | null = null
  let disposed = false

  // 历史缓冲：普通 Map（不响应式）。只被 chartTimeSpan / updateChartStructure 读取，
  // 重算由 markHistoryDirty 的显式信号驱动，放在响应式系统外反而更省。
  const historyMap = new Map<string, { time: string; value: number }[]>()
  // historyMap 非响应式，依赖它的 chartTimeSpan 需要显式重算信号。
  // 每批推送自增一次，语义等价于原先 reactive(Map) 的迭代追踪。
  const historyVersion = ref(0)

  /**
   * 记录一条历史点（不触发重算）。
   *
   * 「写入」与「标记变更」刻意分开：冻结（暂停刷新）期间历史仍要续写，
   * 但图表与跨度标签必须停住——由调用方决定是否调 `markHistoryDirty()`。
   */
  function appendHistory(dp: DataPoint) {
    const hist = historyMap.get(dp.point_id) || []
    hist.push({ time: dp.timestamp.toString(), value: dp.value })
    if (hist.length > HISTORY_CAP) hist.shift()
    historyMap.set(dp.point_id, hist)
  }

  /** 通知 chartTimeSpan 重算（historyMap 非响应式，需显式信号） */
  function markHistoryDirty() {
    historyVersion.value++
  }

  /**
   * 图表覆盖的时间跨度（标签文案）。
   *
   * 只统计**图表实际绘制的 top5 点位**：用全部 1200 点位的历史算跨度既没意义
   * （标签描述的是图表），又让每批推送都要遍历 1200 × 60 = 7.2 万条历史。
   */
  const chartTimeSpan = computed(() => {
    // 读 historyVersion 建立依赖：historyMap 已非响应式，靠它触发重算
    void historyVersion.value
    const points = options.getTopPoints()
    if (points.length === 0) return '等待数据'
    let min = Infinity
    let max = -Infinity
    for (const p of points) {
      const hist = historyMap.get(p.point_id)
      if (!hist) continue
      for (const h of hist) {
        const t = Number(h.time)
        if (Number.isFinite(t) && t > 0) {
          if (t < min) min = t
          if (t > max) max = t
        }
      }
    }
    if (!Number.isFinite(min) || !Number.isFinite(max) || max <= min) return '等待数据'
    const span = Math.round(max - min)
    if (span < 60) return `最近 ${span} 秒`
    if (span < 3600) return `最近 ${Math.round(span / 60)} 分钟`
    return `最近 ${Math.round(span / 3600)} 小时`
  })

  function mountChart() {
    if (!chartRef.value) return
    // setup 阶段 DOM 还不存在，读不到顶部横幅；这里挂载完成后校准一次视口高度
    options.refreshViewport()
    chart = initChart(chartRef.value)
    chart.setOption({
      tooltip: tooltipStyle,
      legend: { ...legendStyle, type: 'scroll' },
      grid: defaultGrid,
      xAxis: {
        ...categoryAxisStyle,
        data: [],
        boundaryGap: false,
      },
      yAxis: {
        ...valueAxisStyle,
        // 不强制从 0 起：小幅波动被压平后看不出趋势，这是最常见的"看不清"
        scale: true,
        splitNumber: 4, // 网格线少一半，曲线本身更突出
        // 轴色由 gw-light/gw-dark 主题提供（canvas 不解析 var()，写死字面量会覆盖暗色主题）
        axisLabel: { formatter: (v: number) => compactAxis(v) },
      },
      series: [],
    })
    resizeHandler = () => {
      // N8: 窗口尺寸变化时先更新视口高度（驱动表格/图表重算），再让图表按新尺寸重绘
      options.refreshViewport()
      chart?.resize()
    }
    window.addEventListener('resize', resizeHandler)
  }

  /** 按容器新尺寸重绘（父组件在 chartHeight 变化后于 nextTick 调用） */
  function resizeChart() {
    chart?.resize()
  }

  /** 重建图表（series / xAxis.data / legend），数据源为 historyMap 的最终态 */
  function updateChartStructure() {
    if (!chart) return
    const points = options.getTopPoints()
    const timeSet = new Set<string>()
    for (const p of points) {
      const hist = historyMap.get(p.point_id) || []
      for (const h of hist) timeSet.add(h.time)
    }
    const times = Array.from(timeSet).sort()
    const xData = times.slice(-HISTORY_CAP)
    const xLabels = xData.map((t) => {
      const ts = Number(t)
      if (!Number.isFinite(ts) || ts <= 0) return t
      const ms = ts < 1e12 ? ts * 1000 : ts
      const d = new Date(ms)
      return isNaN(d.getTime()) ? t : formatTimeOfDay(d)
    })
    const series = points.slice(0, TOP_N).map((p, idx) => {
      const hist = historyMap.get(p.point_id) || []
      const valueMap = new Map(hist.map((h) => [h.time, h.value]))
      const color = COLORS[idx % COLORS.length]!
      const area = AREA_COLORS[idx % AREA_COLORS.length]!
      return {
        name: p.point_id,
        type: 'line' as const,
        showSymbol: false,
        smooth: true,
        // 断点不连线：缺失区间应该"断开"而不是用直线糊过去，否则会被读成真实值
        connectNulls: false,
        emphasis: { focus: 'series' as const },
        lineStyle: { width: 2, color },
        itemStyle: { color },
        areaStyle: {
          color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
            { offset: 0, color: area[0]! },
            { offset: 1, color: area[1]! },
          ]),
        },
        data: xData.map((t) => valueMap.get(t) ?? null),
      }
    })
    try {
      chart.setOption({
        legend: { data: points.slice(0, TOP_N).map((p) => p.point_id) },
        xAxis: { data: xLabels },
        series,
      }, { replaceMerge: ['series'] })
      chartError.value = null
    } catch (e) {
      // 渲染失败必须可见：否则画布永久空白，用户会误判成「采集没有数据」
      chartError.value = e instanceof Error ? e.message : String(e)
      logError('realtime:chart', e, '图表渲染失败')
    }
  }

  /** 100ms 节流重绘：一批消息合并成一次 setOption */
  function scheduleFlush() {
    if (flushTimer) return
    flushTimer = setTimeout(() => {
      flushTimer = null
      if (disposed) return
      updateChartStructure()
    }, APPEND_THROTTLE_MS)
  }

  function dispose() {
    disposed = true
    if (resizeHandler) {
      window.removeEventListener('resize', resizeHandler)
      resizeHandler = null
    }
    if (flushTimer) {
      clearTimeout(flushTimer)
      flushTimer = null
    }
    // disposeChart 同步从 liveCharts 注册表摘除，防止已销毁实例被主题刷新持有（长驻内存）
    disposeChart(chart)
    chart = null
    historyMap.clear()
  }

  return {
    chartRef,
    chartError,
    chartTimeSpan,
    mountChart,
    resizeChart,
    appendHistory,
    markHistoryDirty,
    scheduleFlush,
    dispose,
  }
}
