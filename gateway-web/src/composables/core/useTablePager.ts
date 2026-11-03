/**
 * 列表页「关键词筛选 + 分页」的共用逻辑。
 *
 * 使用场景：点位管理、设备管理这类单次可达上千行的列表。
 * 之前整表交给 DataTable 虚拟滚动，行是流畅了，但用户无法快速定位某一条，
 * 也无法「本页全选再批量处理」。这里把「过滤 → 计总数 → 切片」收敛成一份，
 * 避免两个页面各写一遍后越界、页码不复位等细节漂移。
 *
 * 约定：调用方在关键词变化时调用 reset()，在数据源整体刷新时调用 reset()
 * 并清空表格勾选（DataTable 提供 clearSelection）。
 */
import { computed, ref, watch } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'
import { useQueryState } from './useQueryState'

/**
 * 每页行数偏好：全站共享一份，跨页面、跨会话保留。
 *
 * 与表格密度（useTablePrefs）同理——用户把点位表调到 500 行，表达的是
 * "这个系统的表每页 500 行"，换到设备列表或审计日志不该退回默认值。
 * 存储沿用网关命名空间隔离：切换网关后 A 网关的偏好不会污染 B 网关。
 *
 * 只持久化每页行数，不持久化页码：页码与数据源强相关，
 * 数据变少后停在第 N 页会看到空页，那属于"记忆了错误的状态"。
 */
const PAGE_SIZE_KEY = 'gateway.table-prefs.page-size'

/**
 * 页脚可选档位：全站统一。
 * 此前点位/设备是 [50,100,200,500]、审计日志是 [20,50,100]，两套档位并存，
 * 用户切换页面时既看不到自己习惯的档位，也无法复用同一个偏好值。
 */
export const DEFAULT_PAGE_SIZES = [20, 50, 100, 200, 500]

function loadPageSize(pageSizes: number[], fallback: number): number {
  const raw = scopedGet(PAGE_SIZE_KEY)
  if (!raw) return fallback
  const n = Number(raw)
  // 只认当前表页脚里真实存在的档位：存储值可能来自旧版本或别的档位集合，
  // 直接塞进 el-pagination 会显示一个下拉里选不中的数字
  return Number.isFinite(n) && pageSizes.includes(n) ? n : fallback
}

function savePageSize(v: number): void {
  try {
    scopedSet(PAGE_SIZE_KEY, String(v))
  } catch { /* 隐私模式写入失败：本次会话内仍生效，只是下次进入丢失 */ }
}

export interface TablePagerOptions {
  /** 初始每页行数，默认 100；若存在已保存的偏好则优先用偏好值 */
  pageSize?: number
  /** 页脚可选的每页行数，默认 DEFAULT_PAGE_SIZES */
  pageSizes?: number[]
  /** 初始排序列 key（配合 sortable 列） */
  sortKey?: string
  /** 初始排序方向 */
  sortOrder?: SortOrder
  /**
   * 把「页码 / 每页行数 / 排序」同步到 URL query（默认关闭）。
   *
   * 打开后刷新页面、复制链接、浏览器前进后退都能还原到同一屏，
   * 远程支持时可以直接让现场把出问题的那一页链接发过来。
   * 筛选条件仍由各页自己的 useQueryState 管理，互不干扰。
   */
  syncUrl?: boolean
}

export type SortOrder = 'asc' | 'desc'

export interface TablePager<T> {
  page: Ref<number>
  pageSize: Ref<number>
  pageSizes: number[]
  /** 命中关键词的全部行（已排序） */
  matched: ComputedRef<T[]>
  total: ComputedRef<number>
  pageCount: ComputedRef<number>
  /** 当前页要渲染的行 */
  paged: ComputedRef<T[]>
  /** 回到第一页（关键词变化 / 数据源刷新时调用） */
  reset: () => void
  /** 当前排序列 key，未排序为 null */
  sortKey: Ref<string | null>
  /** 当前排序方向 */
  sortOrder: Ref<SortOrder>
  /** 切换排序列：同列点击则在升/降序间切换，换列则按传入的默认方向 */
  toggleSort: (key: string, defaultOrder?: SortOrder) => void
}

/**
 * 空值判定：null / undefined / 空串都算"没有值"。
 * 表里「尚未采集」「无通信统计」的点位就是这种状态，排序时应统一沉底。
 */
function isEmptyCell(v: unknown): boolean {
  return v === null || v === undefined || v === ''
}

/** 从 URL 还原页码：非法值一律回到第 1 页 */
function toPageNumber(raw: string): number {
  const n = Math.floor(Number(raw))
  return Number.isFinite(n) && n >= 1 ? n : 1
}

/** 排序列在 URL 中编码为 `key:asc|desc`；解析失败视为未排序 */
function parseSortParam(raw: string): { key: string; order: SortOrder } | null {
  if (!raw) return null
  const [key, order] = raw.split(':')
  if (!key) return null
  return { key, order: order === 'desc' ? 'desc' : 'asc' }
}

/**
 * 单元格取值的比较器（方向无关）。
 *
 * 表格列可能是纯字段（el-table 原生排序）、也可能是 cell 渲染出来的复合值
 * （如「12ms / 98%」这种把两个指标拼在一起的通信质量列）。这里对空值
 * 一律沉底，避免未采集的点位插到正常值中间造成"数字忽高忽低"的错觉。
 *
 * 注意：空值沉底的方向由调用方保证（见 toggleSort 里的 dir 计算），
 * 若把 dir 乘进整个比较结果，降序时空值会被翻到最前面，恰好与意图相反。
 */
