/**
 * ECharts 主题注册（暗色 `gw-dark` / 浅色 `gw-light`）。
 *
 * 从原 `utils/echarts.ts` 拆出：主题色板定义体量大，且与实例生命周期管理
 * （注册表 / 建图 / 尺寸自适应）职责无关，独立成模块便于单测直接定位。
 *
 * 色值单一来源在 `chartTheme.ts`；canvas 不解析 var() 的原因见该文件头注。
 */
import { echarts } from './echartsSetup'
import { CHART_DARK, CHART_LIGHT, CHART_PRIMARY, CHART_ZOOM_FILLER } from './chartTheme'

/**
 * 当前是否处于暗色模式（读 <html> 上的 .dark class）。
 *
 * Element Plus 的 dark css-vars 与自研 .dark 变量块都挂在该 class 下，
 * 与 `stores/theme.ts` 的 `applyDarkClass` 是同一个开关。
 */
export function isDarkMode(): boolean {
  if (typeof document === 'undefined') return false
  return document.documentElement.classList.contains('dark')
}

/* ========== 暗色主题跟随（修复 #1 / B-1-ROI #1） ==========
 * 历史问题：旧 `initChart` 只在调用瞬间读一次 `isDarkMode()`，渲染后再切主题
 * 不会刷新已有图表——客户切到暗色后看到"半残"图表（背景已黑、坐标文字仍
 * 是亮色），被截图发工单误以为是渲染 bug。
 *
 * 修复策略：
 *   1. 全局注册表 `liveCharts` 追踪所有通过 `initChart` 创建的活动图表。
 *   2. 提供 `applyEchartsTheme()`：切主题时重新挂载 `gw-dark` ↔ `gw-light`
 *      并 `setOption(prev, true)` 重绘；按实例记录已应用主题（WeakMap），
 *      状态未变的实例跳过，避免重复全量重绘。
 *   3. `stores/theme.ts` 在 `setDark()` 末尾调用 `applyEchartsTheme()`，
 *      让主题切换对所有图表即时生效。SSR / 测试环境自动降级为 no-op。
 *   4. 提供 `unregisterChart()` 给页面 onUnmounted，避免被泄漏的图表实例
 *      持续占用 setOption 调用。
 */
echarts.registerTheme('gw-dark', {
  backgroundColor: 'transparent',
  // tooltip：默认 tooltip 用 canvas 自适应白色背景，暗色模式下视觉上像一块
  // 刺眼的白块盖在深色图表上——强制暗色背景 + 浅灰文字 + 浅描边，和
  // Element Plus dark 主题的弹层色保持一致（与 chartStyle.ts 里的
  // tooltipStyle 选值同源，色值单一来源见 chartTheme.ts）。
  tooltip: {
    backgroundColor: CHART_DARK.tooltipBg,
    borderColor: CHART_DARK.tooltipBorder,
    borderWidth: 1,
    textStyle: { color: CHART_DARK.tooltipText },
  },
  // title：标题与副标题颜色，默认用 echarts 内置的 '#333' 在暗色下几乎看不见，
  // 统一用深背景下可读的浅色。
  title: {
    textStyle: { color: CHART_DARK.tooltipText },
    subtextStyle: { color: CHART_DARK.subText },
  },
  categoryAxis: {
    axisLine: { lineStyle: { color: CHART_DARK.axisLine } },
    axisTick: { lineStyle: { color: CHART_DARK.axisLine } },
    axisLabel: { color: CHART_DARK.subText },
    // 轴名（单位标注）：页面级 chartStyle 已不再写死轴色（canvas 不解析 var()），
    // 暗色下由主题接管
    nameTextStyle: { color: CHART_DARK.subText },
    // 分类轴也画一层浅 splitLine，让"密集分类"场景下每列边界清晰
    splitLine: { show: true, lineStyle: { color: CHART_DARK.splitLine, type: 'dashed' } },
  },
  valueAxis: {
    axisLine: { show: false },
    axisLabel: { color: CHART_DARK.subText },
    nameTextStyle: { color: CHART_DARK.subText },
    splitLine: { lineStyle: { color: CHART_DARK.splitLine } },
  },
  legend: { textStyle: { color: CHART_DARK.subText } },
  // axisPointer 指示线：页面级写 var(--color-primary) 会被 canvas 静默忽略，
  // 由主题统一给主蓝虚线
  axisPointer: {
    lineStyle: { color: CHART_PRIMARY, type: 'dashed' },
  },
  // dataZoom：区域缩放滑块。默认背景色是浅色条纹，暗色模式下整块发白。
  // 这里把 dataZoom 的滑块区、选中区、手柄、文字统一成深底色 + 浅灰文字。
  dataZoom: [
    {
      type: 'inside',
      backgroundColor: CHART_DARK.zoomBg,
      fillerColor: CHART_ZOOM_FILLER,
      borderColor: CHART_DARK.axisLine,
      handleStyle: { color: CHART_PRIMARY, borderColor: CHART_PRIMARY },
      textStyle: { color: CHART_DARK.subText },
    },
    {
      type: 'slider',
      backgroundColor: CHART_DARK.zoomBg,
      fillerColor: CHART_ZOOM_FILLER,
      borderColor: CHART_DARK.axisLine,
      handleStyle: { color: CHART_PRIMARY, borderColor: CHART_PRIMARY },
      textStyle: { color: CHART_DARK.subText },
    },
  ],
  // grid 的轴分隔线由 categoryAxis / valueAxis 的 splitLine 统一控制，
  // grid 本身不需要额外设置——但把 zlevel 默认抬高避免被 canvas 背景色盖住。
})

/* ========== 浅色主题 gw-light ==========
 * 页面级 chartStyle 此前用 var(--token) 写轴色——canvas 不解析 var()（zrender
 * 判非法后由 canvas 2D 静默忽略），浅色下靠"上一笔画笔颜色"侥幸可读，暗色下
 * 还会覆盖 gw-dark 的有效色值。方案：页面级不再写死轴色，浅色由 gw-light 钉死
 * 设计意图值（与 global.css 令牌对齐，见 chartTheme.CHART_LIGHT），暗色由
 * gw-dark 接管——两层主题都不再被页面级无效值覆盖。
 */
echarts.registerTheme('gw-light', {
  backgroundColor: 'transparent',
  categoryAxis: {
    axisLine: { lineStyle: { color: CHART_LIGHT.axisLine } },
    axisTick: { lineStyle: { color: CHART_LIGHT.axisLine } },
    axisLabel: { color: CHART_LIGHT.axisText },
    nameTextStyle: { color: CHART_LIGHT.axisText },
    splitLine: { show: true, lineStyle: { color: CHART_LIGHT.splitLine, type: 'dashed' } },
  },
  valueAxis: {
    axisLine: { show: false },
    axisLabel: { color: CHART_LIGHT.axisText },
    nameTextStyle: { color: CHART_LIGHT.axisText },
    splitLine: { lineStyle: { color: CHART_LIGHT.splitLine } },
  },
  legend: { textStyle: { color: CHART_LIGHT.legendText } },
  axisPointer: {
    lineStyle: { color: CHART_PRIMARY, type: 'dashed' },
  },
})
