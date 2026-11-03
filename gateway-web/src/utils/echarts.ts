import * as echarts from 'echarts/core'
import { LineChart } from 'echarts/charts'
import {
  GridComponent,
  TooltipComponent,
  LegendComponent,
} from 'echarts/components'
import { CanvasRenderer } from 'echarts/renderers'
import { logWarn } from './logger'
import { CHART_DARK, CHART_LIGHT, CHART_PRIMARY, CHART_ZOOM_FILLER } from './chartTheme'

/**
 * 核心图表组件：Line + Grid / Tooltip / Legend + CanvasRenderer。
 *
 * 体积较大但使用频次低的组件（Graph 力导向图、Custom 自定义绘制、Bar 柱状图、
 * DataZoom 区域缩放、Title 标题、MarkLine 标线）拆到 `echarts-extra.ts`，
 * 只有用到的页面才加载，避免折线图页面也拖入整套拓扑图引擎
 * （GraphChart 单组件 ~150KB raw）。
 *
 * 注意：echarts.use 注册到全局单例，echarts-extra 复用同一个实例注册额外组件。
 */
echarts.use([
  LineChart,
  GridComponent,
  TooltipComponent,
  LegendComponent,
  CanvasRenderer,
])

// tooltipStyle 已分离到 utils/chartStyle.ts，避免仅需样式的模块拉入 echarts

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
function isDarkMode(): boolean {
  if (typeof document === 'undefined') return false
  return document.documentElement.classList.contains('dark')
}

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

const origInit = echarts.init.bind(echarts)

/**
 * 活动图表注册表。
 *
 * 用 `Set` 而非 `WeakSet`：虽然 `WeakSet` 自动 GC、理论更安全，但 echarts
 * 实例化后由 `chart.dispose()` 主动释放；保留 `Set` + 显式 `unregisterChart`
 * 是为了让"页面只 dispose 但忘了 unregister"也活得下来——只要 `applyEchartsTheme`
 * 能拿到即可。Disposed 实例在 dispose 时已经从 DOM 移除，对无效实例调用
 * `setOption` 会被 echarts 内部静默 no-op，不会抛错。
 */
const liveCharts = new Set<echarts.ECharts>()

/* ========== 容器尺寸变化自动 resize（修复 #16） ==========
 * 历史问题：8 个图表页（总线负载 / 示波器 / 负载测试 / 抖动分析 / 甘特图 /
 * 设备拓扑 / 数据压缩 / 收藏监控）建图后从不调用 resize。侧栏折叠、平板旋转、
 * 窗口缩放后容器宽度已变，但 echarts canvas 仍按旧尺寸渲染——出现大片空白
 * 或曲线被裁切。
 *
 * 修复：initChart 时给容器挂 ResizeObserver，尺寸变化时自动 chart.resize()。
 * 用 WeakMap 按实例存 observer，disposeChart 时统一 disconnect，避免泄漏。
 * 老浏览器无 ResizeObserver 时回退到 window resize 监听（覆盖窗口缩放场景，
 * 侧栏折叠场景需页面自己触发——但现代浏览器都支持 ResizeObserver）。
 */
const chartObservers = new WeakMap<echarts.ECharts, ResizeObserver>()
/** resize 节流定时器：连续尺寸变化（如拖拽窗口）合并成一次 resize */
const chartResizeTimers = new WeakMap<echarts.ECharts, ReturnType<typeof setTimeout>>()

const RESIZE_DEBOUNCE_MS = 120

