/**
 * 列表分页的边界行为。
 *
 * 这里锁的是「翻页不会翻到空页」这类容易在现场出现、但没人报上来的问题：
 * 过滤后总数变少、每页行数调大、数据刷新，页码都必须被夹回合法范围。
 */
import { describe, it, expect, beforeEach } from 'vitest'
import { computed, nextTick, ref } from 'vue'
import { useTablePager, compareCells } from './useTablePager'

interface Row { id: string; name: string }

function rows(n: number): Row[] {
  return Array.from({ length: n }, (_, i) => ({ id: `p${i}`, name: `点位${i}` }))
}

// 每页行数偏好会写入 localStorage 并在实例间共享（线上真实行为），
// 用例之间必须清空，否则前一个用例调大的档位会改变后一个用例的初始值。
beforeEach(() => localStorage.clear())

describe('useTablePager', () => {
  it('按关键词过滤后再切片，页脚总数是命中数而非原始行数', async () => {
    const source = ref(rows(250))
    const kw = ref('')
    const { matched, total, paged, page, pageSize } = useTablePager(source, (r) => !kw.value || r.id.includes(kw.value))

    expect(total.value).toBe(250)
    expect(pageSize.value).toBe(100)
    expect(paged.value).toHaveLength(100)

    kw.value = 'p1'
    await nextTick()
    expect(matched.value).toHaveLength(111) // p1, p10-p19, p100-p199
    expect(total.value).toBe(111)
    // 关键词变化本身不自动复位页码，由调用方 reset()——这里页码仍是 1
    expect(page.value).toBe(1)
  })

  it('翻到末页后总数变少，页码被夹回合法页而不是停在空页', async () => {
    const source = ref(rows(250))
    const { page, paged, total } = useTablePager(source, () => true)
    page.value = 3 // 250 行 / 100 = 3 页
    expect(paged.value).toHaveLength(50)

    source.value = rows(20)
    await nextTick()
    expect(total.value).toBe(20)
    expect(page.value).toBe(1)
    expect(paged.value).toHaveLength(20)
  })

  it('调大每页行数导致页码越界时同样被夹回', async () => {
    const source = ref(rows(250))
    const { page, pageSize, paged } = useTablePager(source, () => true)
    page.value = 3
    pageSize.value = 500
    await nextTick()
    expect(page.value).toBe(1)
    expect(paged.value).toHaveLength(250)
  })

  it('reset 无条件回到第一页', () => {
    const { page, reset } = useTablePager(ref(rows(500)), () => true)
    page.value = 4
    reset()
    expect(page.value).toBe(1)
  })

  it('空数据时页码保持 1，不会出现 0 页', () => {
    const { pageCount, total, paged } = useTablePager(ref<Row[]>([]), () => true)
    expect(total.value).toBe(0)
    expect(pageCount.value).toBe(1)
    expect(paged.value).toEqual([])
  })

  it('支持以 computed 作为数据源', () => {
    const source = ref(rows(10))
    const view = computed(() => source.value)
    const { total } = useTablePager(view, () => true)
    expect(total.value).toBe(10)
  })
})

/**
 * 排序：排序必须发生在分页之前。
 * 若在页内排序，第 2 页的数据可能整体小于第 1 页，看起来就像"网关数据乱序"，
 * 是最容易被误判成后端问题的排序缺陷，必须锁死。
 */
describe('useTablePager 排序', () => {
  it('排序在全量数据上生效，跨页保持全局有序', async () => {
    // 关键：数据源必须是「乱序」的。若本身就按 p0..p249 有序，
    // 排序后会与原始顺序完全一致，断言就成了恒真的空检查。
    const raw = rows(250).reverse()
    const source = ref(raw)
    const { toggleSort, matched, paged, page } = useTablePager(source, () => true, { pageSize: 100 })

    toggleSort('id')
    await nextTick()

    expect(matched.value).toHaveLength(250)

    // 核心不变量：排序后的全量序列单调不减（含跨页分界处的第 100/101 行）
    const ids = matched.value.map((r) => r.id)
    for (let i = 1; i < ids.length; i++) {
      expect(compareCells(ids[i], ids[i - 1])).toBeGreaterThanOrEqual(0)
    }

    // 排序确实改变了顺序（否则上面的循环是恒真的空断言）
    expect(ids).not.toEqual(raw.map((r) => r.id))
    expect(ids[0]).toBe('p0')
    expect(ids[ids.length - 1]).toBe('p249')

    // 分页结果来自排序后的全量序列，而非页内再排
    page.value = 2
    expect(paged.value[0]!.id).toBe(ids[100])
  })

  it('未排序列时保持数据源原始顺序，不擅自重排', () => {
    const source = ref([{ id: 'c', n: 3 }, { id: 'a', n: 1 }, { id: 'b', n: 2 }])
    const { matched } = useTablePager(source, () => true)
    expect(matched.value.map((r) => r.id)).toEqual(['c', 'a', 'b'])
  })

  it('同列再点一次翻转方向，换列用调用方给的默认方向', async () => {
    const source = ref(rows(5))
    const { toggleSort, matched, sortKey, sortOrder } = useTablePager(source, () => true)

    toggleSort('id')
    expect(sortKey.value).toBe('id')
    expect(sortOrder.value).toBe('asc')

    toggleSort('id')
    expect(sortOrder.value).toBe('desc')
    expect(matched.value.map((r) => r.id)).toEqual(['p4', 'p3', 'p2', 'p1', 'p0'])

    // 换列并指定降序（如"耗时"这类字段升序没意义）
    toggleSort('name', 'desc')
    expect(sortKey.value).toBe('name')
    expect(sortOrder.value).toBe('desc')
  })

  it('排序后自动回到第一页，否则用户会以为排序没生效', () => {
    const source = ref(rows(250))
    const { toggleSort, page } = useTablePager(source, () => true)
    page.value = 3
    toggleSort('id')
    expect(page.value).toBe(1)
  })

  it('空值恒沉底，不因 null 插在中间造成数字忽高忽低', async () => {
    const source = ref([
      { id: 'a', n: 5 },
      { id: 'b', n: null },
      { id: 'c', n: 1 },
    ])
    const { toggleSort, matched } = useTablePager(source, () => true)
    toggleSort('n')
    await nextTick()
    expect(matched.value.map((r) => r.n)).toEqual([1, 5, null])

    toggleSort('n')
    expect(matched.value.map((r) => r.n)).toEqual([5, 1, null])
  })
})

describe('compareCells', () => {
  it('数字按数值比较，不做字符串比较（避免 10 排在 9 前面）', () => {
    expect(compareCells(9, 10)).toBeLessThan(0)
    expect(compareCells('9', '10')).toBeLessThan(0) // numeric: true 时字符串也按数值比
  })

  it('null / undefined / 空串都视作空值并沉底', () => {
    expect(compareCells(null, 1)).toBeGreaterThan(0)
    expect(compareCells(undefined, 1)).toBeGreaterThan(0)
    expect(compareCells('', 1)).toBeGreaterThan(0)
    expect(compareCells(null, undefined)).toBe(0)
  })
})