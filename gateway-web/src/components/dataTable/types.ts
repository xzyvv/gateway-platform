import type { VNodeChild } from 'vue'

/**
 * 列定义：普通表格与虚拟滚动表格共用同一份描述。
 *   - `key` 同时作为默认取值字段名（等价普通表格的 prop / 虚拟表格的 dataKey）
 *   - `cell` 提供自定义渲染时不再走默认字段取值
 *
 * 该类型对外仍从 `@/components/DataTable` 导出（DataTable.vue 里 re-export），
 * 页面里的 `import type { ColumnDef } from '@/components/DataTable.vue'` 不受影响。
 */
export interface ColumnDef<T = Record<string, unknown>> {
  key: string
  title: string
  width?: number
  minWidth?: number
  fixed?: boolean | 'left' | 'right'
  align?: 'left' | 'center' | 'right'
  sortable?: boolean
  /**
   * 超出列宽省略并悬浮提示（等价普通表格的 show-overflow-tooltip）。
   *
   * 默认 `true`：漏声明一列的代价是超长文本撑破布局且无提示，而显式关闭的
   * 代价只是这一列不截断——前者是静默的布局破坏，后者是可见的局部选择。
   */
  ellipsis?: boolean
  /** ID / 地址 / 数值等需要逐位比对的列：等宽字体 + 不折行，便于肉眼对齐 */
  mono?: boolean
  cell?: (row: T) => VNodeChild
}
