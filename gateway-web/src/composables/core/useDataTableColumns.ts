import { computed } from 'vue'
import type { ColumnDef } from '@/components/dataTable/types'

/**
 * 读取行对象里某个字段的值（列 key 即字段名）。
 *
 * 普通表格的单元格、虚拟滚动表格的单元格渲染共用这一份取值逻辑，
 * 避免两处各写一遍 `row[key]`（一处写成 `row?.key` 就会与另一处不一致）。
 */
export function valueOf<T>(row: T, key: string): unknown {
  return (row as Record<string, unknown>)[key]
}

export interface DataTableColumnsOptions<T> {
  /** 页面声明的全部列 */
  getColumns: () => ColumnDef<T>[]
  /** 需要隐藏的列 key（页面把「列显隐」偏好传进来，组件本身不持久化） */
  getHiddenColumns: () => string[]
  /** 使用方显式传入的表格无障碍名称；为空时按列名自动拼装 */
  getCaption: () => string
  /** 是否启用复选框列（影响骨架屏列数） */
  getSelectable: () => boolean
}

/**
 * 表格的列派生逻辑：可见列、表格无障碍名称、骨架屏列数。
 *
 * 输入：列定义 / 隐藏列 / 显式 caption / 是否可选
 * 输出：visibleColumns（渲染用列）、tableCaption（读屏播报名）、skeletonColumns（骨架列数）
 * 异常：无（纯计算，列定义非法也不会抛错，最多渲染出空表格）
 *
 * 说明：列显隐只做「按 key 过滤」，不做列顺序重排——顺序由页面自己决定。
 */
export function useDataTableColumns<T>(options: DataTableColumnsOptions<T>) {
  const visibleColumns = computed(() => {
    const hidden = new Set(options.getHiddenColumns() ?? [])
    const columns = options.getColumns()
    return hidden.size === 0 ? columns : columns.filter((c) => !hidden.has(c.key))
  })

  /**
   * 表格的无障碍名称（读屏器播报的标题）。
   *
   * 优先用使用方显式传入的 caption；未传时按可见列名自动拼装兜底，
   * 保证「每张表都有可访问名称」这条底线——表格若没有名称，读屏器只会
   * 播报「表格」，用户无法判断这是设备表还是点位表。
   *
   * 自动兜底的措辞刻意贴近列名（如「设备 ID、设备名、通信质量…共 9 列」），
   * 信息量低但绝不误导；页面若要更好的名称，显式传 caption 即可。
   */
  const tableCaption = computed(() => {
    if (options.getCaption()) return options.getCaption()
    const titles = visibleColumns.value.map((c) => c.title).filter(Boolean)
    if (!titles.length) return '数据表格'
    // 列名过多时截断，避免读屏器播报一长串列名
    const head = titles.slice(0, 5).join('、')
    const rest = titles.length - 5
    return rest > 0 ? `${head}…等共 ${titles.length} 列` : `${head}，共 ${titles.length} 列`
  })

  /** 骨架屏列数：可见列 +（启用时）复选框列 */
  const skeletonColumns = computed(() => visibleColumns.value.length + (options.getSelectable() ? 1 : 0))

  return { visibleColumns, tableCaption, skeletonColumns }
}
