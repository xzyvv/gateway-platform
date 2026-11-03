import { computed, nextTick, ref, shallowRef, watch, type Ref } from 'vue'

/**
 * 日志窗口化渲染（虚拟滚动）
 *
 * 解决的问题：Logs 渲染区最多 1000 条 v-html 行，日志流式刷入时（每 200ms 一次
 * flush）整列表反复 diff，低配工控机上滚动明显卡顿。
 *
 * 方案与取舍：
 *  - **阈值门控**：行数 ≤ threshold 时全量渲染（与 DataTable 的 VIRTUAL_THRESHOLD
 *    策略一致），常见场景零行为差异；只有日志风暴时才进入窗口模式。
 *  - **行高可变**：`.log-line` 是 pre-wrap 会换行，行高不固定。渲染后实测窗口内
 *    行高缓存（按日志 id，id 单调递增且与文本一一对应，跨筛选状态稳定），
 *    未渲染行用估算值兜底（12px × 1.5 行高 + 4px padding ≈ 22px）。
 *  - **底部锚定走 tail 窗口**：atBottom 时不依赖 scrollTop 时序，直接从尾部
 *    反推窗口，flush 追加新日志后 scrollToBottom 一定命中，绕开"先渲染还是
 *    先测量"的竞态。
 *  - **头部截断剪枝**：MAX_LOGS 截断只从头部 splice，被裁掉的 id 从缓存清除
 *    （id 单调递增，小于新首 id 的缓存必已滚出缓冲区）。
 */

/** 单行估算高度：0.75rem(12px) × 1.5 行高 + 上下 padding 2px×2 ≈ 22px */
export const ESTIMATED_ROW_HEIGHT = 22

export interface VirtualLogItem {
  id: number
  text: string
}

export interface WindowRange {
  /** [start, end) 行下标区间 */
  start: number
  end: number
}

/**
 * 前缀和：prefix[i] = 前 i 行的总高度。
 * heights 未命中的行用估算值兜底；prefix[n] 即列表总高。
 */
export function buildPrefix(
  items: VirtualLogItem[],
  heights: Map<number, number>,
  estimate: number = ESTIMATED_ROW_HEIGHT,
): number[] {
  const prefix = new Array<number>(items.length + 1)
  prefix[0] = 0
  for (let i = 0; i < items.length; i++) {
    prefix[i + 1] = prefix[i]! + (heights.get(items[i]!.id) ?? estimate)
  }
  return prefix
}

/** scrollTop → 起始行下标（prefix 单调递增，二分；返回值可为 n，调用方需 clamp） */
export function findStartIndex(prefix: number[], scrollTop: number): number {
  let lo = 0
  let hi = prefix.length - 1
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (prefix[mid]! <= scrollTop) lo = mid
    else hi = mid - 1
  }
  return lo
}

/** 按滚动位置取窗口：覆盖 [scrollTop, scrollTop+viewport]，上下各多渲染 overscan 行 */
export function scrollTopRange(
  prefix: number[],
  scrollTop: number,
  viewport: number,
  overscan = 10,
): WindowRange {
  const n = prefix.length - 1
  if (n === 0) return { start: 0, end: 0 }
  const start = Math.min(findStartIndex(prefix, scrollTop), n - 1)
  const from = Math.max(0, start - overscan)
  const bottom = scrollTop + viewport
  let end = from
  while (end < n && prefix[end]! < bottom) end++
  return { start: from, end: Math.min(n, end + overscan) }
}

/** 底部锚定窗口：从尾部向上凑满 viewport + overscan 余量，bottomPad 恒为 0 */
export function tailRange(prefix: number[], viewport: number, overscan = 10): WindowRange {
  const n = prefix.length - 1
  if (n === 0) return { start: 0, end: 0 }
  const target = viewport + overscan * ESTIMATED_ROW_HEIGHT
  const total = prefix[n]!
  let start = n
  while (start > 0 && total - prefix[start - 1]! < target) start--
  return { start, end: n }
}

