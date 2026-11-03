/**
 * ECharts 主题色板 —— canvas 绘制用色的单一来源
 *
 * 为什么必须是字面量而不是 var(--token)：
 * ECharts 的 canvas 绘制（lineStyle / itemStyle / handleStyle / fillerColor 等）
 * 不经过 CSS 解析，`var(--x)` 会被 zrender 判为非法色并静默回退，导致配色脱节。
 * 只有 DOM 渲染的部分（tooltip 弹层、aria 文本）可用 CSS 变量——那部分统一在
 * chartStyle.ts 里用 var() 定义。
 *
 * 与 global.css 令牌的对应关系见各行注释；换品牌色时本文件与 global.css 同步修改。
 * （chartStyle.ts 的 tooltip/轴样式注释里"与 gw-dark 选值同源"即指向本文件。）
 */

/** 主色，对应 --color-primary。比界面主色 #4f46e5 亮一档（indigo-500）：
 *  canvas 无法跟随主题，取明暗两种底色下都可读的折中值 */
export const CHART_PRIMARY = '#6366f1'

/** 功能色，对应 --color-success / --color-warning / --color-danger / --color-info */
export const CHART_SUCCESS = '#67c23a'
export const CHART_WARNING = '#e6a23c'
export const CHART_DANGER = '#f56c6c'

/** 拓扑图连线灰（浅色主题场景），对应 --el-border-color 系的 #c0c4cc */
export const CHART_LINE = '#c0c4cc'

/**
 * 多系列分类色板：首色为主色，后续按区分度排列。
 * 使用方循环取色（idx % length），顺序即视觉优先级。
 */
export const CHART_PALETTE = [
  '#6366f1', // 主靛蓝 --color-primary
  '#36cfc9', // 青
  '#e6a23c', // 琥珀   --el-color-warning
  '#f56c6c', // 红     --el-color-danger
  '#67c23a', // 绿     --el-color-success
  '#9254de', // 紫
  '#13c2c2', // 深青
  '#722ed1', // 深紫
] as const

/**
 * 主色透明叠加（canvas 不支持 color-mix，直接给 rgba）：
 * 单序列面积图渐变，从主色 30% 渐隐到 2%。
 */
export const CHART_AREA_GRADIENT = {
  from: 'rgba(99, 102, 241, 0.30)',
  to: 'rgba(99, 102, 241, 0.02)',
} as const

/** dataZoom 选中区填充（gw-dark 主题用 20% 透明度） */
export const CHART_ZOOM_FILLER = 'rgba(99, 102, 241, 0.20)'

/** dataZoom 选中区填充（浅色页面内嵌滑块用更轻的 15%，避免压住下方数据） */
export const CHART_ZOOM_FILLER_SOFT = 'rgba(99, 102, 241, 0.15)'

/**
 * 图表内中性灰文字（dataZoom 刻度、图例等 canvas 文字）。
 * 对应 --el-color-info：浅色/暗色背景下都有基本可读性——canvas 无法用
 * var() 跟随主题，只能选"两边都不难看"的固定值。
 */
export const CHART_MUTED_TEXT = '#909399'

/**
 * 示波器/波形视图的「暗底」配色。
 *
 * 与其他图表不同，示波器刻意不跟随明暗主题：黑底 + 高饱和扫描线是仪表的
 * 物理观感，换浅底会削弱可读性。这里集中声明，避免在页面里散落色值。
 */
export const CHART_SCOPE = {
  background: '#000',
  tooltipBg: 'rgba(0, 0, 0, 0.8)',
  text: '#fff',
  axis: '#444',
  split: '#222',
  label: '#888',
}

/**
 * 浅色图表底座（gw-light 主题专用）：把全站图表的浅色轴/图例值显式钉死，
 * 防止 echarts 内置默认值跨版本漂移。取值与 global.css 浅色令牌逐行对齐，
 * 换品牌色时本文件与 global.css 同步修改。
 */
export const CHART_LIGHT = {
  /** 轴标签/轴名文字（--text-secondary） */
  axisText: '#6b7280',
  /** 图例文字（--text-regular） */
  legendText: '#606266',
  /** 类目轴线（--border-light） */
  axisLine: '#e4e7ed',
  /** 数值轴分隔线（--border-extralight，刻意极淡：不抢数据线视觉权重） */
  splitLine: '#f0f2f5',
} as const

/**
 * 暗色图表底座（gw-dark 主题专用）：tooltip 弹层、轴、图例、dataZoom 的固定色。
 * 与 chartStyle.ts 的 tooltipStyle、Element Plus dark 弹层色同源，
 * 避免"图表 canvas 用一套色、弹层 DOM 用另一套色"的割裂。
 */
export const CHART_DARK = {
  /** tooltip 背景（--code-bg 同系深底） */
  tooltipBg: '#1e1e2f',
  /** tooltip 描边 */
  tooltipBorder: '#4c4d5f',
  /** tooltip 主文字 */
  tooltipText: '#e4e7ed',
  /** 副标题 / 轴标签 / 图例文字 */
  subText: '#a3a6ad',
  /** 轴线与轴刻度 */
  axisLine: '#4c4d5f',
  /** 轴分隔线（比轴线更浅一档） */
  splitLine: '#2a2a3b',
  /** dataZoom 背板 */
  zoomBg: '#141414',
} as const
