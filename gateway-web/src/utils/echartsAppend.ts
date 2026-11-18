/**
 * appendData 增量刷新 + setOption 节流。
 *
 * 从原 `utils/echarts.ts` 拆出：这两块是「高频数据写入路径」，与建图/主题
 * 管理正交，单独成模块便于压测与单测。
 *
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
import type { ECharts } from 'echarts/core'
import { logWarn } from './logger'

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
  chart: ECharts,
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