export function useVirtualLogs(opts: {
  /** 过滤后的日志列表（filteredLogs） */
  source: Ref<VirtualLogItem[]>
  /** 滚动容器（logBox） */
  container: Ref<HTMLElement | undefined>
  /** 是否钉在底部（tail 窗口模式的开关） */
  atBottom: Ref<boolean>
  /** 行数超过该值才启用窗口化，否则全量渲染 */
  threshold: number
  /**
   * 数据版本信号（每次 flush 自增）。
   *
   * 必须显式传入：filteredLogs 无筛选时直接返回 logs.value 同一数组引用
   * （W24 优化），Vue computed 对 Object.is 相等的返回值不会触发下游 computed，
   * 虚拟化链路将全部读到陈旧长度。所有暴露的 computed 都消费 version 强制重算。
   */
  version: Ref<number>
}) {
  const { source, container, atBottom, threshold, version } = opts

  const enabled = computed(() => {
    void version.value
    return source.value.length > threshold
  })
  const scrollTopRef = ref(0)
  const viewportH = ref(0)
  /** id → 实测行高（px）。shallowRef：只在实测变化时整体替换，驱动 prefix 重算 */
  const heights = shallowRef(new Map<number, number>())

  // 头部截断剪枝：日志 id 单调递增，列表首 id 变大说明头部被 MAX_LOGS 裁过。
  // 注意 watch 的对象是 version 而非 source——source 返回同引用时 watch 不触发
  watch(version, () => {
    const list = source.value
    if (list.length === 0) {
      if (heights.value.size > 0) heights.value = new Map()
      return
    }
    const minId = list[0]!.id
    let pruned = false
    for (const id of heights.value.keys()) {
      if (id < minId) {
        heights.value.delete(id)
        pruned = true
      }
    }
    if (pruned) heights.value = new Map(heights.value)
  })

  const prefix = computed(() => {
    void version.value
    return enabled.value ? buildPrefix(source.value, heights.value) : []
  })

  const range = computed<WindowRange>(() => {
    void version.value
    if (!enabled.value) return { start: 0, end: source.value.length }
    const p = prefix.value
    const viewport = viewportH.value || container.value?.clientHeight || 400
    return atBottom.value
      ? tailRange(p, viewport)
      : scrollTopRange(p, scrollTopRef.value, viewport)
  })

  const topPad = computed(() => {
    void version.value
    return enabled.value ? prefix.value[range.value.start] ?? 0 : 0
  })
  const bottomPad = computed(() => {
    void version.value
    return enabled.value
      ? (prefix.value[source.value.length] ?? 0) - (prefix.value[range.value.end] ?? 0)
      : 0
  })
  const renderItems = computed(() => {
    void version.value
    return enabled.value ? source.value.slice(range.value.start, range.value.end) : source.value
  })

  /** 滚动事件回调：更新窗口定位信号（atBottom 判定仍由 useLogs.onScroll 负责） */
  function onScroll(el: HTMLElement) {
    if (!enabled.value) return
    scrollTopRef.value = el.scrollTop
    viewportH.value = el.clientHeight
  }

  /**
   * 渲染后实测窗口内行高。jsdom 里 offsetHeight 恒为 0，守卫保证测试环境零副作用；
   * 实测无变化时不替换 Map，watch(renderItems) 链路两轮内收敛、不会死循环。
   */
  async function measure(): Promise<void> {
    const box = container.value
    if (!box || !enabled.value) return
    await nextTick()
    if (!container.value) return
    const rows = box.querySelectorAll<HTMLElement>('[data-lid]')
    if (rows.length === 0) return
    const next = new Map(heights.value)
    let changed = false
    rows.forEach((row) => {
      const id = Number(row.dataset.lid)
      const h = row.offsetHeight
      if (h > 0 && next.get(id) !== h) {
        next.set(id, h)
        changed = true
      }
    })
    if (changed) {
      heights.value = next
      viewportH.value = box.clientHeight
      // 实测行高让 scrollHeight 变化，钉底状态需要重新吸底（flush 追加场景），
      // 否则估算与真实的差值会随日志流不断累积，把用户"顶离"底部
      if (atBottom.value) {
        await nextTick()
        if (container.value && atBottom.value) {
          container.value.scrollTop = container.value.scrollHeight
        }
      }
    }
  }

  return {
    virtualEnabled: enabled,
    renderItems,
    topPad,
    bottomPad,
    onScroll,
    measure,
  }
}
