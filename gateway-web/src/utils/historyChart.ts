/**
 * 历史趋势图 ECharts option 构建
 *
 * 从 useHistoryQuery.renderChart 抽出的纯函数：把多序列历史数据转成折线图 option，
 * 含调色板、单序列渐变面积、echarts 内置 LTTB 采样与前端预降采样。
 * 不持有图表实例，由调用方负责 init/setOption 与 resize 生命周期。
 */
import echarts from './echarts'
import { tooltipStyle, valueAxisStyle, categoryAxisStyle } from './chartStyle'
import { CHART_PALETTE, CHART_AREA_GRADIENT, CHART_ZOOM_FILLER_SOFT, CHART_PRIMARY, CHART_MUTED_TEXT } from './chartTheme'
import { downsampleIfNeeded } from './downsample'

export interface HistoryChartSeries {
  pointId: string
  data: { timestamp: number; value: number }[]
}

/**
 * 构建历史趋势图 option
 *
 * @param multiHistory 多序列数据（每条序列含 pointId 与数据点）
 * @param xLabels 与首条序列对齐的 X 轴标签（由调用方按 formatTime 生成）
 */
export function buildHistoryChartOption(multiHistory: HistoryChartSeries[], xLabels: string[]) {
  // 色板单一来源：与 gw-dark 主题、其他图表页共用 chartTheme（canvas 不解析 CSS var，
  // 图表色必须字面量，见 chartTheme.ts 头注）
  const palette: readonly string[] = CHART_PALETTE
  const series = multiHistory.map((m, idx) => {
    const color = palette[idx % palette.length]
    const isSingle = multiHistory.length === 1
    return {
      name: m.pointId,
      type: 'line',
      showSymbol: false,
      smooth: true,
      symbol: 'circle',
      symbolSize: 6,
      lineStyle: { width: 2.5, color },
      itemStyle: { color, borderColor: '#fff', borderWidth: 2 },
      // 大数据集优化：echarts 内置 LTTB 降采样
      // - 数据点 > 5000 时自动按可视范围降采样，保留视觉特征同时减少渲染开销
      // - 仅对 cartesian 坐标系（line/bar）生效，配合 dataZoom 缩放时动态重采样
      sampling: 'lttb',
      areaStyle: isSingle ? {
        color: new echarts.graphic.LinearGradient(0, 0, 0, 1, [
          { offset: 0, color: CHART_AREA_GRADIENT.from },
          { offset: 1, color: CHART_AREA_GRADIENT.to },
        ]),
      } : undefined,
      // 前端预降采样：单序列 > 10000 点时先做 LTTB 到 5000 点，避免 echarts 一次性处理过载
      data: downsampleIfNeeded(m.data, 10000, 5000).map((h) => h.value),
    }
  })
  const isMulti = multiHistory.length > 1
  return {
    tooltip: tooltipStyle,
    legend: isMulti ? {
      data: multiHistory.map((m) => m.pointId),
      top: 0,
      // 图例文字是 canvas 绘制：var(--text-regular) 会被 zrender 静默忽略，
      // 改用明暗两用中性灰（见 chartTheme.CHART_MUTED_TEXT）
      textStyle: { color: CHART_MUTED_TEXT, fontSize: 12 },
    } : undefined,
    grid: { left: 50, right: 20, top: isMulti ? 36 : 20, bottom: 60 },
    xAxis: { ...categoryAxisStyle, data: xLabels },
    yAxis: { ...valueAxisStyle },
    dataZoom: [
      { type: 'inside' },
      {
        type: 'slider',
        height: 20,
        borderColor: 'transparent',
        // 滑块整体是 canvas 绘制：以下 var() 此前全部被 zrender 静默忽略，
        // 统一改为 chartTheme 合法色值（背景用透明色同时适配明暗两种图表底色）
        backgroundColor: 'transparent',
        fillerColor: CHART_ZOOM_FILLER_SOFT,
        handleStyle: { color: CHART_PRIMARY, borderColor: CHART_PRIMARY },
        moveHandleStyle: { color: CHART_PRIMARY },
        textStyle: { color: CHART_MUTED_TEXT },
      },
    ],
    series,
  }
}