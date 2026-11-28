/**
 * 图表通用样式常量
 *
 * 从 echarts.ts 分离出来，避免仅需要样式的模块（如非图表页面）
 * 因 import tooltipStyle 而将整个 echarts core + charts + components 拉入打包。
 *
 * 分层约定（重要）：
 *  - **本文件只保留布局常量与 DOM 渲染样式**（tooltip 弹层是 DOM，可用 CSS 变量跟随主题）；
 *  - **canvas 绘制的颜色一律不在这里写**——canvas 不解析 var()（zrender 判非法后
 *    被 canvas 2D 静默忽略，实际渲染成"上一笔画笔颜色"），历史版本在这里用
 *    var(--text-secondary) 写轴色，浅色下侥幸可读、暗色下反而覆盖 gw-dark 主题
 *    的有效色值。现在轴/图例/轴名/指示线颜色统一由主题接管：
 *    浅色 → `gw-light`（chartTheme.CHART_LIGHT 钉死设计意图值），
 *    暗色 → `gw-dark`（chartTheme.CHART_DARK）。initChart 自动注入。
 *  - canvas 用色的单一来源在 chartTheme.ts。
 */
import { CHART_PRIMARY } from './chartTheme'

export const tooltipStyle = {
  trigger: 'axis' as const,
  // tooltip 弹层本体是 DOM 渲染：直接用 EP 背景变量，暗色模式自动切换深色底
  backgroundColor: 'var(--el-bg-color)',
  borderColor: 'var(--border-light)',
  borderWidth: 1,
  borderRadius: 8,
  padding: [10, 14],
  textStyle: { color: 'var(--text-primary)', fontSize: 13 },
  extraCssText: 'box-shadow: 0 4px 16px rgba(0, 0, 0, 0.12);',
  axisPointer: {
    // 指示虚线是 canvas 绘制：不能写 var()，用主蓝字面量（明暗通用）
    lineStyle: { color: CHART_PRIMARY, type: 'dashed' },
  },
}

/**
 * 坐标轴/网格/图例的统一样式（全站图表视觉一致的单一来源）。
 *
 * 此前各页 setOption 里手写 grid 边距与轴色，13 处 grid 各不相同。
 * 颜色部分已全部移交 gw-light / gw-dark 两层主题（原因见文件头注），
 * 这里只保留布局与行为配置，新页面直接展开使用。
 */

/** 网格默认边距；top 需为图例留位，bottom 需容纳类目轴标签 */
export const defaultGrid = { left: 50, right: 20, top: 30, bottom: 32 }

/** 数值轴：虚线分隔线不抢数据线的视觉权重（颜色由主题提供） */
export const valueAxisStyle = {
  type: 'value' as const,
  splitLine: { lineStyle: { type: 'dashed' as const } },
}

/** 类目轴：标签重叠时自动隐藏，避免文字叠字糊成一片（颜色由主题提供） */
export const categoryAxisStyle = {
  type: 'category' as const,
  axisLabel: { hideOverlap: true },
}

/** 图例：靠右上角不压数据（文字颜色由主题提供） */
export const legendStyle = {
  top: 0,
  right: 0,
  itemWidth: 12,
  itemHeight: 8,
  textStyle: { fontSize: 12 },
}

/** 轴名（单位标注）：小号，避免与轴标签抢视觉（颜色由主题提供） */
export const axisNameStyle = { fontSize: 11 }
