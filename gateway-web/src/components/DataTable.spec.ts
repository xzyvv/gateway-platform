import { describe, it, expect, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { defineComponent, h, nextTick, type VNodeChild } from 'vue'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import DataTable, { type ColumnDef } from './DataTable.vue'
import { ElPagination } from 'element-plus'
import { useTableDensity, __resetTablePrefsForTests } from '@/composables/core/useTablePrefs'

interface Row {
  id: string
  name: string
  value: number
}

const columns: ColumnDef<Row>[] = [
  { key: 'id', title: 'ID', width: 120, ellipsis: true },
  { key: 'name', title: '名称', minWidth: 100 },
  { key: 'value', title: '值', width: 80, cell: (row) => h('span', { class: 'cell-value' }, String(row.value * 2)) },
]

function makeRows(n: number): Row[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `r${i + 1}`,
    name: `点位 ${i + 1}`,
    value: i + 1,
  }))
}

// jsdom 没有真实布局：注入固定宽高驱动 el-table-v2，避免 auto-resizer 测不到尺寸
const AutoResizerStub = defineComponent({
  name: 'ElAutoResizer',
  setup(_, { slots }) {
    return () => slots.default?.({ height: 480, width: 800 })
  },
})

const virtualStubs = {
  ElTableV2: true,
  ElAutoResizer: AutoResizerStub,
  'el-table-v2': true,
  'el-auto-resizer': AutoResizerStub,
}

interface MountExtra {
  virtual?: boolean | 'auto'
  selectable?: boolean
  preserveSelection?: boolean
  hiddenColumns?: string[]
  loading?: boolean
  size?: 'large' | 'default' | 'small'
}

function mountTable(rows: Row[], extra: MountExtra = {}) {
  const opts = {
    props: { data: rows, columns, rowKey: 'id', ...extra },
    global: { stubs: virtualStubs },
  }
  // DataTable 是泛型 SFC，VTU 的 mount 不会据 props 反推 T（退化为 unknown），此处断言绕过
  return mount(DataTable, opts as never)
}

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('DataTable 引擎选择', () => {
  it('行数 ≤ 阈值时渲染 el-table 并展示字段与自定义 cell', async () => {
    const wrapper = mountTable(makeRows(3))
    await settle()
    expect(wrapper.find('.el-table').exists()).toBe(true)
    expect(wrapper.find('.virtual-table-wrap').exists()).toBe(false)
    const text = wrapper.text()
    expect(text).toContain('r1')
    expect(text).toContain('点位 1')
    // 自定义 cell：value=1 → 2
    expect(wrapper.find('.cell-value').text()).toBe('2')
  })

  it('行数 > 阈值时自动切换虚拟滚动', async () => {
    const wrapper = mountTable(makeRows(201))
    await settle()
    expect(wrapper.find('.virtual-table-wrap').exists()).toBe(true)
    expect(wrapper.find('.el-table').exists()).toBe(false)
  })

  it('阈值边界精确：200 走普通表，201 才切虚拟（1200 点场景依赖此边界）', async () => {
    // 阈值 VIRTUAL_THRESHOLD=200，改动它会静默影响实时页 1200 点的渲染路径
    const atLimit = mountTable(makeRows(200))
    await settle()
    expect(atLimit.find('.virtual-table-wrap').exists()).toBe(false)
    expect(atLimit.find('.el-table').exists()).toBe(true)

    const overLimit = mountTable(makeRows(201))
    await settle()
    expect(overLimit.find('.virtual-table-wrap').exists()).toBe(true)
  })

  it('运行期行数跨过阈值时动态切换引擎', async () => {
    const rows = makeRows(100)
    const wrapper = mountTable(rows, { selectable: true })
    await settle()
    expect(wrapper.find('.virtual-table-wrap').exists()).toBe(false)

    // 追加到超阈值：应自动切虚拟滚动
    await wrapper.setProps({ data: makeRows(201) })
    await settle()
    expect(wrapper.find('.virtual-table-wrap').exists()).toBe(true)
  })

  it('virtual=true / false 可强制引擎', async () => {
    const forcedVirtual = mountTable(makeRows(3), { virtual: true })
    await settle()
    expect(forcedVirtual.find('.virtual-table-wrap').exists()).toBe(true)

    const forcedPlain = mountTable(makeRows(300), { virtual: false })
    await settle()
    expect(forcedPlain.find('.el-table').exists()).toBe(true)
    expect(forcedPlain.find('.virtual-table-wrap').exists()).toBe(false)
  })
})

