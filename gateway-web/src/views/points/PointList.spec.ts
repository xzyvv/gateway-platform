import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick, ref, reactive } from 'vue'
import { ElDropdown } from 'element-plus'
import Points from './PointList.vue'
import PointsCopyDialog from './dialogs/PointsCopyDialog.vue'
import PointsBatchEditDialog from './dialogs/PointsBatchEditDialog.vue'
import PointsGenerateDialog from './dialogs/PointsGenerateDialog.vue'
import { __resetTablePrefsForTests } from '@/composables/core/useTablePrefs'

const mockLoadPoints = vi.fn()
const mockOpenCreate = vi.fn()
const mockOpenEdit = vi.fn()
const mockReadPoint = vi.fn()
const mockRemovePoint = vi.fn()
const mockBatchDelete = vi.fn()
const mockDoImport = vi.fn()
const mockOnSelectionChange = vi.fn()
const mockSavePointField = vi.fn().mockResolvedValue(true)

// 列表加载失败原因：默认空串（无错误）；失败态用例按需置为错误文案
const loadErrorState = ref('')

// 选中行由 mock 的 onSelectionChange 回写，才能驱动批量条出现/消失
const selState = vi.hoisted(() => ({ rows: [] as Array<{ id: string }> }))

// 批量导入进度：由 mock 的 usePoints 返回，测试里按需改成「导入中」
const importState = vi.hoisted(() => ({
  show: false, importing: false, completed: 0, total: 0, running: false,
}))

// useQueryState 依赖 route.query 与 router.currentRoute：筛选条件记忆走 URL
const routerState = vi.hoisted(() => ({
  route: { query: {} as Record<string, string>, path: '/points' },
  replace: vi.fn(),
  push: vi.fn(),
}))

vi.mock('vue-router', () => ({
  useRouter: () => ({ push: routerState.push, replace: routerState.replace, currentRoute: { value: routerState.route } }),
  useRoute: () => routerState.route,
}))