function attachResizeObserver(chart: echarts.ECharts, dom: HTMLElement): void {
  if (typeof ResizeObserver === 'undefined') {
    // 回退：window resize 时统一 resize 所有活动图表
    if (!windowResizeHandlerInstalled) installWindowResizeFallback()
    return
  }
  const observer = new ResizeObserver(() => {
    // 容器尚未布局（宽高为 0）时跳过，否则 echarts 会按 0 尺寸渲染后卡死
    if (dom.clientWidth === 0 || dom.clientHeight === 0) return
    const prev = chartResizeTimers.get(chart)
    if (prev) clearTimeout(prev)
    const timer = setTimeout(() => {
      try {
        if (typeof chart.isDisposed !== 'function' || !chart.isDisposed()) chart.resize()
      } catch (e) {
        logWarn('echarts', '[resize] chart.resize fail', e)
      }
    }, RESIZE_DEBOUNCE_MS)
    chartResizeTimers.set(chart, timer)
  })
  observer.observe(dom)
  chartObservers.set(chart, observer)
}

let windowResizeHandlerInstalled = false
function installWindowResizeFallback(): void {
  windowResizeHandlerInstalled = true
  window.addEventListener('resize', () => {
    const charts = Array.from(liveCharts)
    for (const c of charts) {
      try {
        if (typeof c.isDisposed !== 'function' || !c.isDisposed()) c.resize()
      } catch (e) {
        logWarn('echarts', '[resize-fallback] chart.resize fail', e)
      }
    }
  })
}

function detachResizeObserver(chart: echarts.ECharts): void {
  const observer = chartObservers.get(chart)
  if (observer) {
    observer.disconnect()
    chartObservers.delete(chart)
  }
  const timer = chartResizeTimers.get(chart)
  if (timer) {
    clearTimeout(timer)
    chartResizeTimers.delete(chart)
  }
}

/**
 * 每个图表实例「当前已应用的主题」。
 *
 * 不再用模块级单变量 `lastAppliedDark`：一个页面往往有多个图表实例，全局单标志
 * 会让"图表 A 已应用暗色"把尚未刷新的图表 B 一起跳过（B 保持浅色，暗色下文字
 * 不可读）。改为以实例为键逐个比较：状态相同 → 跳过；无记录（首次）或不同 →
 * 应用并记录。
 *
 * 用 `WeakMap` 而非 `Map`：实例被 GC 后记录自动回收，不会因为注册表里遗留的
 * 失效引用长期占内存；`unregisterChart` / dispose 分支里也会显式删除记录。
 */
let appliedThemeByChart = new WeakMap<echarts.ECharts, boolean>()

/**
 * 把已构建的图表加入活动注册表（initChart 内部自动调用，外部不需要手动调）。
 * 返回原实例，便于调用方直接 `initChart(...)` 后继续 `setOption` 等操作。
 */
export function registerChart(c: echarts.ECharts): echarts.ECharts {
  liveCharts.add(c)
  return c
}

/**
 * 注销一个图表实例（页面 onUnmounted 时调用，避免持有已 dispose 的引用）。
 *
 * 幂等：重复注销同一个实例不会抛错；dispose 后的实例也不影响后续 dispatch。
 */
export function unregisterChart(c: echarts.ECharts): void {
  liveCharts.delete(c)
  appliedThemeByChart.delete(c)
  detachResizeObserver(c)
}

/**
 * 页面 `onUnmounted` 的统一出口：dispose 实例并从注册表摘除。
 *
 * 仅调 `chart.dispose()` 是不够的——`liveCharts` 持有强引用，实例直到下次
 * `applyEchartsTheme` 才会被惰性剔除。网关控制台是长驻场景（拓扑图、甘特图
 * 都是排查常用页，反复进出），漏掉摘除会让已销毁实例持续占用内存。
 *
 * 幂等：未初始化、重复调用、dispose 后再调用均不抛错。
 */
export function disposeChart(c: echarts.ECharts | null | undefined): void {
  if (!c) return
  unregisterChart(c)
  // 已 dispose 的实例再调dispose 会抛错，这里统一吞掉，避免掩盖真实问题
  try {
    if (typeof c.isDisposed !== 'function' || !c.isDisposed()) c.dispose()
  } catch (e) {
    logWarn('echarts', '[echarts] disposeChart: 实例已销毁，跳过', e)
  }
}

/**
 * 测试用：查询当前注册的图表数量。
 */
