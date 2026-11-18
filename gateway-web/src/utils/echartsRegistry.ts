/**
 * 图表实例注册表与生命周期管理。
 *
 * 从原 `utils/echarts.ts` 拆出：这里管的是「哪些图表还活着」以及它们的
 * 销毁、尺寸自适应、主题重绘，与建图细节（initChart / option 组装）无关。
 *
 * 关键约束：`liveCharts` 用 `Set` 而非 `WeakSet`——虽然 `WeakSet` 自动 GC、
 * 理论更安全，但 echarts 实例化后由 `chart.dispose()` 主动释放；保留 `Set` +
 * 显式 `unregisterChart` 是为了让"页面只 dispose 但忘了 unregister"也活得下来
 * ——只要 `applyEchartsTheme` 能拿到即可。Disposed 实例在 dispose 时已经从
 * DOM 移除，对无效实例调用 `setOption` 会被 echarts 内部静默 no-op，不会抛错。
 */
import type { ECharts } from 'echarts/core'
import { isDarkMode } from './echartsThemes'
import { logWarn } from './logger'

/**
 * 活动图表注册表。
 *
 * 所有通过 `initChart` 创建的实例都会进入这里，主题切换时统一重绘。
 */
const liveCharts = new Set<ECharts>()

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
const chartObservers = new WeakMap<ECharts, ResizeObserver>()
/** resize 节流定时器：连续尺寸变化（如拖拽窗口）合并成一次 resize */
const chartResizeTimers = new WeakMap<ECharts, ReturnType<typeof setTimeout>>()

const RESIZE_DEBOUNCE_MS = 120

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

function attachResizeObserver(chart: ECharts, dom: HTMLElement): void {
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

function detachResizeObserver(chart: ECharts): void {
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
let appliedThemeByChart = new WeakMap<ECharts, boolean>()

/**
 * 把已构建的图表加入活动注册表（initChart 内部自动调用，外部不需要手动调）。
 * 返回原实例，便于调用方直接 `initChart(...)` 后继续 `setOption` 等操作。
 */
export function registerChart(c: ECharts): ECharts {
  liveCharts.add(c)
  return c
}

/**
 * 注销一个图表实例（页面 onUnmounted 时调用，避免持有已 dispose 的引用）。
 *
 * 幂等：重复注销同一个实例不会抛错；dispose 后的实例也不影响后续 dispatch。
 */
export function unregisterChart(c: ECharts): void {
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
export function disposeChart(c: ECharts | null | undefined): void {
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

/** 测试用：查询当前注册的图表数量。 */
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

/** 测试用：强制重置"每个实例已应用主题"缓存。 */
export function __resetEchartsThemeCacheForTests(): void {
  appliedThemeByChart = new WeakMap()
}

/**
 * 运行时读取 CSS 变量当前值（canvas 内元素用，如拓扑连线），暗色切换后随重渲染生效
 */
export function cssVar(name: string, fallback: string): string {
  try {
    if (typeof document === 'undefined') return fallback
    return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback
  } catch { /* 非浏览器环境兜底 */ }
  return fallback
}

/** 供 `echartsInit` 复用：把实例挂上容器尺寸自适应。 */
export function attachChartResizeObserver(chart: ECharts, dom: HTMLElement): void {
  attachResizeObserver(chart, dom)
}