vi.mock('@/composables/devices/usePoints', () => ({
  usePoints: () => {
    const selection = ref(selState.rows)
    return {
    devices: ref([
      { id: 'd1', name: 'dianbiao1', bus: 'bus1', protocol: 'modbus' },
      { id: 'd2', name: 'shuibiao2', bus: 'bus2', protocol: 'dlt645' },
    ]),
    points: ref([
      { id: 'p1', name: 'dianya', bus: 'bus1', protocol: 'modbus', address: '40001', slave_id: 1 },
    ]),
    // 清单表格实际数据源 = 配置点位 + 模板展开点位；本测试只用配置点位
    mergedPoints: ref([
      { id: 'p1', name: 'dianya', bus: 'bus1', protocol: 'modbus', address: '40001', slave_id: 1 },
    ]),
    loading: ref(false),
    // 失败态：默认无错误；需要断言失败分支的用例再改这个 ref
    loadError: loadErrorState,
    selectedDevice: ref('d1'),
    currentDeviceName: ref('dianbiao1'),
    selection,
    pointForm: reactive({ id: '', name: '', bus: '', protocol: 'modbus', address: '', slave_id: 1, address_num: 0 }),
    pointFormRef: ref(null),
    pointRules: {},
    formMode: ref('create' as 'create' | 'edit'),
    submitting: ref(false),
    showForm: ref(false),
    showImport: ref(importState.show),
    importText: ref(''),
    importing: ref(importState.importing),
    importProgress: reactive({
      completed: importState.completed,
      total: importState.total,
      running: importState.running,
    }),
    showCopy: ref(false),
    showBatchEdit: ref(false),
    showGen: ref(false),
    protocolLabel: { modbus: 'Modbus', dlt645: 'DLT645', cjt188: 'CJT188' },
    protocolType: () => 'info',
    protocolOptions: [
      { value: 'modbus', label: 'Modbus' },
      { value: 'dlt645', label: 'DLT645' },
    ],
    onSelectionChange: (rows: Array<{ id: string }>) => {
      selection.value = rows
      mockOnSelectionChange(rows)
    },
    loadDevices: vi.fn(),
    loadPoints: mockLoadPoints,
    readPoint: mockReadPoint,
    removePoint: mockRemovePoint,
    batchDelete: mockBatchDelete,
    doImport: mockDoImport,
    openCreate: mockOpenCreate,
    openEdit: mockOpenEdit,
    resetForm: vi.fn(),
    submitPoint: vi.fn(),
    toggleWritable: vi.fn().mockResolvedValue(true),
    savePointField: mockSavePointField,
    }
  },
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('Points.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    selState.rows = []
    loadErrorState.value = ''
    Object.assign(importState, { show: false, importing: false, completed: 0, total: 0, running: false })
    localStorage.clear()
    // 表格偏好是模块级单例，不重置会让上一条用例的隐藏列/密度泄漏到下一条
    __resetTablePrefsForTests()
    routerState.route = { query: {}, path: '/points' }
  })

  it('renders count badge in toolbar without duplicating the page title', async () => {
    const wrapper = mount(Points)
    await settle()
    const text = wrapper.text()
    // 页面标题由 HubShell 按页签（点位清单）渲染，卡片头只留数量与操作
    expect(text).not.toContain('点位管理')
    expect(wrapper.find('.list-toolbar').exists()).toBe(true)
    expect(text).toContain('1 个')
  })

  it('renders toolbar buttons with clear hierarchy', async () => {
    const wrapper = mount(Points)
    await settle()
    const text = wrapper.text()
    // 主操作直接露出，刷新降为图标按钮（文案挂在 aria-label 上）
    expect(wrapper.find('button[aria-label="刷新"]').exists()).toBe(true)
    expect(text).toContain('新增点位')
    // 批量导入统一收口到「CSV / 粘贴 JSON」双模式页面，入口按钮只负责跳转
    expect(text).toContain('批量导入')
    expect(text).not.toContain('JSON 批量导入')
    expect(text).toContain('更多操作')
    // 低频操作收进"更多"，不再和主操作争夺注意力
    expect(text).not.toContain('批量生成')
    expect(text).not.toContain('复制到其他设备')
  })

  it('展开更多操作后可见复制到其他设备与批量生成', async () => {
    const wrapper = mount(Points, { attachTo: document.body })
    await settle()
    const more = wrapper.findAll('button').find((b) => b.text().includes('更多操作'))
    await more!.trigger('click')
    await settle()
    const bodyText = document.body.textContent || ''
    expect(bodyText).toContain('复制到其他设备')
    expect(bodyText).toContain('批量生成')
    wrapper.unmount()
  })

  /**
   * 防静默失效守护：三个弹窗组件曾因不在 src/components/ 下、又没显式 import，
   * 被 unplugin-vue-components 漏掉，按钮点得动但弹窗永不出现。这里挂真实组件
   * （不再用 stubs 掩盖），断言下拉命令确实把弹窗打开了。
   */
  it('更多操作 → 复制到其他设备，弹窗真正打开', async () => {
    const wrapper = mount(Points)
    await settle()
    wrapper.findComponent(ElDropdown).vm.$emit('command', 'copy')
    await settle()
    const dialog = wrapper.findComponent(PointsCopyDialog)
    expect(dialog.exists()).toBe(true)
    expect(dialog.props('modelValue')).toBe(true)
    expect(wrapper.text()).toContain('复制点位到其他设备')
  })

  it('更多操作 → 批量生成，弹窗真正打开', async () => {
    const wrapper = mount(Points)
    await settle()
    wrapper.findComponent(ElDropdown).vm.$emit('command', 'generate')
    await settle()
    const dialog = wrapper.findComponent(PointsGenerateDialog)
    expect(dialog.exists()).toBe(true)
    expect(dialog.props('modelValue')).toBe(true)
    expect(wrapper.text()).toContain('批量生成点位')
  })

  it('双击名称单元格就地修改并提交，无需打开弹窗', async () => {
    const wrapper = mount(Points)
    await settle()

    const row = wrapper.findAll('.el-table__body tr')[0]!
    // 按内容定位名称列：列顺序/窄屏隐藏列会变，硬编码下标（旧：td[3]）易失效
    const nameCell = row.findAll('td').find((td) => td.text().includes('dianya'))!
    expect(nameCell, '应能定位到名称单元格').toBeTruthy()

    await nameCell.get('.iec-view').trigger('dblclick')
    await nextTick()
    const input = nameCell.get('input')
    await input.setValue('dianya2')
    await input.trigger('keydown', { key: 'Enter' })
    await settle()

    expect(mockSavePointField).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'p1' }),
      { name: 'dianya2' },
    )
    expect(mockOpenEdit).not.toHaveBeenCalled()
  })

  it('行操作只露编辑，读取/删除收进更多菜单', async () => {
    const wrapper = mount(Points, { attachTo: document.body })
    await settle()
    const row = wrapper.findAll('.el-table__body tr')[0]!
    expect(row.text()).toContain('编辑')
    expect(row.text()).toContain('更多')
    // 不再平铺 3 个同权重按钮
    expect(row.text()).not.toContain('删除')

    const more = row.findAll('button').find((b) => b.text().includes('更多'))
    await more!.trigger('click')
    await settle()
    expect(document.body.textContent).toContain('读取当前值')
    expect(document.body.textContent).toContain('删除点位')
    wrapper.unmount()
  })

  it('renders point table with point id and name', async () => {
    const wrapper = mount(Points)
    await settle()
    const text = wrapper.text()
    expect(text).toContain('p1')
    expect(text).toContain('dianya')
  })

  it('clicking refresh button calls loadPoints', async () => {
    const wrapper = mount(Points)
    await settle()
    const btn = wrapper.find('button[aria-label="刷新"]')
    expect(btn.exists()).toBe(true)
    await btn.trigger('click')
    expect(mockLoadPoints).toHaveBeenCalled()
  })

  it('clicking add-point button calls openCreate', async () => {
    const wrapper = mount(Points)
    await settle()
    const btn = wrapper.findAll('button').find((b) => b.text().includes('新增点位'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockOpenCreate).toHaveBeenCalled()
  })

  it('关键词搜索按 ID/名称过滤，无命中时给出明确空态', async () => {
    const wrapper = mount(Points)
    await settle()

    await wrapper.get('.search-input input').setValue('dianya')
    await settle()
    expect(wrapper.text()).toContain('p1')

    await wrapper.get('.search-input input').setValue('不存在的点位')
    await settle()
    // 空态必须说明「是搜索没命中」，而不是笼统的「该设备暂无点位」
    expect(wrapper.text()).toContain('没有匹配「不存在的点位」的点位')

    await wrapper.get('.search-input input').setValue('')
    await settle()
    expect(wrapper.text()).toContain('dianya')
  })

  it('有数据时渲染分页并展示命中总数', async () => {
    const wrapper = mount(Points)
    await settle()

    expect(wrapper.find('.table-pager').exists()).toBe(true)
    expect(wrapper.find('.el-pagination').exists()).toBe(true)
    expect(wrapper.find('.table-pager').text()).toContain('1')
  })

  it('从 URL 还原设备与关键词（刷新/切页后筛选不丢）', async () => {
    routerState.route = { query: { device: 'd2', kw: 'p1' }, path: '/points' }
    const wrapper = mount(Points)
    await settle()

    expect((wrapper.get('.search-input input').element as HTMLInputElement).value).toBe('p1')
    // 设备下拉停在 URL 指定设备，而不是被 composable 的默认选中覆盖
    expect(wrapper.text()).toContain('shuibiao2')
    expect(mockLoadPoints).toHaveBeenCalled()
  })

  it('恢复本地表格偏好：隐藏列不渲染、紧凑密度生效', async () => {
    // 偏好已拆分为两个来源（见 useTablePrefs）：列显隐按表记忆，密度全站共享
    localStorage.setItem('gateway.table-prefs.cols.points', JSON.stringify(['bus']))
    localStorage.setItem('gateway.table-prefs.density', 'compact')
    __resetTablePrefsForTests()
    const wrapper = mount(Points)
    await settle()

    const headers = wrapper.findAll('.el-table__header th').map((th) => th.text())
    expect(headers).toContain('点位 ID')
    expect(headers).not.toContain('总线')
    expect(wrapper.find('.el-table--small').exists()).toBe(true)
  })

  it('表格显示设置提供列显隐与密度切换入口', async () => {
    const wrapper = mount(Points, { attachTo: document.body })
    await settle()
    await wrapper.find('button[aria-label="表格显示设置"]').trigger('click')
    await settle()

    const body = document.body.textContent || ''
    expect(body).toContain('显示列')
    expect(body).toContain('行密度')
    expect(body).toContain('紧凑')
    wrapper.unmount()
  })

  it('搜索框在已选设备时可用', async () => {
    const wrapper = mount(Points)
    await settle()
    expect(wrapper.get('.search-input input').attributes('disabled')).toBeUndefined()
  })

  it('未选中时不渲染批量条，工具栏也不出现灰色禁用的批量按钮', async () => {
    const wrapper = mount(Points)
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(false)
    // 批量动作只在有勾选时才出现，避免常驻 disabled 按钮占位
    expect(wrapper.text()).not.toContain('批量删除')
    expect(wrapper.text()).not.toContain('批量修改')
  })

  it('勾选后出现批量条，显示已选数量与批量动作', async () => {
    const wrapper = mount(Points)
    await settle()

    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()

    const bar = wrapper.find('.selection-bar')
    expect(bar.exists()).toBe(true)
    expect(bar.text()).toContain('已选')
    expect(bar.text()).toContain('1')
    expect(bar.text()).toContain('批量删除')
    expect(bar.text()).toContain('批量修改')
    // 单页时不显示跨页提示
    expect(bar.text()).not.toContain('可跨页勾选')
  })

  it('点击批量删除调用 batchDelete', async () => {
    const wrapper = mount(Points)
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()

    const btn = wrapper.find('.selection-bar').findAll('button').find((b) => b.text().includes('批量删除'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(mockBatchDelete).toHaveBeenCalledTimes(1)
  })

  it('取消选择清空勾选并收起批量条', async () => {
    const wrapper = mount(Points)
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()
    expect(wrapper.find('.selection-bar').exists()).toBe(true)

    const clear = wrapper.find('.selection-bar').findAll('button').find((b) => b.text().includes('取消选择'))
    await clear!.trigger('click')
    await settle()

    expect(wrapper.find('.selection-bar').exists()).toBe(false)
    expect(mockOnSelectionChange).toHaveBeenLastCalledWith([])
  })

  it('「批量导入」跳转到统一的 CSV / JSON 导入页', async () => {
    const wrapper = mount(Points, { attachTo: document.body })
    await settle()

    const btn = wrapper.findAll('button').find((b) => b.text().includes('批量导入'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    expect(routerState.push).toHaveBeenCalledWith('/devices?tab=batch-import')
    wrapper.unmount()
  })

  it('批量修改打开对话框', async () => {
    const wrapper = mount(Points)
    await settle()
    await wrapper.findAll('.el-checkbox__original')[1]!.setValue(true)
    await settle()

    const btn = wrapper.find('.selection-bar').findAll('button').find((b) => b.text().includes('批量修改'))
    await btn!.trigger('click')
    await settle()

    // 真实组件挂载（不再 stub）：打开状态直接体现在 modelValue 上
    const dialog = wrapper.findComponent(PointsBatchEditDialog)
    expect(dialog.exists()).toBe(true)
    expect(dialog.props('modelValue')).toBe(true)
  })
})