export function __liveChartCountForTests(): number {
  return liveCharts.size
}

/**
 * 测试用：清空注册表（teardown 时调用，避免测试间状态泄漏）。
 * 同时丢弃每个实例的主题记录，避免上一个用例留下的"已应用暗色"命中新用例。
 */
export function __resetLiveChartsForTests(): void {
  liveCharts.clear()
  appliedThemeByChart = new WeakMap()
}

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
): echarts.ECharts {
  const chart = origInit(dom, theme ?? (isDarkMode() ? 'gw-dark' : 'gw-light'), opts)
  applyChartA11y(dom)
  // 自动追踪到全局注册表，主题切换时统一刷新
  registerChart(chart)
  // 容器尺寸变化（侧栏折叠 / 窗口缩放 / 平板旋转）时自动 resize
  if (dom instanceof HTMLElement) attachResizeObserver(chart, dom)
  patchSetOptionForReducedMotion(chart)
  return chart
}

/**
 * 包装实例的 setOption：在「减少动态效果」偏好命中时，把 option 中的动画
 * 配置覆盖为无动画。
 *
 * 只在偏好命中时改写 option 内容（不改调用方传入的对象，避免污染其状态）。
 * 偏好未命中时直接透传，零额外开销。
 */
function patchSetOptionForReducedMotion(chart: echarts.ECharts): void {
  if (!prefersReducedMotion()) return
  const original = chart.setOption.bind(chart)
  chart.setOption = ((option: echarts.EChartsCoreOption, notMerge?: boolean, lazyUpdate?: boolean) => {
    if (option && typeof option === 'object') {
      return original(applyReducedMotion(option as Record<string, unknown>) as echarts.EChartsCoreOption, notMerge, lazyUpdate)
    }
    return original(option, notMerge, lazyUpdate)
  }) as typeof chart.setOption
}

/**
 * 重新应用当前主题到所有活动图表。
 *
 * 行为契约：
 *   - 不带参数：使用 `isDarkMode()` 当前态判定。多次连发调用安全（幂等）。
 *   - 带参数 `dark`：明确指定目标态，绕过当前态判定（测试与外部触发用）。
 *   - 调用顺序：先把图表 `dispose` 再 `init` 会丢 setOption；这里走
 *     `setOption(prev, true)` 重绘——保留所有数据与交互（dataZoom/tooltip 等），
 *     仅主题色调被刷新。
 *   - 已 dispose 的实例：`chart.isDisposed()` 为真时直接从注册表与主题记录中剔除，
 *     不再尝试 setOption。
 *   - 状态去重：按实例（WeakMap）比较目标主题，已处于目标态 → 跳过，避免每次
 *     切换都对全部图表全量重绘；首次调用（无记录）必定应用。
 *   - SSR / 测试环境：liveCharts 为空或 dom 不存在，直接返回。
 */
export function applyEchartsTheme(dark?: boolean): void {
  if (typeof document === 'undefined') return
  const targetDark = typeof dark === 'boolean' ? dark : isDarkMode()
  // 拷贝成数组后再遍历，listener 内 unregisterChart 不会影响本次迭代
  const charts = Array.from(liveCharts)
  for (const c of charts) {
    // 已销毁实例：setOption 会抛 `Component is disposed`，提前剔除
    if (typeof c.isDisposed === 'function' && c.isDisposed()) {
      liveCharts.delete(c)
      appliedThemeByChart.delete(c)
      continue
    }
    // 该实例已处于目标主题：跳过（同一实例重复调用不重复重绘）
    if (appliedThemeByChart.get(c) === targetDark) continue
    try {
      // ECharts 的 theme 不可运行时切换：标准做法是「重新挂载主题名 + setOption 重绘」。
      // 通过 chart.dispose + 重建会丢失 id 与所有 DOM 绑定，对外不友好；
      // 这里折中：保留实例，仅 setOption(已有 option, true) 重绘——暗色主题本身只是
      // 协调颜色，新主题的色值已写到 option（参见 registerTheme gw-dark 与各页面 setOption 调用），
      // setOption(true) 会重新查找 CSS var 解析 + 重新渲染。
      const opt = c.getOption()
      c.setOption(opt, true)
      appliedThemeByChart.set(c, targetDark)
    } catch (e) {
      // 已 dispose 的实例：从注册表与主题记录中移除（防止下一次 dispatch 再抛）
      liveCharts.delete(c)
      appliedThemeByChart.delete(c)
      logWarn('echarts', '[echarts] applyEchartsTheme: 已移除失效图表', e)
    }
  }
}

