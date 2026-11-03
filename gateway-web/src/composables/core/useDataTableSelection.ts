import { computed, shallowRef, watch } from 'vue'

export interface DataTableSelectionOptions<T> {
  /** 当前渲染的数据行（分页时只是当前页） */
  getData: () => T[]
  /** 行唯一键：字段名或取值函数 */
  getRowKey: () => string | ((row: T) => string | number)
  /** 行级勾选禁用判定（例如只读行不参与批量操作）；未提供时全部可选 */
  getRowSelectable: () => ((row: T) => boolean) | undefined
  /** 是否启用复选框列 */
  getSelectable: () => boolean
  /** 翻页/换页时是否保留勾选（配合「跨页批量操作」）；为 false 时数据源一变就清空 */
  getPreserveSelection: () => boolean
  /** 勾选变化回调：抛出整行对象 */
  onChange: (rows: T[]) => void
}

/**
 * 表格勾选态管理。
 *
 * 选中态由组件自管：以 rowKey 为索引，派生勾选态并向外抛整行对象。
 * 用 Map 而不是 Set 存整行，是为了让跨页勾选（只渲染当前页时）仍能拿到未渲染页的行对象。
 * shallowRef 而非 ref：泛型 T 若被深度解包会与表格的行类型对不上，且行对象无需深代理。
 *
 * 输入：数据行 / rowKey / rowSelectable / preserveSelection
 * 输出：selectedRows（整行对象数组）、selectedIndex（key 数组）、allSelected / indeterminate、isSelected / canSelect、onHeaderToggle / onRowToggle / clearSelection
 * 异常：无
 */
export function useDataTableSelection<T>(options: DataTableSelectionOptions<T>) {
  const selectedMap = shallowRef<Map<string, T>>(new Map())

  const selectedRows = computed(() => Array.from(selectedMap.value.values()))
  const selectedIndex = computed(() => [...selectedMap.value.keys()])

  /** 行唯一键（统一口径：函数型 rowKey 与字符串型走同一条路径） */
  function keyOf(row: T): string {
    const rk = options.getRowKey()
    return typeof rk === 'function' ? String(rk(row)) : String((row as Record<string, unknown>)[rk] ?? '')
  }

  /** 行是否可被勾选：默认全部可选，rowSelectable 可排除只读行 */
  function canSelect(row: T): boolean {
    const fn = options.getRowSelectable()
    return fn ? fn(row) : true
  }

  /** 当前数据中实际可勾选的行：全选/半选只在这些行上判定，避免只读行让全选永远不成立 */
  const selectableRows = computed(() => options.getData().filter((row) => canSelect(row)))

  const allSelected = computed(
    () => selectableRows.value.length > 0 && selectableRows.value.every((row) => selectedMap.value.has(keyOf(row))),
  )

  const indeterminate = computed(() => {
    if (allSelected.value) return false
    return selectableRows.value.some((row) => selectedMap.value.has(keyOf(row)))
  })

  // 数据源整体替换时的处理：
  //   preserveSelection = false（默认）→ 清空，避免删完一批后旧勾选残留；
  //   preserveSelection = true（跨页批量）→ 保持不动。这里刻意不做「剔除已不存在的行」：
  //   分页时当前 data 只是全集的一个子集，按子集裁剪会把其他页的勾选误删。
  //   行被删除后的残留由使用方在数据刷新时调 clearSelection() 清理（见 Points.vue）。
  // watch 数据源本身即可：调用方一律整体替换数组（loadPoints 里 points.value = data），
  // 引用必变，浅比较足够。若将来改为 push/原地改，就需改成 watch 数据源长度。
  watch(
    () => options.getData(),
    () => {
      if (options.getSelectable() && !options.getPreserveSelection()) selectedMap.value = new Map()
    },
  )

  function emitSelection() {
    options.onChange(selectedRows.value)
  }

  function isSelected(row: T): boolean {
    return selectedMap.value.has(keyOf(row))
  }

  /**
   * 表头全选只作用于当前渲染的数据（分页时即"全选当前页"），
   * 同时保留其他页已勾选的行——否则翻页做批量时前面的勾选会被悄悄抹掉。
   */
  function onHeaderToggle(checked: unknown) {
    const next = new Map(selectedMap.value)
    // 只对可勾选行生效：只读行既不能被全选，也不应被全选取消
    if (checked) selectableRows.value.forEach((row) => next.set(keyOf(row), row))
    else selectableRows.value.forEach((row) => next.delete(keyOf(row)))
    selectedMap.value = next
    emitSelection()
  }

  function onRowToggle(row: T, checked: unknown) {
    const k = keyOf(row)
    const next = new Map(selectedMap.value)
    if (checked) next.set(k, row)
    else next.delete(k)
    selectedMap.value = next
    emitSelection()
  }

  /** 供使用方在批量操作/刷新后清空勾选（与内部 selection-change 同步） */
  function clearSelection() {
    selectedMap.value = new Map()
    emitSelection()
  }

  return {
    selectedRows,
    selectedIndex,
    keyOf,
    canSelect,
    isSelected,
    allSelected,
    indeterminate,
    onHeaderToggle,
    onRowToggle,
    clearSelection,
  }
}
