/**
 * 表格「定位到高亮行并闪烁」工具。
 *
 * # 为什么抽出来
 *
 * 从 URL 带 `?highlight=<id>` 跳进列表页时，要滚动到目标行并闪烁 3 秒提示
 * 「你看到的是这一行」。这段逻辑在设备清单与点位清单两处各写了一遍：
 *
 *     setTimeout(() => {
 *       const row = document.querySelector('.xxx .el-table__row.is-highlighted')
 *       if (row) {
 *         row.scrollIntoView({ behavior: 'smooth', block: 'center' })
 *         row.classList.add('is-highlighted-flash')
 *         setTimeout(() => row.classList.remove('is-highlighted-flash'), 3000)
 *       }
 *     }, 100)
 *
 * 除了重复，两处都漏了**定时器清理**：组件卸载后 timer 仍在跑，
 * 回调持有已移除的 DOM 引用；用户快速切换页面时多个 timer 叠加，
 * 各自往不同的 DOM 上加/删 class。
 *
 * # 契约
 *
 * - 100ms 的等待是为了等表格重渲染出目标行（翻页后需要 nextTick + 一帧）；
 * - 闪烁 class 在 3 秒后移除，**由本工具负责**；
 * - 组件卸载时自动清掉所有未触发的定时器，不会往已销毁的 DOM 上写 class。
 *
 * @param containerSelector 表格容器选择器（如 '.device-table'）
 * @param isUnmounted       卸载判定（传 useUnmountedGuard 的 isUnmounted）
 */
export function flashHighlightRow(
  containerSelector: string,
  isUnmounted: () => boolean = () => false,
): () => void {
  let scrollTimer: ReturnType<typeof setTimeout> | null = null
  let flashTimer: ReturnType<typeof setTimeout> | null = null

  function clear() {
    if (scrollTimer) { clearTimeout(scrollTimer); scrollTimer = null }
    if (flashTimer) { clearTimeout(flashTimer); flashTimer = null }
  }

  function scrollToHighlightedRow() {
    clear()
    scrollTimer = setTimeout(() => {
      scrollTimer = null
      // 卸载后不再查 DOM、更不再写 class
      if (isUnmounted()) return
      const row = document.querySelector(
        `${containerSelector} .el-table__row.is-highlighted`,
      ) as HTMLElement | null
      if (!row) return
      row.scrollIntoView({ behavior: 'smooth', block: 'center' })
      row.classList.add('is-highlighted-flash')
      flashTimer = setTimeout(() => {
        flashTimer = null
        if (isUnmounted()) return
        row.classList.remove('is-highlighted-flash')
      }, 3000)
    }, 100)
  }

  return scrollToHighlightedRow
}