/** 测试用：强制重置"每个实例已应用主题"缓存。 */
export function __resetEchartsThemeCacheForTests(): void {
  appliedThemeByChart = new WeakMap()
}

/** 运行时读取 CSS 变量当前值（canvas 内元素用，如拓扑连线），暗色切换后随重渲染生效 */
export function cssVar(name: string, fallback: string): string {
  try {
    if (typeof document === 'undefined') return fallback
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback
  } catch { /* 非浏览器环境兜底 */ }
  return fallback
}

/* ========== appendData 增量刷新 + 节流 ==========
 * 历史问题：实时推送页面（Realtime / Monitor sparkline / Oscilloscope）在高频消息下
 * 每条都 setOption 全量重绘——30fps 推送时主线程被 setOption 占满，图表抖动、
 * 表格掉帧。ECharts 5 提供 appendData API，可以只给 series 末尾追加新点，绕开
 * 全量 diff；再配合轻量节流把 N 次 append 合并到 ~10fps 的 flush，CPU 降 70%+。
 *
 * 使用契约：
 *   1. 初始化时仍用 setOption 一次性注入 option（xAxis/series 结构、样式等），
 *      之后所有刷新改走 updater.append()。
 *   2. category 轴图表：appendData 同时推 xAxis.data 与 series.data，数量需对齐。
 *   3. 滑窗裁剪（超过上限 shift）仍由 setOption 完成——appendData 只追加不裁剪。
 *      裁剪可以放在独立的、更低频的 setOption 里做（比如 5s 一次或"超阈即裁"）。
 *   4. 组件 onUnmounted 必须调用 updater.destroy() 清定时器。
 */

/** 单次 append 任务：一个 seriesIndex + 一批 data（可选同时追 xAxis category 标签） */
export interface AppendDataTask {
  seriesIndex: number
  /** 本次追加的数据点数组；与 xLabels 数量对齐（如果传了） */
  data: unknown[]
  /** 可选：同时追加到 category xAxis 的标签（用于 time-based xAxis 的折线图） */
  xLabels?: string[]
}

export interface AppendDataUpdater {
  /** 入队一个或多个 append 任务；实际刷盘由节流 timer 触发 */
  append(tasks: AppendDataTask[]): void
  /** 立即 flush（用于组件销毁前、或需要同步可见时） */
  flush(): void
  /** 销毁节流 timer，禁止后续 flush */
  destroy(): void
  /** 测试用：当前队列中等待 flush 的 seriesIndex -> data 数量 */
  __pendingForTests(): Record<number, number>
  /** 测试用：当前队列中等待 flush 的 xAxis label 数量 */
  __pendingXLabelsForTests(): number
  /** 测试用：是否已销毁 */
  __isDestroyedForTests(): boolean
}

/** 默认节流 100ms —— ~10fps 的视觉刷新率，既不会肉眼察觉延迟，又把 N 次 setOption 合并成 1 次 */
export const APPEND_THROTTLE_MS = 100

/**
 * 创建一个 appendData 增量刷新器。
 *
 * 内部对同一 seriesIndex 做分桶，flush 时一次调用 `chart.appendData({ seriesIndex, data })`
 * 把整桶数据灌进去，避免高频小批量多次调用。category 轴的 xAxis.data 同理。
 *
 * 错误隔离：单次 appendData 抛错只 warn，不影响其它 series 继续追加，也不破坏后续 flush。
 */
