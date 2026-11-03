import { nextTick, onMounted, onUnmounted, onUpdated, watch, type Ref } from 'vue'

export interface DataTableA11yOptions {
  /** 表格根容器（表格本体在其内部，DOM 补丁以它为查询范围） */
  wrapRef: Ref<HTMLElement | undefined>
  /** 表格无障碍名称（读屏器播报的标题） */
  getCaption: () => string
  /** 数据行数（虚拟表格的 aria-rowcount 需要 +1 表头行） */
  getRowCount: () => number
  /** 是否处于虚拟滚动分支（决定要不要给虚拟表格 DOM 补 role） */
  useVirtual: Ref<boolean>
}

/**
 * 表格的无障碍补丁逻辑：aria-sort 校准、表格名称（caption / aria-label）、
 * 虚拟滚动表格的表格语义兜底。
 *
 * 这些都是 Element Plus 没有提供插槽、只能直接改 DOM 的部分，统一收口在这里，
 * 避免主组件的 setup 变成一大坨 DOM 查询。
 *
 * 输入：表格根容器 ref、表格名称、数据行数、是否虚拟滚动
 * 输出：无（直接作用于 DOM）
 * 异常：无（容器尚未挂载时静默跳过）
 */
export function useDataTableA11y(options: DataTableA11yOptions): void {
  const { wrapRef } = options

  /**
   * aria-sort 补丁：
   *   - 普通表格：2.14 起原生按 column.order 输出 aria-sort，但未排序的 sortable 列
   *     会输出非法的空串（ARIA 1.2 已废弃 "none"，合法做法是不输出），这里按
   *     ascending/descending class 校准：无方向就移除属性
   *   - 虚拟表格：无原生支持，按自绘表头的 .v2-sortable.is-sorted + is-asc/is-desc 标注
   * 排序方向能在挂载后才写入（default-sort 双 nextTick 才生效），所以挂载与每次
   * 更新后都要重新校准一遍。
   */
  function syncElTableAriaSort() {
    const root = wrapRef.value
    if (!root) return
    for (const th of root.querySelectorAll('th')) {
      if (th.classList.contains('ascending')) th.setAttribute('aria-sort', 'ascending')
      else if (th.classList.contains('descending')) th.setAttribute('aria-sort', 'descending')
      else if (th.hasAttribute('aria-sort')) th.removeAttribute('aria-sort')
    }
  }

  /** 虚拟表格侧校准 */
  function syncV2AriaSort() {
    const root = wrapRef.value
    if (!root) return
    for (const cell of root.querySelectorAll('.el-table-v2__header-cell')) {
      const sorted = cell.querySelector('.v2-sortable.is-sorted')
      if (!sorted) {
        if (cell.hasAttribute('aria-sort')) cell.removeAttribute('aria-sort')
        continue
      }
      cell.setAttribute('aria-sort', sorted.querySelector('.v2-sort-arrow.is-desc') ? 'descending' : 'ascending')
    }
  }

  function syncAriaSort() {
    syncElTableAriaSort()
    syncV2AriaSort()
  }

  /**
   * EP 会在数据落地后的二次布局里重建表头，这次重建由它内部组件发起（不触发本组件
   * onUpdated），带出的 aria-sort="" 晚于本次校准，所以再补一帧后的扫描兜住。
   * 虚拟分支没有 th，调用无副作用。
   */
  function scanElTableAriaSortNextFrame() {
    requestAnimationFrame(() => syncElTableAriaSort())
  }

  /**
   * 虚拟表格语义兜底：全程 div 渲染且不带任何 table 语义 role，行数超过阈值
   * 切到虚拟分支后读屏听到的就不再是表格。这里按结构补 role：
   *   根 → table（aria-rowcount 报总行数）；header / body 网格 → rowgroup；
   *   header-row / row → row；header-cell → columnheader；row-cell → cell。
   * 行随虚拟滚动动态挂载，Grid 内部重渲染不会触发本组件 onUpdated，
   * 所以另用 MutationObserver 兜住滚动补行（只监听 childList，自己写属性不会造成回环）。
   */
  function syncV2Aria() {
    const root = wrapRef.value
    if (!root) return
    const table = root.querySelector('.el-table-v2')
    if (!table) return
    table.setAttribute('role', 'table')
    table.setAttribute('aria-rowcount', String(options.getRowCount() + 1))
    for (const el of table.querySelectorAll('.el-table-v2__header, .el-table-v2__body')) el.setAttribute('role', 'rowgroup')
    for (const el of table.querySelectorAll('.el-table-v2__header-row, .el-table-v2__row')) el.setAttribute('role', 'row')
    for (const el of table.querySelectorAll('.el-table-v2__header-cell')) el.setAttribute('role', 'columnheader')
    for (const el of table.querySelectorAll('.el-table-v2__row-cell')) el.setAttribute('role', 'cell')
  }

  /**
   * 给普通表格补 <caption>（无障碍，P2 #4b）。
   *
   * el-table 内部渲染两张 <table>（fixed 布局时 header/body 分离），Element Plus
   * 没有 caption 插槽，无法从模板传入。表格若无 <caption>，读屏器只会播报
   * 「表格」，用户无从判断这张表装的是设备还是点位。
   *
   * 这里直接在 DOM 里补：对每张 <table> 插入 <caption class="sr-only">。
   * 视觉上完全不可见（sr-only 裁剪为 1px），但读屏器会把它作为表格名称播报。
   *
   * 幂等：已存在 caption 时只更新文本，不重复插入。
   */
  function syncTableCaption() {
    const root = wrapRef.value
    if (!root) return
    for (const table of root.querySelectorAll('table')) {
      // 排除虚拟表格内部的 table（虚拟分支走 aria-label 路线，不塞 caption）
      if (table.closest('.el-table-v2')) continue
      let caption = table.querySelector('caption')
      if (!caption) {
        caption = document.createElement('caption')
        caption.className = 'sr-only'
        // caption 必须是 table 的第一个子元素
        table.insertBefore(caption, table.firstChild)
      }
      const text = options.getCaption()
      if (caption.textContent !== text) caption.textContent = text
    }
  }

  /**
   * 虚拟表格的表格名称：虚拟分支全程 div 渲染，没有 caption 语义，
   * 只能用 aria-label 标注表格名称（配合 syncV2Aria 打的 role="table"）。
   */
  function syncV2Caption() {
    const root = wrapRef.value
    if (!root) return
    const table = root.querySelector('.el-table-v2')
    if (!table) return
    table.setAttribute('aria-label', options.getCaption())
  }

  let ariaObserver: MutationObserver | undefined

  /**
   * 虚拟表格结构观察器：行随虚拟滚动动态挂载，Grid 内部重渲染不触发本组件
   * onUpdated，只能靠 childList 观察兜住补行/补 role。
   * 只给虚拟分支用：普通表格侧往表里写 DOM（插 caption、改 th 属性）会扰动 EP 的尺寸观察，
   * 触发它重建表头，从而与观察回调互相激发（实测会把主线程卡死）。
   */
  function startV2AriaObserver() {
    if (ariaObserver || !wrapRef.value) return
    ariaObserver = new MutationObserver(() => {
      syncV2Aria()
      syncTableCaption()
      syncV2Caption()
    })
    ariaObserver.observe(wrapRef.value, { childList: true, subtree: true })
  }

  function stopV2AriaObserver() {
    ariaObserver?.disconnect()
    ariaObserver = undefined
  }

  // 首次校准要等表格列注册引发的重渲染落地（th 此时才带上原生 aria-sort），
  // 单拍 nextTick 可能跑在 th 出现之前；清理过之后 '' → '' 的 patch 不会重设，幂等
  onMounted(() => nextTick(() => nextTick(() => {
    syncAriaSort(); syncV2Aria(); syncTableCaption(); syncV2Caption(); scanElTableAriaSortNextFrame()
  })))
  onUpdated(() => { syncAriaSort(); syncV2Aria(); syncTableCaption(); syncV2Caption(); scanElTableAriaSortNextFrame() })

  // caption 文案随列显隐/列定义变化（如用户切换列显示设置），需要同步到 DOM
  watch(() => options.getCaption(), () => { syncTableCaption(); syncV2Caption() })

  // 阈值两侧互切（筛选后行数变化）时跟随启停；immediate 覆盖"挂载即超阈值"的首帧
  watch(options.useVirtual, (v) => {
    if (v) nextTick(startV2AriaObserver)
    else stopV2AriaObserver()
  }, { immediate: true })

  onUnmounted(stopV2AriaObserver)
}
