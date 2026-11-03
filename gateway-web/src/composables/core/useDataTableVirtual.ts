import { computed, h, isVNode, type Component, type VNode } from 'vue'
import { ElCheckbox, ElTooltip, type Column } from 'element-plus'
import type { TableV2FixedDir } from 'element-plus/es/components/table-v2'
import type { ColumnDef } from '@/components/dataTable/types'
import { valueOf } from './useDataTableColumns'

/** data 行数超过该值自动切换虚拟滚动 */
export const VIRTUAL_THRESHOLD = 200

/**
 * 虚拟滚动表格的最小像素列宽下限。
 * 声明 width/minWidth 都小于此值的列会被钳住，避免多列挤在一起时表头抖动。
 * （普通表格原生 min-width 是弹性下限，虚拟表格必须给确定像素值，这里统一兜底）
 */
const MIN_COLUMN_WIDTH = 80

/**
 * 虚拟滚动表格的 rowKey 只接受字符串字段名（见 element-plus table-v2 common.mjs:28-35，default: "id"），
 * 不支持函数。当 rowKey 是函数时（如 RealtimeMonitor 的 `row => row.point_id`），
 * 若直接透传 undefined 会回退到默认 "id"，而行对象无 id 字段 → 行高测量/选中/展开全部错乱。
 *
 * 解决：函数模式下给每行注入一个非枚举的键字段，v2RowKey 指向该字段；
 * 字符串模式下直接透传字段名，data 原样传递不做拷贝。
 */
const V2_KEY_FIELD = '__dt_v2_row_key__'

export interface DataTableVirtualSelection<T> {
  /** 当前页是否全选 */
  allSelected: () => boolean
  /** 当前页是否半选 */
  indeterminate: () => boolean
  /** 行是否已勾选 */
  isSelected: (row: T) => boolean
  /** 行是否可勾选 */
  canSelect: (row: T) => boolean
  /** 表头全选切换 */
  onHeaderToggle: (checked: unknown) => void
  /** 行勾选切换 */
  onRowToggle: (row: T, checked: unknown) => void
}

export interface DataTableVirtualOptions<T> {
  /** 当前渲染的数据行 */
  getData: () => T[]
  /** 行唯一键：字段名或取值函数 */
  getRowKey: () => string | ((row: T) => string | number)
  /** true 强制虚拟，false 强制普通，'auto' 按行数自动选择 */
  getVirtualMode: () => boolean | 'auto'
  /** 虚拟滚动容器高度（px） */
  getHeight: () => number
  /** 最大高度：虚拟表需要数值，字符串形式的高度表达式无法静态求值 */
  getMaxHeight: () => number | string | undefined
  /** 可见列 */
  getVisibleColumns: () => ColumnDef<T>[]
  /** 是否启用复选框列 */
  getSelectable: () => boolean
  /** 受控排序：当前排序列 key */
  getSortKey: () => string | null
  /** 受控排序方向 */
  getSortOrder: () => 'asc' | 'desc'
  /** 表头点击/键盘触发的排序请求（由主组件统一抛给使用方） */
  requestSort: (col: ColumnDef<T>) => void
  /** 单元格渲染器组件（携带使用方 scopeId） */
  cellView: Component
  /** 勾选态（与普通表格分支共用同一份状态） */
  selection: DataTableVirtualSelection<T>
}

/**
 * 虚拟滚动分支的全部计算：引擎选择、有效高度、虚拟列定义、虚拟数据源。
 *
 * 输入：数据行 / rowKey / 列定义 / 排序状态 / 勾选态
 * 输出：useVirtual（是否走虚拟分支）、effectiveHeight（容器高度）、v2RowKey、v2Data、v2Columns
 * 异常：无
 */