export function compareCells(a: unknown, b: unknown): number {
  const aEmpty = isEmptyCell(a)
  const bEmpty = isEmptyCell(b)
  if (aEmpty && bEmpty) return 0
  if (aEmpty) return 1
  if (bEmpty) return -1
  if (typeof a === 'number' && typeof b === 'number') return a - b
  return String(a).localeCompare(String(b), 'zh-CN', { numeric: true })
}

export function useTablePager<T>(
  source: Ref<T[]> | ComputedRef<T[]> | (() => T[]),
  match: (row: T) => boolean,
  options: TablePagerOptions = {},
): TablePager<T> {
  const pageSizes = options.pageSizes ?? DEFAULT_PAGE_SIZES
  const defaultPageSize = options.pageSize ?? 100
  // syncUrl 是静态配置，同一组件实例内取值不变，条件调用 composable 是安全的
  const qPage = options.syncUrl ? useQueryState('page', '1') : null
  const qSize = options.syncUrl ? useQueryState('size') : null
  const qSort = options.syncUrl ? useQueryState('sort') : null
  const initialSort = qSort ? parseSortParam(qSort.value) : null
  const urlPageSize = qSize ? Number(qSize.value) : NaN

  const page = ref(qPage ? toPageNumber(qPage.value) : 1)
  const pageSize = ref(
    Number.isFinite(urlPageSize) && pageSizes.includes(urlPageSize)
      ? urlPageSize
      : loadPageSize(pageSizes, defaultPageSize),
  )
  // 用户改一次档位，全站下次进入都沿用（含当前页刷新）
  watch(pageSize, savePageSize)
  const sortKey = ref<string | null>(initialSort?.key ?? options.sortKey ?? null)
  const sortOrder = ref<SortOrder>(initialSort?.order ?? options.sortOrder ?? 'asc')

  if (qPage) {
    // 本地状态 → URL（默认值不写入，保持链接干净），URL → 本地状态供前进/后退还原
    watch(page, (v) => { qPage.value = v > 1 ? String(v) : '' })
    watch(qPage, (v) => {
      const n = toPageNumber(v)
      if (n !== page.value) page.value = n
    })
  }
  if (qSize) {
    watch(pageSize, (v) => { qSize.value = v === defaultPageSize ? '' : String(v) })
    watch(qSize, (v) => {
      const n = Number(v)
      if (Number.isFinite(n) && pageSizes.includes(n) && n !== pageSize.value) pageSize.value = n
    })
  }
  if (qSort) {
    watch([sortKey, sortOrder], ([k, o]) => { qSort.value = k ? `${k}:${o}` : '' })
    watch(qSort, (v) => {
      const parsed = parseSortParam(v)
      const key = parsed?.key ?? options.sortKey ?? null
      const order = parsed?.order ?? options.sortOrder ?? 'asc'
      if (key !== sortKey.value) sortKey.value = key
      if (order !== sortOrder.value) sortOrder.value = order
    })
  }

  const read = (): T[] => (typeof source === 'function' ? source() : source.value)

  // match 内部读取关键词 ref，computed 会自动建立依赖，关键词一变就重算
  const filtered = computed(() => read().filter(match))

  // 排序必须发生在分页之前：组件只拿到当前页数据，在页内排序会让"第 2 页比第 1 页小"
  // 这种跨页错乱看起来像数据本身有问题
  const matched = computed(() => {
    const key = sortKey.value
    if (!key) return filtered.value
    const dir = sortOrder.value === 'asc' ? 1 : -1
    return [...filtered.value].sort((a, b) => {
      const av = (a as Record<string, unknown>)[key]
      const bv = (b as Record<string, unknown>)[key]
      // 空值恒沉底：不乘 dir，否则降序时「尚未采集」的点位会被顶到最前面
      if (isEmptyCell(av) || isEmptyCell(bv)) return compareCells(av, bv)
      return dir * compareCells(av, bv)
    })
  })

  const total = computed(() => matched.value.length)
  const pageCount = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)))
  const paged = computed(() =>
    matched.value.slice((page.value - 1) * pageSize.value, page.value * pageSize.value),
  )

  // 过滤后总数变少、或用户把每页行数调大，页码都可能越界到空页，必须夹住
  watch([total, pageSize], () => {
    if (page.value < 1) page.value = 1
    else if (page.value > pageCount.value) page.value = pageCount.value
  })

  function reset() {
    page.value = 1
  }

  /**
   * 切换排序列。同一列再点一次翻转方向；换列时用调用方给的默认方向
   * （列自己知道"耗时"这类字段升序没意义，默认降序更符合直觉）。
   */
  function toggleSort(key: string, defaultOrder: SortOrder = 'asc') {
    if (sortKey.value === key) {
      sortOrder.value = sortOrder.value === 'asc' ? 'desc' : 'asc'
    } else {
      sortKey.value = key
      sortOrder.value = defaultOrder
    }
    // 排序变了仍停在第 5 页的话，用户会以为"排序没生效"
    page.value = 1
  }

  return {
    page, pageSize, pageSizes, matched, total, pageCount, paged, reset,
    sortKey, sortOrder, toggleSort,
  }
}