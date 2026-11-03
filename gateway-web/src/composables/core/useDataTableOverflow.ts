import { nextTick, onMounted, onUnmounted, onUpdated, ref, type Ref } from 'vue'

export interface DataTableOverflowOptions {
  /** 表格根容器（横向溢出提示靠实测它的内部滚动容器得出） */
  wrapRef: Ref<HTMLElement | undefined>
}

/**
 * 表格横向溢出提示。
 *
 * 笔记本 1366 宽下常有列被截在右缘之外（如设备表 13 列），用户不会主动去拖
 * 底部滚动条，容易以为「就这些列」。这里在列宽超出可视宽度时给右缘加一层渐隐阴影，
 * 滚到最右端就收起——否则会暗示「右边还有」，反而把用户往错误方向引。
 * 状态由 DOM 实测得出（scrollWidth vs clientWidth），列显隐切换后也能自动跟上。
 *
 * 输入：表格根容器 ref
 * 输出：hOverflow（右侧还有列）、hAtEnd（已滚到最右端）
 * 异常：无（容器未挂载时静默跳过）
 */
export function useDataTableOverflow(options: DataTableOverflowOptions) {
  const { wrapRef } = options

  const hOverflow = ref(false)
  const hAtEnd = ref(false)
  let hWrap: HTMLElement | null = null

  function onHScroll() {
    if (!hWrap) return
    hAtEnd.value = hWrap.scrollLeft + hWrap.clientWidth >= hWrap.scrollWidth - 1
  }

  /** 与表格布局同步：EP 在数据/列变化后会重建内部滚动容器，每次更新都重新取一次并沿用旧监听 */
  function syncHScrollCue() {
    const wrap = wrapRef.value?.querySelector<HTMLElement>('.el-table__body-wrapper .el-scrollbar__wrap') ?? null
    if (wrap !== hWrap) {
      hWrap?.removeEventListener('scroll', onHScroll)
      hWrap = wrap
      hWrap?.addEventListener('scroll', onHScroll, { passive: true })
    }
    if (!hWrap) {
      hOverflow.value = false
      return
    }
    hOverflow.value = hWrap.scrollWidth - hWrap.clientWidth > 1
    onHScroll()
  }

  /** 侧栏折叠 / 窗口缩放只改容器宽度，不触发父组件重渲染，得靠观察器补一次 */
  let hResizeObserver: ResizeObserver | undefined

  onMounted(() => nextTick(() => nextTick(() => {
    syncHScrollCue()
    if (typeof ResizeObserver !== 'undefined' && wrapRef.value) {
      hResizeObserver = new ResizeObserver(() => syncHScrollCue())
      hResizeObserver.observe(wrapRef.value)
    }
  })))

  onUpdated(() => { syncHScrollCue() })

  onUnmounted(() => {
    hWrap?.removeEventListener('scroll', onHScroll)
    hWrap = null
    hResizeObserver?.disconnect()
    hResizeObserver = undefined
  })

  return { hOverflow, hAtEnd }
}