export function useDataTableVirtual<T>(options: DataTableVirtualOptions<T>) {
  const useVirtual = computed(() => {
    const mode = options.getVirtualMode()
    if (mode === true) return true
    if (mode === false) return false
    return options.getData().length > VIRTUAL_THRESHOLD
  })

  /**
   * 虚拟容器实际高度：受 maxHeight 上限约束（普通表格侧由 max-height 原生处理）。
   * 字符串形式的高度表达式（如 calc(100vh - 320px)）无法静态求值，虚拟表沿用 height，
   * 外层 .virtual-table-wrap 的 60vh 上限仍会兜住。
   */
  const effectiveHeight = computed(() => {
    const cap = options.getMaxHeight()
    return typeof cap === 'number' ? Math.min(options.getHeight(), cap) : options.getHeight()
  })

  const v2RowKey = computed(() => {
    return typeof options.getRowKey() === 'string' ? (options.getRowKey() as string) : V2_KEY_FIELD
  })

  // 虚拟数据源：函数 rowKey 时用 defineProperty 在原对象挂不可枚举键字段，
  // 避免浅拷贝新对象（少一次对象创建 + 全部可枚举属性拷贝）；字符串 rowKey 时透传数据，零开销。
  // V2_KEY_FIELD 不可枚举，不影响 JSON.stringify / for...in / Object.keys。
  const v2Data = computed<T[]>(() => {
    const rk = options.getRowKey()
    if (typeof rk === 'string') return options.getData()
    return options.getData().map((row) => {
      const obj = row as Record<string, unknown>
      // 已挂载过则跳过重复计算（Vue computed 依赖未变时不重跑；但数据替换时行引用会变，
      // 此时 defineProperty 是单次 O(1) 操作，比 spread 少 N 次属性拷贝）
      if (V2_KEY_FIELD in obj) return row
      Object.defineProperty(obj, V2_KEY_FIELD, {
        value: String(rk(row)),
        enumerable: false,
        configurable: true,
      })
      return row
    })
  })

  function normFixed(f: ColumnDef<T>['fixed']): true | TableV2FixedDir | undefined {
    if (f === true) return true
    // 值本身与枚举常量一致（'left'/'right'）；用字面量避免把 table-v2 整包静态引回本组件。
    if (f === 'left') return 'left' as TableV2FixedDir
    if (f === 'right') return 'right' as TableV2FixedDir
    return undefined
  }

  /**
   * 把列声明归一化成虚拟滚动所需的固定像素列：
   *   - 虚拟表格不像普通表格支持 min-width 弹性伸缩，必须给确定的 width；
   *     这里优先取显式 width，其次 minWidth，最后 150，并统一钳住 MIN_COLUMN_WIDTH 下限。
   *   - 虚拟表格的 Column 上不传递 minWidth（element-plus 在 fixed=true 时会忽略，但非固定表头时
   *     会再次把 minWidth 注入 style 触发二次布局 → 表头抖动），统一只传 width。
   *   - fixed 列用 TableV2FixedDir.LEFT/RIGHT 映射，避免 true 被误判成 LEFT 以外的方向。
   */
  function resolveV2Width(col: ColumnDef<T>): number {
    return Math.max(MIN_COLUMN_WIDTH, col.width ?? col.minWidth ?? 150)
  }

  /** 虚拟表格单元格渲染：复刻普通表格的默认取值 / 自定义渲染 / 溢出提示 */
  function renderNode(col: ColumnDef<T>, row: T): VNode {
    const raw = col.cell ? col.cell(row) : String(valueOf(row, col.key) ?? '')
    const body: VNode = isVNode(raw) ? raw : h('span', String(raw ?? ''))
    if (col.ellipsis === false) return body
    return h(
      ElTooltip,
      { content: String(valueOf(row, col.key) ?? ''), placement: 'top', showAfter: 300 },
      { default: () => body },
    )
  }

  /**
   * 虚拟表格表头点击/键盘共用同一套排序翻转：未排序→升序，升序→降序（与既有点击行为保持一致）。
   * 翻转规则由主组件统一处理（与普通表格分支共用一份 requestSort）。
   */
  function v2HeaderRenderer(col: ColumnDef<T>) {
    if (!col.sortable) return undefined
    return () => {
      const active = options.getSortKey() === col.key
      const arrowClass = active ? `is-${options.getSortOrder()}` : 'is-none'
      const stateLabel = active ? (options.getSortOrder() === 'desc' ? '降序' : '升序') : '未排序'
      return h('div', {
        class: ['v2-sortable', active && 'is-sorted'],
        // P3 键盘走查：虚拟表头是自绘 div，原生没有任何键盘语义，点击列头排序对
        // 纯键盘用户完全不可达。补 role/tabindex + Enter/Space 后与普通表格对齐。
        role: 'button',
        tabindex: 0,
        'aria-label': `${col.title}，当前${stateLabel}，按 Enter 切换排序`,
        onClick: () => options.requestSort(col),
        onKeydown: (e: KeyboardEvent) => {
          if (e.key !== 'Enter' && e.key !== ' ') return
          e.preventDefault()
          options.requestSort(col)
        },
      }, [
        h('span', col.title),
        h('span', { class: ['v2-sort-arrow', arrowClass], 'aria-hidden': 'true' }, '⇅'),
      ])
    }
  }

  const v2Columns = computed<Column[]>(() => {
    const cols: Column[] = options.getVisibleColumns().map((col) => ({
      key: col.key,
      title: col.title,
      dataKey: col.cell ? undefined : col.key,
      width: resolveV2Width(col),
      align: col.align,
      fixed: normFixed(col.fixed),
      // 不向虚拟表格传 sortable：EP 会据此在表头渲染一个原生排序按钮（表头因此多一个
      // Tab 停靠点），而虚拟表格的排序依赖受控的 sortBy/sortState —— 这两个 prop 本组件
      // 没有传，那个按钮点了不会生效；同时 EP 还会给每列误写 aria-sort="ascending"。
      // 排序统一交给下面的自绘表头（点击/键盘都走 requestSort）。
      headerCellRenderer: v2HeaderRenderer(col),
      cellRenderer: ({ rowData }) => h(options.cellView, { render: () => renderNode(col, rowData as T) }),
    }))
    if (options.getSelectable()) {
      cols.unshift({
        key: '__selection__',
        width: 50,
        align: 'center',
        fixed: 'left' as TableV2FixedDir,
        headerCellRenderer: () =>
          h(
            ElCheckbox,
            {
              modelValue: options.selection.allSelected(),
              indeterminate: options.selection.indeterminate(),
              onChange: options.selection.onHeaderToggle,
            },
            () => h('span', { class: 'sr-only' }, '全选当前页'),
          ),
        cellRenderer: ({ rowData, rowIndex }) =>
          h(
            ElCheckbox,
            {
              modelValue: options.selection.isSelected(rowData as T),
              disabled: !options.selection.canSelect(rowData as T),
              onChange: (v: unknown) => options.selection.onRowToggle(rowData as T, v),
            },
            () => h('span', { class: 'sr-only' }, `选择第 ${rowIndex + 1} 行`),
          ),
      })
    }
    return cols
  })

  return { useVirtual, effectiveHeight, v2RowKey, v2Data, v2Columns }
}