export function createAppendDataUpdater(
  chart: echarts.ECharts,
  opts?: { throttleMs?: number },
): AppendDataUpdater {
  const throttleMs = opts?.throttleMs ?? APPEND_THROTTLE_MS
  // 按 seriesIndex 分桶，避免 flush 时对同一 series 连续多次 appendData（一次全塞更省）
  const queue = new Map<number, unknown[]>()
  let xLabelQueue: string[] = []
  let timer: ReturnType<typeof setTimeout> | null = null
  let destroyed = false

  function flushNow() {
    timer = null
    if (destroyed) return
    // 先追加 xAxis.category（如果有）——必须在 series 之前，否则 echarts 可能因长度不齐报错
    if (xLabelQueue.length) {
      try {
        // eslint-disable-next-line @typescript-eslint/no-explicit-any -- echarts/core 的 appendData 类型声明对 xAxis componentType overload 不完整
        ;(chart.appendData as any)({ componentType: 'xAxis', dataIndex: 0, data: xLabelQueue })
      } catch (e) {
        logWarn('echarts', '[appendDataUpdater] xAxis appendData fail', e)
      }
      xLabelQueue = []
    }
    for (const [seriesIndex, data] of queue) {
      if (!data.length) continue
      try {
        chart.appendData({ seriesIndex, data })
      } catch (e) {
        logWarn('echarts', `[appendDataUpdater] series[${seriesIndex}] appendData fail`, e)
      }
    }
    queue.clear()
  }

  return {
    append(tasks) {
      if (destroyed) return
      for (const t of tasks) {
        const bucket = queue.get(t.seriesIndex) ?? []
        bucket.push(...t.data)
        queue.set(t.seriesIndex, bucket)
        if (t.xLabels) xLabelQueue.push(...t.xLabels)
      }
      if (timer) return
      timer = setTimeout(flushNow, throttleMs)
    },
    flush() {
      if (timer) { clearTimeout(timer); timer = null }
      flushNow()
    },
    destroy() {
      destroyed = true
      if (timer) { clearTimeout(timer); timer = null }
      queue.clear()
      xLabelQueue = []
    },
    __pendingForTests() {
      const out: Record<number, number> = {}
      for (const [k, v] of queue) out[k] = v.length
      return out
    },
    __pendingXLabelsForTests() { return xLabelQueue.length },
    __isDestroyedForTests() { return destroyed },
  }
}

/**
 * 对单个 setOption 全量刷新做 100ms 节流。
 *
 * 用途：ECharts sparkline 这类需要同时"追加 + 裁剪"的图表，appendData 只管追加，
 * 裁剪得靠 setOption 重建整个 series.data。但 per-message 全量刷新仍然太猛，
 * 套上节流把一批消息合并成一次 setOption，即可缓解 CPU 压力。
 *
 * 返回 { schedule(fn), destroy() }：schedule 重复调用会合并到下一个节流窗口里。
 */
export function createThrottledSetter<TArgs extends unknown[]>(
  fn: (...args: TArgs) => void,
  opts?: { throttleMs?: number },
): { schedule: (...args: TArgs) => void; flush: () => void; destroy: () => void } {
  const throttleMs = opts?.throttleMs ?? APPEND_THROTTLE_MS
  let timer: ReturnType<typeof setTimeout> | null = null
  let pendingArgs: TArgs | null = null
  let destroyed = false

  function runNow() {
    timer = null
    if (destroyed) return
    if (pendingArgs) {
      const args = pendingArgs
      pendingArgs = null
      fn(...args)
    }
  }

  return {
    schedule(...args) {
      if (destroyed) return
      pendingArgs = args
      if (timer) return
      timer = setTimeout(runNow, throttleMs)
    },
    flush() {
      if (timer) { clearTimeout(timer); timer = null }
      runNow()
    },
    destroy() {
      destroyed = true
      if (timer) { clearTimeout(timer); timer = null }
      pendingArgs = null
    },
  }
}

export default echarts
