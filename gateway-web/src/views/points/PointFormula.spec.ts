import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElSelect } from 'element-plus'

/**
 * D1：点位管理 › 虚拟点位（PointFormula）行为测试。
 *
 * 虚拟点位是公式派生量，公式写错会让整条上报数据失真，而「尚未求值」又常被误读成故障。
 * 钉住的核心：
 * - 求值状态分类口径：待采集/等待引用值/求值异常/已停用不能混为一谈；
 * - 公式语法校验直接控制保存按钮，错误公式进不了后端；
 * - 名称/设备缺失在本地提示；
 * - 写接口失败必须可见（useErrorDisplay），不能静默；
 * - 重算结果（有值/报错/未产出）如实反馈。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRoute: () => ({ path: '/points/formula' }) }))
vi.mock('@/utils/confirm', () => ({ confirmDelete: (...args: unknown[]) => confirmDelete(...args) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/composables/core/useDialogDirtyGuard', () => ({
  useDialogDirtyGuard: () => ({ beforeClose: () => {}, close: closeDialog }),
}))
vi.mock('@/api/pointsOps', () => ({
  listVirtualPointsView: (...a: unknown[]) => listVirtualPointsView(...a),
  createVirtualPointView: (...a: unknown[]) => createVirtualPointView(...a),
  updateVirtualPointView: (...a: unknown[]) => updateVirtualPointView(...a),
  deleteVirtualPoint: (...a: unknown[]) => deleteVirtualPoint(...a),
  recalcVirtualPointView: (...a: unknown[]) => recalcVirtualPointView(...a),
}))
vi.mock('@/api/devices', () => ({ listDevices: vi.fn() }))

const confirmDelete = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())
const closeDialog = vi.hoisted(() => vi.fn())
const listVirtualPointsView = vi.hoisted(() => vi.fn())
const createVirtualPointView = vi.hoisted(() => vi.fn())
const updateVirtualPointView = vi.hoisted(() => vi.fn())
const deleteVirtualPoint = vi.hoisted(() => vi.fn())
const recalcVirtualPointView = vi.hoisted(() => vi.fn())

import PointFormula from './PointFormula.vue'
import DataTable from '@/components/DataTable.vue'
import { listDevices } from '@/api/devices'

const DialogStub = {
  name: 'ElDialog',
  props: { modelValue: { type: Boolean, default: false }, title: { type: String, default: '' } },
  template: '<div class="dialog-stub"><span class="dlg-title">{{ title }}</span><slot /><slot name="footer" /></div>',
}

const POINTS = [
  {
    id: 'vp1',
    name: '视在功率',
    formula: '{voltage} * {current}',
    unit: 'VA',
    decimals: 1,
    enabled: true,
    deviceId: 'dev-1',
    lastValue: 220.5,
    error: null,
  },
  {
    id: 'vp2',
    name: '温度',
    formula: '{t}',
    unit: '℃',
    decimals: 1,
    enabled: true,
    deviceId: 'dev-1',
    lastValue: null,
    error: '尚未求值（等待采集数据）',
  },
  {
    id: 'vp3',
    name: '比值',
    formula: '{a} / {b}',
    unit: '',
    decimals: 2,
    enabled: true,
    deviceId: 'dev-1',
    lastValue: null,
    error: '公式求值失败: 除数为 0',
  },
  { id: 'vp4', name: '停用点位', formula: '{x}', unit: '', decimals: 2, enabled: false, deviceId: 'dev-1', lastValue: null, error: null },
  { id: 'vp5', name: '等数据', formula: '{y} + 1', unit: '', decimals: 0, enabled: true, deviceId: 'dev-1', lastValue: null, error: null },
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(PointFormula, {
    global: { stubs: { ElDialog: DialogStub } },
  })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string, index = 0) {
  const list = wrapper.findAll('button').filter((b) => b.text().trim() === text)
  expect(list[index], `找不到第 ${index + 1} 个按钮「${text}」`).toBeTruthy()
  return list[index]!
}

async function openCreate(wrapper: ReturnType<typeof mountPage>) {
  await button(wrapper, '新建虚拟点位').trigger('click')
  await settle()
}

async function fillForm(
  wrapper: ReturnType<typeof mountPage>,
  form: { name?: string; formula?: string; device?: string },
) {
  if (form.name !== undefined) await wrapper.find('input[placeholder="请输入虚拟点位名称"]').setValue(form.name)
  if (form.formula !== undefined) await wrapper.find('textarea').setValue(form.formula)
  if (form.device !== undefined) wrapper.findComponent(ElSelect).vm.$emit('update:modelValue', form.device)
  await settle()
}

describe('PointFormula 虚拟点位', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listVirtualPointsView.mockResolvedValue(POINTS)
    createVirtualPointView.mockResolvedValue(undefined)
    updateVirtualPointView.mockResolvedValue(undefined)
    deleteVirtualPoint.mockResolvedValue(undefined)
    recalcVirtualPointView.mockResolvedValue({ value: null, error: null })
    vi.mocked(listDevices).mockResolvedValue([{ id: 'dev-1', name: '设备一' }] as never)
    confirmDelete.mockResolvedValue(true)
  })

  it('列表加载后按真实求值状态分色：待采集 ≠ 求值异常', async () => {
    const wrapper = mountPage()
    await settle()

    const data = (wrapper.findComponent(DataTable).props() as unknown as { data: unknown[] }).data
    expect(data).toHaveLength(5)
    const text = wrapper.text()
    expect(text).toContain('待采集')
    expect(text).toContain('求值异常')
    expect(text).toContain('已停用')
    // 无值且有 error 时展示引擎原因，而不是「等待采集数据」占位
    expect(text).toContain('公式求值失败: 除数为 0')
    expect(text).toContain('等待采集数据')
    expect(text).toContain('220.5 VA')
  })

  it('列表接口失败不崩页：给出可见错误并复位加载态', async () => {
    listVirtualPointsView.mockRejectedValue(new Error('list 500'))
    const wrapper = mountPage()
    await settle()

    expect(wrapper.exists()).toBe(true)
    const table = wrapper.findAllComponents(DataTable)[0]!
    expect((table.props() as unknown as { loading: boolean }).loading).toBe(false)
    expect(showToast).toHaveBeenCalledWith(new Error('list 500'), '加载虚拟点位失败')
  })

  it('新建弹窗：公式为空时保存禁用，语法错误时给出原因', async () => {
    const wrapper = mountPage()
    await settle()
    await openCreate(wrapper)
    expect(wrapper.text()).toContain('新建虚拟点位')
    expect(button(wrapper, '保存').attributes('disabled')).toBeDefined()

    await fillForm(wrapper, { formula: '{voltage} *' })
    expect(button(wrapper, '保存').attributes('disabled')).toBeDefined()
    const tag = wrapper.findAll('.dialog-stub .el-tag').at(-1)!
    expect(tag.classes()).toContain('el-tag--danger')
  })

  it('合法公式：展示引用点位并允许保存', async () => {
    const wrapper = mountPage()
    await settle()
    await openCreate(wrapper)
    await fillForm(wrapper, { formula: '{voltage} * {current} * 0.85' })

    expect(wrapper.text()).toContain('语法通过')
    expect(wrapper.text()).toContain('voltage')
    expect(wrapper.text()).toContain('current')
    expect(button(wrapper, '保存').attributes('disabled')).toBeUndefined()
  })

  it('保存前本地校验：缺名称 / 缺设备分别提示', async () => {
    const wrapper = mountPage()
    await settle()
    await openCreate(wrapper)

    await fillForm(wrapper, { formula: '{a} + 1' })
    await button(wrapper, '保存').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入名称')

    await fillForm(wrapper, { name: '新点位' })
    await button(wrapper, '保存').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请选择设备')
    expect(createVirtualPointView).not.toHaveBeenCalled()
  })

  it('新建成功：按后端字段契约提交并刷新列表', async () => {
    const wrapper = mountPage()
    await settle()
    await openCreate(wrapper)
    await fillForm(wrapper, { name: '视在功率2', formula: '{voltage} * {current} * 0.85', device: 'dev-1' })
    await button(wrapper, '保存').trigger('click')
    await settle()

    expect(createVirtualPointView).toHaveBeenCalledWith({
      name: '视在功率2',
      device_id: 'dev-1',
      formula: '{voltage} * {current} * 0.85',
      unit: '',
      decimals: 2,
      enabled: true,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    expect(listVirtualPointsView).toHaveBeenCalledTimes(2)
  })

  it('新建失败：写接口错误必须可见，且不再刷新列表', async () => {
    createVirtualPointView.mockRejectedValue(new Error('device_id 不存在'))
    const wrapper = mountPage()
    await settle()
    await openCreate(wrapper)
    await fillForm(wrapper, { name: '视在功率2', formula: '{a} + 1', device: 'dev-1' })
    await button(wrapper, '保存').trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('device_id 不存在'), '新增虚拟点位失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(listVirtualPointsView).toHaveBeenCalledTimes(1)
  })

  it('编辑：回填原值并按 id 提交更新', async () => {
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '编辑', 0).trigger('click')
    await settle()

    expect(wrapper.text()).toContain('编辑虚拟点位')
    const name = wrapper.find('input[placeholder="请输入虚拟点位名称"]').element as HTMLInputElement
    expect(name.value).toBe('视在功率')

    await fillForm(wrapper, { formula: '{voltage} * {current} * 0.9' })
    await button(wrapper, '保存').trigger('click')
    await settle()

    expect(updateVirtualPointView).toHaveBeenCalledWith('vp1', {
      name: '视在功率',
      device_id: 'dev-1',
      formula: '{voltage} * {current} * 0.9',
      unit: 'VA',
      decimals: 1,
      enabled: true,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('已更新')
  })

  it('重算三种结果分别如实提示：有值 / 引擎报错 / 未产出', async () => {
    const wrapper = mountPage()
    await settle()

    recalcVirtualPointView.mockResolvedValueOnce({ value: 220.5, error: null })
    await button(wrapper, '重算', 0).trigger('click')
    await settle()
    expect(ElMessage.success).toHaveBeenCalledWith('重算结果: 220.5')

    recalcVirtualPointView.mockResolvedValueOnce({ value: null, error: '引用点位不可用' })
    await button(wrapper, '重算', 0).trigger('click')
    await settle()
    expect(ElMessage.error).toHaveBeenCalledWith('重算失败: 引用点位不可用')

    recalcVirtualPointView.mockResolvedValueOnce({ value: null, error: null })
    await button(wrapper, '重算', 0).trigger('click')
    await settle()
    expect(ElMessage.warning).toHaveBeenCalledWith('重算未产出数值')
  })

  it('重算请求异常按失败提示，不误报数值', async () => {
    recalcVirtualPointView.mockRejectedValue(new Error('recalc 500'))
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '重算', 0).trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('recalc 500'), '重算虚拟点位失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('删除需二次确认；取消不发请求，确认后才删除', async () => {
    confirmDelete.mockResolvedValueOnce(false)
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '删除', 0).trigger('click')
    await settle()
    expect(confirmDelete).toHaveBeenCalledWith('视在功率')
    expect(deleteVirtualPoint).not.toHaveBeenCalled()

    confirmDelete.mockResolvedValueOnce(true)
    await button(wrapper, '删除', 0).trigger('click')
    await settle()
    expect(deleteVirtualPoint).toHaveBeenCalledWith('vp1')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
  })

  it('删除失败按可见错误处理，不谎报已删除', async () => {
    deleteVirtualPoint.mockRejectedValue(new Error('delete 500'))
    const wrapper = mountPage()
    await settle()
    await button(wrapper, '删除', 0).trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('delete 500'), '删除虚拟点位失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })
})