describe('DataTable 复选框选择', () => {
  it('勾选单行 emit 整行对象', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable(rows, { selectable: true })
    await settle()
    // DOM 顺序：表头复选框在前，随后为各行
    const boxes = wrapper.findAll('.el-checkbox__original')
    expect(boxes.length).toBe(rows.length + 1)
    await boxes[1]!.setValue(true)
    await settle()

    const emitted = wrapper.emitted('selection-change')!
    expect(emitted.at(-1)![0]).toEqual([rows[0]])
  })

  it('表头全选选中全部 data 并 emit 全部行', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable(rows, { selectable: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[0]!.setValue(true)
    await settle()

    const emitted = wrapper.emitted('selection-change')!
    expect(emitted.at(-1)![0]).toEqual(rows)
  })

  it('data 被整体替换（刷新/切设备）后清空选中', async () => {
    const wrapper = mountTable(makeRows(3), { selectable: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()

    // is-checked / is-indeterminate 落在 .el-checkbox__input 上
    const headerBox = () => wrapper.findAll('.el-checkbox__input')[0]!
    expect(headerBox().classes()).toContain('is-indeterminate')

    await wrapper.setProps({ data: makeRows(3) })
    await settle()
    expect(headerBox().classes()).not.toContain('is-indeterminate')
    expect(headerBox().classes()).not.toContain('is-checked')
  })
})

/**
 * 跨页勾选（配合分页的批量操作）：只渲染当前页时，未渲染页的勾选也必须保留，
 * 否则「第一页勾几条 → 翻页再勾 → 批量删除」会静默丢掉前面选的。
 */
describe('DataTable 跨页勾选', () => {
  it('preserveSelection 下更换当前页数据，已选行仍然保留', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable(rows, { selectable: true, preserveSelection: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()

    // 翻到含 r1 的另一页：r1 的勾选必须还在（行复选框第 2 个）
    await wrapper.setProps({ data: [rows[0]!, rows[2]!] })
    await settle()
    expect(wrapper.findAll('.el-checkbox__input')[1]!.classes()).toContain('is-checked')
    // 当前页没选满 → 表头半选
    expect(wrapper.findAll('.el-checkbox__input')[0]!.classes()).toContain('is-indeterminate')
  })

  it('表头全选只作用于当前页，并保留其他页已勾选的行', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable([rows[0]!], { selectable: true, preserveSelection: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true) // 选中第 1 页的 r1
    await settle()

    await wrapper.setProps({ data: [rows[2]!] })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[0]!.setValue(true) // 第 2 页全选
    await settle()

    const emitted = wrapper.emitted('selection-change')!
    expect(emitted.at(-1)![0]).toEqual([rows[0], rows[2]])
  })

  it('取消表头全选只取消当前页，不影响其他页勾选', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable([rows[0]!, rows[1]!], { selectable: true, preserveSelection: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true) // 选中 r1
    await settle()

    await wrapper.setProps({ data: [rows[1]!, rows[2]!] })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[0]!.setValue(true) // 本页全选 → r1,r2,r3
    await settle()
    await wrapper.findAll('.el-checkbox__original')[0]!.setValue(false) // 取消本页 → 只剩 r1
    await settle()

    const emitted = wrapper.emitted('selection-change')!
    expect(emitted.at(-1)![0]).toEqual([rows[0]])
  })

  it('clearSelection 清空全部勾选并同步抛出空数组', async () => {
    const rows = makeRows(3)
    const wrapper = mountTable(rows, { selectable: true, preserveSelection: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[0]!.setValue(true)
    await settle()

    ;(wrapper.vm as unknown as { clearSelection: () => void }).clearSelection()
    await settle()

    const emitted = wrapper.emitted('selection-change')!
    expect(emitted.at(-1)![0]).toEqual([])
    expect(wrapper.findAll('.el-checkbox__input')[0]!.classes()).not.toContain('is-indeterminate')
  })
})

describe('DataTable 骨架屏', () => {
  it('首次加载且无数据时用表格骨架替代转圈', async () => {
    const wrapper = mountTable([], { loading: true })
    await settle()

    expect(wrapper.find('.table-skeleton').exists()).toBe(true)
    // 骨架期间不渲染空表格 + 蒙层，避免"先看到暂无数据再闪出内容"
    expect(wrapper.find('.el-table').exists()).toBe(false)
    // 列数与真实列一致：3 列 ×（表头 1 行 + 数据 8 行）
    expect(wrapper.findAll('.sk-cell')).toHaveLength(27)
  })

  it('已有数据时刷新保留表格，走轻量蒙层而非整块替换', async () => {
    const wrapper = mountTable(makeRows(2), { loading: true })
    await settle()

    expect(wrapper.find('.table-skeleton').exists()).toBe(false)
    expect(wrapper.find('.el-table').exists()).toBe(true)
  })
})

describe('DataTable 列显隐', () => {
  it('hiddenColumns 中的列既不渲染表头也不渲染单元格', async () => {
    const wrapper = mountTable(makeRows(2), { hiddenColumns: ['name'] })
    await settle()
    const headers = wrapper.findAll('.el-table__header th').map((th) => th.text())
    expect(headers).toContain('ID')
    expect(headers).not.toContain('名称')
    // 数据层面也一并消失，避免"列头没了但值还在"的错位
    expect(wrapper.text()).not.toContain('点位 1')
    expect(wrapper.text()).toContain('r1')
  })

  it('hiddenColumns 为空时全列渲染（不误伤默认行为）', async () => {
    const wrapper = mountTable(makeRows(2), { hiddenColumns: [] })
    await settle()
    const headers = wrapper.findAll('.el-table__header th').map((th) => th.text())
    expect(headers).toEqual(['ID', '名称', '值'])
  })
})

/**
 * 排序：组件只抛 sort-change，不自己排序。
 * 它拿到的是"当前页"数据，页内排序会让第 2 页看起来比第 1 页小，
 * 用户会当成网关数据乱序报障——所以排序必须由使用方在全量上做。
 */
describe('DataTable 排序事件', () => {
  const sortCols: ColumnDef<Row>[] = [
    { key: 'id', title: 'ID', width: 120, sortable: true },
    { key: 'value', title: '值', width: 80, sortable: true },
    { key: 'name', title: '名称', minWidth: 100 },
  ]

  it('点击可排序列的表头抛出 sort-change，未标记列不参与', async () => {
    const wrapper = mount(DataTable, {
      props: { data: makeRows(3), columns: sortCols, rowKey: 'id', virtual: false },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    const headers = wrapper.findAll('.el-table__header th')
    expect(headers).toHaveLength(3)
    // 只有 sortable 列才有排序标记
    expect(headers[0]!.classes().join(' ')).toContain('sortable')
    expect(headers[2]!.classes().join(' ')).not.toContain('sortable')

    await headers[1]!.trigger('click')
    await settle()
    expect(wrapper.emitted('sort-change')).toBeTruthy()
    const events = wrapper.emitted('sort-change') as { key: string; order: string }[][]
    expect(events[0]![0]!.key).toBe('value')
  })

  it('受控排序列的表头同步 aria-sort，未排序列不输出', async () => {
    const wrapper = mount(DataTable, {
      props: { data: makeRows(3), columns: sortCols, rowKey: 'id', virtual: false, sortKey: 'value', sortOrder: 'desc' },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    const headers = wrapper.findAll('.el-table__header th')
    const byTitle = (t: string) => headers.find((hd) => hd.text().includes(t))
    expect(byTitle('值')?.attributes('aria-sort')).toBe('descending')
    expect(byTitle('ID')?.attributes('aria-sort')).toBeUndefined()
    expect(byTitle('名称')?.attributes('aria-sort')).toBeUndefined()
  })

  it('虚拟滚动模式下排序标记由 sortKey/sortOrder 驱动渲染在 v2 表头', async () => {
    // el-table-v2 在 jsdom 下用真组件会因缺布局而不渲染表头，
    // 这里用一个"只调用 headerCellRenderer"的最小 stub 验证列定义正确性
    const headerRenderers: (() => VNodeChild)[] = []
    const TableV2Probe = defineComponent({
      name: 'ElTableV2',
      props: ['columns'],
      setup(p) {
        return () => {
          if (!headerRenderers.length) {
            for (const c of p.columns as { headerCellRenderer?: () => VNodeChild }[]) {
              if (c.headerCellRenderer) headerRenderers.push(c.headerCellRenderer)
            }
          }
          return h('div')
        }
      },
    })

    mount(DataTable, {
      props: {
        data: makeRows(3),
        columns: sortCols,
        rowKey: 'id',
        virtual: true,
        sortKey: 'value',
        sortOrder: 'desc',
      },
      global: { stubs: { ...virtualStubs, ElTableV2: TableV2Probe, 'el-table-v2': TableV2Probe } },
    } as never)
    await settle()

    // 两个 sortable 列各有一个表头渲染器，非 sortable 列没有
    expect(headerRenderers).toHaveLength(2)

    // 把表头渲染结果真正挂载出来再断言 DOM，而不是硬啃 VNode 结构
    const probe = mount(defineComponent({
      setup() {
        return () => h('div', headerRenderers.map((r) => h('div', { class: 'hd' }, [r()])))
      },
    }))
    const sortableHeads = probe.findAll('.v2-sortable')
    expect(sortableHeads).toHaveLength(2)

    const sortedHeads = probe.findAll('.v2-sortable.is-sorted')
    expect(sortedHeads).toHaveLength(1)
    expect(sortedHeads[0]!.text()).toContain('值')
    expect(sortedHeads[0]!.find('.v2-sort-arrow').classes()).toContain('is-desc')
  })

  it('虚拟滚动模式的排序列头可用键盘操作（P3 键盘走查）', async () => {
    const headerRenderers: (() => VNodeChild)[] = []
    const TableV2Probe = defineComponent({
      name: 'ElTableV2',
      props: ['columns'],
      setup(p) {
        return () => {
          if (!headerRenderers.length) {
            for (const c of p.columns as { headerCellRenderer?: () => VNodeChild }[]) {
              if (c.headerCellRenderer) headerRenderers.push(c.headerCellRenderer)
            }
          }
          return h('div')
        }
      },
    })

    const wrapper = mount(DataTable, {
      props: { data: makeRows(3), columns: sortCols, rowKey: 'id', virtual: true },
      global: { stubs: { ...virtualStubs, ElTableV2: TableV2Probe, 'el-table-v2': TableV2Probe } },
    } as never)
    await settle()

    const probe = mount(defineComponent({
      setup() {
        return () => h('div', headerRenderers.map((r) => h('div', { class: 'hd' }, [r()])))
      },
    }))
    const heads = probe.findAll('.v2-sortable')
    // id 列未排序：应能 Tab 到、读屏读出状态、Enter 触发升序
    const idHead = heads[0]!
    expect(idHead.attributes('role')).toBe('button')
    expect(idHead.attributes('tabindex')).toBe('0')
    expect(idHead.attributes('aria-label')).toContain('ID')
    expect(idHead.attributes('aria-label')).toContain('未排序')
    expect(idHead.find('.v2-sort-arrow').attributes('aria-hidden')).toBe('true')

    await idHead.trigger('keydown', { key: 'Enter' })
    expect((wrapper.emitted('sort-change') as { key: string; order: string }[][])[0]![0]).toEqual({
      key: 'id',
      order: 'asc',
    })

    // 空格键与 Enter 等价（role="button" 的标准按键约定），且不冒泡成页面滚动
    await idHead.trigger('keydown', { key: ' ' })
    expect(wrapper.emitted('sort-change')).toHaveLength(2)

    // 其他按键不触发排序
    await idHead.trigger('keydown', { key: 'a' })
    expect(wrapper.emitted('sort-change')).toHaveLength(2)
  })
})

/**
 * 行高跟随全站密度偏好。
 * 密度是跨表格的同一件事，若某页写死 size="small"，用户在别处切的紧凑就到这里失效。
 */
describe('DataTable 密度跟随全局偏好', () => {
  beforeEach(() => {
    localStorage.clear()
    __resetTablePrefsForTests()
  })

  it('未显式传 size 时跟随全局密度', async () => {
    const wrapper = mountTable(makeRows(2))
    await settle()
    expect(wrapper.find('.el-table--small').exists()).toBe(false)

    useTableDensity().setDensity('compact')
    await settle()
    expect(wrapper.find('.el-table--small').exists()).toBe(true)
  })

  it('显式传入的 size 优先于全局偏好', async () => {
    const wrapper = mountTable(makeRows(2), { size: 'small' })
    await settle()
    expect(wrapper.find('.el-table--small').exists()).toBe(true)

    // 页面显式声明标准时，全局紧凑不应覆盖它（否则页面布局会被改坏）
    const wrapper2 = mountTable(makeRows(2), { size: 'default' })
    await settle()
    expect(wrapper2.find('.el-table--small').exists()).toBe(false)
  })
})

/**
 * 单元格内容由使用方的 `cell` 函数产出，必须继承使用方组件的 scopeId，
 * 否则使用方 scoped 样式（.value-cell / .comms-cell 等）匹配不到。
 */
describe('DataTable 单元格 scopeId 继承', () => {
  it('cell 渲染的节点携带使用方组件的 scopeId', async () => {
    const Host = Object.assign(
      defineComponent({
        setup() {
          return () => h(DataTable as never, { data: makeRows(1), columns, rowKey: 'id', virtual: false })
        },
      }),
      { __scopeId: 'data-v-host' },
    )
    const wrapper = mount(Host, { global: { stubs: virtualStubs } })
    await settle()
    const cell = wrapper.find('.cell-value')
    expect(cell.exists()).toBe(true)
    expect(cell.attributes('data-v-host')).toBe('')
  })
})

/**
 * 分页下沉：页脚由表格自己渲染，页面只接 useTablePager 的状态。
 * total 缺席/为 0 时不渲染——空表下面再挂一条"共 0 条"是噪音。
 */
describe('DataTable 分页下沉', () => {
  it('传 total 时渲染页脚，翻页与改每页条数向上抛出', async () => {
    const wrapper = mount(DataTable, {
      props: {
        data: makeRows(2),
        columns,
        rowKey: 'id',
        virtual: false,
        page: 1,
        pageSize: 50,
        total: 120,
      },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    expect(wrapper.find('.table-pager').exists()).toBe(true)
    const pager = wrapper.findComponent(ElPagination)
    pager.vm.$emit('update:current-page', 2)
    pager.vm.$emit('update:page-size', 100)
    await settle()

    expect(wrapper.emitted('update:page')![0]![0]).toBe(2)
    expect(wrapper.emitted('update:pageSize')![0]![0]).toBe(100)
  })

  it('total 为 0 或未传时不渲染页脚', async () => {
    const zero = mount(DataTable, {
      props: { data: [], columns, rowKey: 'id', virtual: false, total: 0 },
      global: { stubs: virtualStubs },
    } as never)
    await settle()
    expect(zero.find('.table-pager').exists()).toBe(false)

    // 不分页的表（如弹窗里的小表）不传 total，也不能凭空出现页脚
    const noTotal = mountTable(makeRows(2))
    await settle()
    expect(noTotal.find('.table-pager').exists()).toBe(false)
  })
})

/**
 * 批量操作下沉：勾选态由表格维护，页面只提供「对选中项能做什么」。
 * 没提供动作插槽的表格即使开了复选框也不该冒出空操作条。
 */
describe('DataTable 批量操作条', () => {
  it('选中行且提供 batch-actions 时出现批量条，取消选择后消失', async () => {
    const Host = defineComponent({
      setup() {
        return () =>
          h(
            DataTable as never,
            { data: makeRows(3), columns, rowKey: 'id', selectable: true, virtual: false },
            { 'batch-actions': () => h('button', { class: 'bulk-delete' }, '批量删除') },
          )
      },
    })
    const wrapper = mount(Host, { global: { stubs: virtualStubs } })
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(false)

    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(true)
    expect(wrapper.find('.bulk-delete').exists()).toBe(true)

    await wrapper.find('.sb-clear').trigger('click')
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(false)
  })

  it('只开复选框但没给 batch-actions 插槽时不渲染批量条', async () => {
    const wrapper = mountTable(makeRows(3), { selectable: true })
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(false)
  })
})

/** 空态统一：描述说明"为什么空"，actionText 给出下一步，点击抛 empty-action 由页面处理 */
describe('DataTable 空态行动', () => {
  it('渲染语境描述与行动按钮，点击抛出 empty-action', async () => {
    const wrapper = mount(DataTable, {
      props: {
        data: [],
        columns,
        rowKey: 'id',
        virtual: false,
        emptyText: '没有匹配「abc」的点位',
        emptyActionText: '清空搜索',
      },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    expect(wrapper.find('.el-empty__description').text()).toContain('没有匹配「abc」的点位')
    const action = wrapper.findAll('button').find((b) => b.text() === '清空搜索')
    expect(action).toBeTruthy()
    await action!.trigger('click')
    expect(wrapper.emitted('empty-action')).toBeTruthy()
  })
})

/**
 * v2 语义兜底：el-table-v2 全程 div 渲染且不带 role，读屏不再是表格。
 * DataTable 按结构补 role（table/rowgroup/row/columnheader/cell），
 * 并用 MutationObserver 兜住虚拟滚动动态补行。这里用模拟 v2 DOM 骨架的
 * stub 驱动补丁逻辑（真 v2 组件在 jsdom 下因缺布局不渲染）。
 */
describe('DataTable v2 表格语义兜底', () => {
  const TableV2Skeleton = defineComponent({
    name: 'ElTableV2',
    props: ['data'],
    setup(p) {
      return () => h('div', { class: 'el-table-v2' }, [
        h('div', { class: 'el-table-v2__header' }, [
          h('div', { class: 'el-table-v2__header-row' }, [
            h('div', { class: 'el-table-v2__header-cell' }, 'ID'),
            h('div', { class: 'el-table-v2__header-cell' }, '名称'),
          ]),
        ]),
        h('div', { class: 'el-table-v2__body' }, (p.data as Row[]).map((r) =>
          h('div', { class: 'el-table-v2__row', key: r.id }, [
            h('div', { class: 'el-table-v2__row-cell' }, r.id),
            h('div', { class: 'el-table-v2__row-cell' }, r.name),
          ]),
        )),
      ])
    },
  })

  function mountV2(rows: Row[]) {
    return mount(DataTable, {
      props: { data: rows, columns, rowKey: 'id', virtual: true },
      global: { stubs: { ...virtualStubs, ElTableV2: TableV2Skeleton, 'el-table-v2': TableV2Skeleton } },
    } as never)
  }

  it('挂载后为 v2 DOM 补 role：table/rowgroup/row/columnheader/cell + aria-rowcount', async () => {
    const wrapper = mountV2(makeRows(3))
    await settle()
    await nextTick() // syncV2Aria 挂在 onMounted 的双 nextTick 之后，再补一拍

    const table = wrapper.find('.el-table-v2')
    expect(table.attributes('role')).toBe('table')
    expect(table.attributes('aria-rowcount')).toBe('4') // 3 数据行 + 1 表头行
    expect(wrapper.find('.el-table-v2__header').attributes('role')).toBe('rowgroup')
    expect(wrapper.find('.el-table-v2__body').attributes('role')).toBe('rowgroup')
    expect(wrapper.find('.el-table-v2__header-row').attributes('role')).toBe('row')
    const headerCells = wrapper.findAll('.el-table-v2__header-cell')
    expect(headerCells.length).toBeGreaterThan(0)
    expect(headerCells.every((c) => c.attributes('role') === 'columnheader')).toBe(true)
    const rows = wrapper.findAll('.el-table-v2__row')
    expect(rows.length).toBeGreaterThan(0)
    expect(rows.every((r) => r.attributes('role') === 'row')).toBe(true)
    const cells = wrapper.findAll('.el-table-v2__row-cell')
    expect(cells.length).toBeGreaterThan(0)
    expect(cells.every((c) => c.attributes('role') === 'cell')).toBe(true)
  })

  it('MutationObserver 兜底：虚拟滚动动态补进的行也会被补上 role', async () => {
    const wrapper = mountV2(makeRows(2))
    await settle()
    await nextTick()

    // 模拟滚动后 Grid 补挂新行：不走 Vue 更新（不触发 onUpdated），只动 DOM
    const body = wrapper.element.querySelector('.el-table-v2__body')!
    const extra = document.createElement('div')
    extra.className = 'el-table-v2__row'
    body.appendChild(extra)
    await new Promise((resolve) => setTimeout(resolve, 0)) // 等观察器微任务
    expect(extra.getAttribute('role')).toBe('row')
    wrapper.unmount()
  })

  it('数据整体替换后 aria-rowcount 跟随更新', async () => {
    const wrapper = mountV2(makeRows(2))
    await settle()
    await nextTick()
    expect(wrapper.find('.el-table-v2').attributes('aria-rowcount')).toBe('3')

    await wrapper.setProps({ data: makeRows(9) })
    await settle()
    await nextTick()
    expect(wrapper.find('.el-table-v2').attributes('aria-rowcount')).toBe('10')
    wrapper.unmount()
  })
})

/**
 * 表格无障碍名称（无障碍 #4b）。
 *
 * 表格若没有 <caption>，读屏器只会播报「表格」，用户无从判断这张表
 * 装的是设备还是点位。el-table 内部渲染两张 <table>（fixed 布局时 header/body
 * 分离），Element Plus 没有 caption 插槽，只能在 DOM 里补。
 */
describe('DataTable 表格无障碍名称', () => {
  it('普通表：给渲染出的 table 补 <caption class="sr-only">', async () => {
    const wrapper = mountTable(makeRows(3))
    await settle()

    const captions = wrapper.findAll('table caption')
    expect(captions.length).toBeGreaterThan(0)
    // caption 必须视觉隐藏但保留在无障碍树里
    expect(captions[0]!.classes()).toContain('sr-only')
    // 兜底文案含列名，用户能听出这张表装什么
    expect(captions[0]!.text()).toContain('ID')
    expect(captions[0]!.text()).toContain('名称')
  })

  it('显式传 caption 时优先使用传入值', async () => {
    const wrapper = mount(DataTable, {
      props: { data: makeRows(2), columns, rowKey: 'id', virtual: false, caption: '设备清单表' },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    const caption = wrapper.find('table caption')
    expect(caption.exists()).toBe(true)
    expect(caption.text()).toBe('设备清单表')
  })

  it('caption 是 table 的第一个子元素（HTML 规范要求）', async () => {
    const wrapper = mountTable(makeRows(2))
    await settle()

    const table = wrapper.find('table')
    expect(table.element.firstElementChild?.tagName).toBe('CAPTION')
  })

  it('列显隐变化后 caption 跟随更新', async () => {
    const wrapper = mountTable(makeRows(2), { hiddenColumns: ['name'] })
    await settle()
    expect(wrapper.find('table caption').text()).not.toContain('名称')

    await wrapper.setProps({ hiddenColumns: [] })
    await settle()
    expect(wrapper.find('table caption').text()).toContain('名称')
  })

  it('虚拟滚动表：v2 无 caption 语义，改用 aria-label 标注表格名称', async () => {
    const TableV2Skeleton = defineComponent({
      name: 'ElTableV2',
      props: ['data'],
      setup(p) {
        return () => h('div', { class: 'el-table-v2' }, [
          h('div', { class: 'el-table-v2__header' }, [
            h('div', { class: 'el-table-v2__header-row' }, [
              h('div', { class: 'el-table-v2__header-cell' }, 'ID'),
            ]),
          ]),
          h('div', { class: 'el-table-v2__body' }, (p.data as Row[]).map((r) =>
            h('div', { class: 'el-table-v2__row', key: r.id }, [
              h('div', { class: 'el-table-v2__row-cell' }, r.id),
            ]),
          )),
        ])
      },
    })

    const wrapper = mount(DataTable, {
      props: { data: makeRows(3), columns, rowKey: 'id', virtual: true, caption: '点位清单表' },
      global: { stubs: { ...virtualStubs, ElTableV2: TableV2Skeleton, 'el-table-v2': TableV2Skeleton } },
    } as never)
    await settle()
    await nextTick()

    const table = wrapper.find('.el-table-v2')
    expect(table.attributes('role')).toBe('table')
    expect(table.attributes('aria-label')).toBe('点位清单表')
    wrapper.unmount()
  })

  it('无列定义时兜底为「数据表格」（不输出空 caption）', async () => {
    const wrapper = mount(DataTable, {
      props: { data: makeRows(1), columns: [], rowKey: 'id', virtual: false },
      global: { stubs: virtualStubs },
    } as never)
    await settle()

    const caption = wrapper.find('table caption')
    expect(caption.exists()).toBe(true)
    expect(caption.text()).toBe('数据表格')
  })
})

/**
 * 防重复实现守护：表格虚拟滚动只允许在 DataTable 内落地。
 * 白名单为「待迁移」页面的临时放行，迁移完成后必须同步删除条目（下方会校验腐化）。
 */
describe('DataTable 单一实现守护', () => {
  const here = dirname(fileURLToPath(import.meta.url))
  const srcRoot = resolve(here, '..')

  function walkVue(dir: string, out: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      let st
      try {
        st = statSync(p)
      } catch {
        continue
      }
      if (st.isDirectory()) walkVue(p, out)
      else if (name.endsWith('.vue')) out.push(p)
    }
    return out
  }

  const ALLOWLIST = ['src/components/DataTable.vue']

  it('除白名单外禁止直接使用 el-table-v2（统一走 DataTable）', () => {
    const allow = new Set(ALLOWLIST)
    const offenders: string[] = []
    const used = new Set<string>()
    for (const f of walkVue(srcRoot)) {
      const rel = `src/${relative(srcRoot, f).split('\\').join('/')}`
      if (!readFileSync(f, 'utf-8').includes('el-table-v2')) continue
      used.add(rel)
      if (!allow.has(rel)) offenders.push(rel)
    }
    expect(offenders, `以下文件绕过 DataTable 直接使用 el-table-v2：\n${offenders.join('\n')}`).toEqual([])

    // 白名单必须与实际使用一致：迁移完成后若未清理条目，在此报错（防白名单腐化）
    const stale = ALLOWLIST.filter((a) => !used.has(a))
    expect(stale, `白名单中以下文件已不再使用 el-table-v2，请删除条目：\n${stale.join('\n')}`).toEqual([])
  })

  it('除白名单外禁止直接使用原生 el-table / el-table-column / el-pagination', () => {
    // #7 迁移完成后不再允许新增原生表：固定表头、骨架、空态、分页、批量条都只在 DataTable 内实现，
    // 任何绕过都会让同一页面出现两种表格观感（并且大概率漏掉骨架/空态）。
    const allow = new Set(ALLOWLIST)
    const offenders: string[] = []
    const rawTable = /<el-table(?!-v2)[\s>]|<el-table-column[\s>]|<el-pagination[\s>]/
    for (const f of walkVue(srcRoot)) {
      const rel = `src/${relative(srcRoot, f).split('\\').join('/')}`
      if (allow.has(rel)) continue
      if (rawTable.test(readFileSync(f, 'utf-8'))) offenders.push(rel)
    }
    expect(offenders, `以下文件绕过 DataTable 直接使用原生表格/分页：\n${offenders.join('\n')}`).toEqual([])
  })
})

/**
 * EP 按需样式完整性守护。
 *
 * unplugin-vue-components 的 ElementPlusResolver 只在模板里出现 `resolveComponent()` 调用时
 * 才会注入对应 style/css。一旦某组件在 <script setup> 里被静态 import，模板标签就被编译器
 * 解析成 setup 绑定，不再产生该调用 —— 样式从此永不注入，而且没有任何报错，
 * 只在页面上表现为「布局散架」（分页器竖排堆高 270px、徽标角标跑到框外）。
 *
 * 判据：对全站每个静态 import 的 EP 组件，只要存在「使用它但没静态 import 它」的某个文件，
 * 该文件就走 resolver 注入，样式在全站可用；否则必须有文件显式 import 样式入口。
 */
describe('EP 按需样式完整性守护', () => {
  const here = dirname(fileURLToPath(import.meta.url))
  const srcRoot = resolve(here, '..')

  function walkVue(dir: string, out: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      let st
      try {
        st = statSync(p)
      } catch {
        continue
      }
      if (st.isDirectory()) walkVue(p, out)
      else if (name.endsWith('.vue')) out.push(p)
    }
    return out
  }

  /** ElPagination → el-pagination（保留 el- 前缀，与模板标签一致） */
  const kebab = (name: string) => name.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase()

  /** 服务式 API：不是模板标签，EP 在运行时自行注入样式，不参与本守护 */
  const NON_TEMPLATE = new Set(['ElMessage', 'ElMessageBox', 'ElNotification', 'ElLoading'])

  /**
   * 传递性注入：el-table/style/css 会顺带 import tooltip / checkbox / scrollbar 的样式，
   * el-select 会带 option / tag / popper 等。父组件必须是走 resolver 的，
   * 这样父组件哪天也变成全站静态 import 时，这里的白名单会自然失效并报错。
   */
  const TRANSITIVE_FROM: Record<string, string> = {
    'el-checkbox': 'el-table',
    'el-tooltip': 'el-table',
  }

  it('静态 import 的 EP 组件都有样式入口（resolver 或显式 style/css）', () => {
    const files = walkVue(srcRoot)
    const codes = new Map<string, string>()
    for (const f of files) {
      codes.set(`src/${relative(srcRoot, f).split('\\').join('/')}`, readFileSync(f, 'utf-8'))
    }

    // 静态 import 的 EP 组件 → 出现在哪些文件
    const importedIn = new Map<string, Set<string>>()
    for (const [rel, code] of codes) {
      for (const m of code.matchAll(/import\s*\{([^}]+)\}\s*from\s*['"]element-plus['"]/g)) {
        for (const raw of m[1]!.split(',')) {
          const name = raw.trim().replace(/^type\s+/, '')
          if (!/^El[A-Z]/.test(name) || NON_TEMPLATE.has(name)) continue
          if (!importedIn.has(name)) importedIn.set(name, new Set())
          importedIn.get(name)!.add(rel)
        }
      }
    }

    // 走 resolver 的组件：模板里用了但该文件没静态 import 它
    const resolverUsed = new Set<string>()
    for (const code of codes.values()) {
      const staticHere = new Set<string>()
      for (const m of code.matchAll(/import\s*\{([^}]+)\}\s*from\s*['"]element-plus['"]/g)) {
        for (const raw of m[1]!.split(',')) staticHere.add(raw.trim().replace(/^type\s+/, ''))
      }
      for (const name of importedIn.keys()) {
        if (staticHere.has(name)) continue
        if (new RegExp(`<${kebab(name)}[\\s/>]`).test(code)) resolverUsed.add(name)
      }
    }

    // 显式注入样式入口的文件与组件目录
    const styleInjectedDirs = new Set<string>()
    for (const code of codes.values()) {
      for (const m of code.matchAll(/element-plus\/es\/components\/([a-z-]+)\/style\/css/g)) {
        styleInjectedDirs.add(m[1]!)
      }
    }

    const dirOf = (name: string) => kebab(name).replace(/^el-/, '')

    const offenders: string[] = []
    for (const [name, files] of importedIn) {
      if (resolverUsed.has(name)) continue
      if (styleInjectedDirs.has(dirOf(name))) continue
      const parent = TRANSITIVE_FROM[kebab(name)]
      if (parent) {
        // el-table → ElTable
        const parentName = parent.split('-').map((s) => s.charAt(0).toUpperCase() + s.slice(1)).join('')
        if (resolverUsed.has(parentName)) continue
      }
      offenders.push(`${name}（${[...files].join(', ')}）`)
    }

    expect(
      offenders,
      `以下 EP 组件的按需样式不会被注入，页面上会静默错位。请在相应文件里补 import 'element-plus/es/components/<dir>/style/css'：\n${offenders.join('\n')}`,
    ).toEqual([])
  })
})

/**
 * 模板字符串 ref 绑定完整性守护。
 *
 * `<el-form ref="formRef">` 里写的是字符串 ref，Vue 只会在当前渲染上下文里按名字去找
 * `setup()` 返回的同名绑定。如果 <script setup> 里没声明（典型场景：从 composable
 * 解构时漏掉一项），ref 就绑不到任何东西，`formRef.value` 永远是 undefined。
 * 这类缺陷不会报错、不会警告，表现为「点保存按钮毫无反应」。
 */
describe('模板 ref 绑定完整性守护', () => {
  const here = dirname(fileURLToPath(import.meta.url))
  const srcRoot = resolve(here, '..')

  function walkVue(dir: string, out: string[] = []): string[] {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name)
      let st
      try {
        st = statSync(p)
      } catch {
        continue
      }
      if (st.isDirectory()) walkVue(p, out)
      else if (name.endsWith('.vue')) out.push(p)
    }
    return out
  }

  it('模板里的字符串 ref 都能在 script 中找到同名绑定', () => {
    const offenders: string[] = []
    for (const file of walkVue(srcRoot)) {
      const code = readFileSync(file, 'utf-8')
      const tplStart = code.indexOf('<template>')
      if (tplStart < 0) continue
      const tplEnd = code.lastIndexOf('</template>')
      const tpl = tplEnd > tplStart ? code.slice(tplStart, tplEnd) : code
      const script = code.slice(tplEnd)

      // 只认静态字符串 ref；:ref / v-bind:ref 是表达式，由编译器自行校验
      for (const m of tpl.matchAll(/(?<![:@\w-])ref="([A-Za-z_$][\w$]*)"/g)) {
        const name = m[1]!
        const bound = new RegExp(`\\b${name}\\b`).test(script)
        if (!bound) {
          const rel = `src/${relative(srcRoot, file).split('\\').join('/')}`
          offenders.push(`${rel} —— 模板 ref="${name}" 在 script 中没有同名绑定`)
        }
      }
    }
    expect(
      offenders,
      `以下文件的字符串 ref 绑不到任何东西，相关操作会静默失效（先从 composable 解构里把它补上）：\n${offenders.join('\n')}`,
    ).toEqual([])
  })
})