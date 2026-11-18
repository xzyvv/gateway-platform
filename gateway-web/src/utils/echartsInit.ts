/**
 * 图表建图入口：`initChart` / `ensureChart` 与两项无障碍处理。
 *
 * 从原 `utils/echarts.ts` 拆出：本模块只管「怎么把一个 DOM 变成可用的图表
 * 实例」，与实例注册表（echartsRegistry）和主题色板（echartsThemes）职责分离。
 */
import { echarts } from './echartsSetup'
import type { ECharts, EChartsCoreOption } from 'echarts/core'
import { isDarkMode } from './echartsThemes'
import { attachChartResizeObserver, disposeChart, registerChart } from './echartsRegistry'

const origInit = echarts.init.bind(echarts)

/**
 * 图表容器的无障碍兜底。
 *
 * canvas 里的线条/柱子对读屏器完全不可见，容器本身也没有语义，读屏器会直接跳过
 * 整个图表。这里统一补 `role="img"` + `aria-label`：
 *   - 调用方可给容器加 `data-chart-label="点位 1 近 1 小时趋势"` 提供具体描述；
 *   - 没给时退化为通用说明，至少让用户知道"这里有一个图表"。
 *
 * 没有直接开 ECharts 的 `aria` 组件：它会把整份 option 的数据生成成朗读文本，
 * 对 500 点位的实时曲线会产出过长且频繁变化的 aria 内容，反而干扰读屏，
 * 且会额外增加打包体积。
 */
function applyChartA11y(dom: unknown): void {
  const el = dom as HTMLElement | null
  if (!el || typeof el.setAttribute !== 'function') return
  el.setAttribute('role', 'img')
  if (!el.getAttribute('aria-label')) {
    el.setAttribute('aria-label', el.dataset.chartLabel || '数据图表，具体数值请查看同页表格或悬停提示')
  }
}

/**
 * 尊重系统「减少动态效果」偏好（无障碍，P2 #4a）。
 *
 * CSS 侧的 `prefers-reduced-motion` 兜底（global.css）只对 DOM 动画/过渡生效；
 * ECharts 的入场动画、逐点动画是 canvas 内的 JS 驱动渲染，CSS 完全管不到——
 * 前庭障碍用户在开启「减少动态效果」后，图表仍会播放折线生长/柱状增长动画。
 *
 * 处理方式：initChart 时检测媒体查询，命中则把 ECharts 动画时长强制为 0
 * （`animationDuration: 0` + `animationDurationUpdate: 0`），并关闭逐点动画。
 * 图表仍然完整渲染出最终形态，只是不播放过程动画。
 *
 * 媒体查询用 `matchMedia` 而非一次性读取：用户可以在系统设置里随时切换，
 * 切换后进入的页面即刻生效；已打开的页面不强刷（避免打断用户操作）。
 */
const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)'

function prefersReducedMotion(): boolean {
  if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return false
  try {
    return window.matchMedia(REDUCED_MOTION_QUERY).matches
  } catch {
    return false
  }
}

/**
 * 减少动效时给 option 打上「无动画」补丁。
 *
 * 不能直接改调用方传入的 option 对象——那会让调用方的状态被隐式污染
 * （例如同一份 option 用于导出图片时动画也一并消失）。这里返回浅拷贝，
 * 只覆盖动画相关字段，其余配置原样保留。
 */
export function applyReducedMotion(option: Record<string, unknown>): Record<string, unknown> {
  if (!prefersReducedMotion()) return option
  return {
    ...option,
    animation: false,
    animationDuration: 0,
    animationDurationUpdate: 0,
    animationEasing: 'linear',
    animationEasingUpdate: 'linear',
    // 关闭逐点动画（折线/柱状的依次生长）
    animationDelay: 0,
    animationDelayUpdate: 0,
  }
}

/**
 * 包装实例的 setOption：在「减少动态效果」偏好命中时，把 option 中的动画
 * 配置覆盖为无动画。
 *
 * 只在偏好命中时改写 option 内容（不改调用方传入的对象，避免污染其状态）。
 * 偏好未命中时直接透传，零额外开销。
 */
function patchSetOptionForReducedMotion(chart: ECharts): void {
  if (!prefersReducedMotion()) return
  const original = chart.setOption.bind(chart)
  chart.setOption = ((option: EChartsCoreOption, notMerge?: boolean, lazyUpdate?: boolean) => {
    if (option && typeof option === 'object') {
      return original(applyReducedMotion(option as Record<string, unknown>) as EChartsCoreOption, notMerge, lazyUpdate)
    }
    return original(option, notMerge, lazyUpdate)
  }) as typeof chart.setOption
}

/**
 * 图表初始化统一出口：暗色模式自动注入 gw-dark 主题，页面用 initChart 替代 echarts.init。
 *
 * 额外职责：包装 `setOption`，在「减少动态效果」偏好下自动剥掉动画配置。
 * 包装而非要求每个页面自己调 `applyReducedMotion`——那样 8+ 个图表页每处都
 * 可能漏掉，漏一处就等于该图表仍会播放动画，无障碍改造形同虚设。
 */
export function initChart(
  dom: Parameters<typeof origInit>[0],
  theme?: Parameters<typeof origInit>[1],
  opts?: Parameters<typeof origInit>[2],
): ECharts {
  const chart = origInit(dom, theme ?? (isDarkMode() ? 'gw-dark' : 'gw-light'), opts)
  applyChartA11y(dom)
  // 自动追踪到全局注册表，主题切换时统一刷新
  registerChart(chart)
  // 容器尺寸变化（侧栏折叠 / 窗口缩放 / 平板旋转）时自动 resize
  if (dom instanceof HTMLElement) attachChartResizeObserver(chart, dom)
  patchSetOptionForReducedMotion(chart)
  return chart
}

/**
 * 保证图表实例与当前宿主元素匹配，必要时重建。
 *
 * 历史问题（DataCompare / DiskHealthChart）：图表宿主被 `v-if` 包着，
 * 条件不满足时整块 DOM 被卸载。此时页面侧的 `chart` 变量仍持有绑定在
 * **已移除 DOM** 上的旧实例，且旧实例还挂在 `liveCharts` 注册表里没释放。
 * 条件再次满足时 `v-if` 会创建**全新**的宿主元素，但调用方按
 * `if (!chart)` 判断——旧实例非空 → 跳过 initChart → `setOption` 写向
 * 已销毁的 DOM → 新宿主里永久空白，同时旧实例持续泄漏。
 *
 * 修法：用 `getDom()` 比对实例绑定的宿主与当前宿主；不一致（或实例已
 * dispose / 宿主为空）就 dispose 旧的并对新宿主重新 initChart。
 *
 * 宿主为空（宿主被 v-if 卸载）时返回 null 并顺手释放旧实例——调用方
 * 通常写成 `chart = ensureChart(chart, chartEl.value)`，即可自动完成
 * 「卸载时释放、条件恢复时重建」两件事。
 *
 * 幂等：宿主未变时原样返回同一实例，不重复 init。
 */
export function ensureChart(
  current: ECharts | null | undefined,
  dom: HTMLElement | undefined | null,
): ECharts | null {
  // 宿主已消失（v-if 卸载）：释放旧实例并置空，等宿主回来时重建
  if (!dom) {
    if (current) disposeChart(current)
    return null
  }
  // 旧实例已销毁，或绑定的宿主与当前宿主不是同一个元素 → 重建
  const stale =
    !current ||
    typeof current.isDisposed === 'function' && current.isDisposed() ||
    current.getDom() !== dom
  if (stale) {
    if (current) disposeChart(current)
    return initChart(dom)
  }
  return current
}
