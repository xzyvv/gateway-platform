import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage, ElInput } from 'element-plus'

/**
 * D1：设备管理 › 设备模板（DeviceTemplates）行为测试。
 *
 * 模板是批量建站的起点：一份模板错了，后续每台新设备都跟着错。
 * 钉住的核心：
 * - 提交契约跟后端字段一致（protocol / default_interval / points.default_address），
 *   空地址不下发 0；
 * - 名称/点位名称为空时本地拦下，不发请求；
 * - 加载失败必须可见报错（否则用户看到的是「没有模板」而不是「加载失败」）；
 * - 删除先确认，取消不发请求；失败不谎报「已删除」；
 * - 「用此模板新建设备」真的跳转到设备页并带参数。
 */

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})
vi.mock('vue-router', () => ({ useRouter: () => ({ push: routerPush }) }))
vi.mock('@/api/templates', () => ({
  listTemplates: (...a: unknown[]) => listMock(...a),
  saveTemplate: (...a: unknown[]) => saveMock(...a),
  deleteTemplate: (...a: unknown[]) => deleteMock(...a),
}))
vi.mock('@/utils/confirm', () => ({ confirmDelete: (...a: unknown[]) => confirmDeleteMock(...a) }))
vi.mock('@/composables/core/useErrorDisplay', () => ({ useErrorDisplay: () => ({ showToast }) }))
vi.mock('@/composables/core/useDialogDirtyGuard', () => ({
  useDialogDirtyGuard: () => ({ beforeClose: () => {}, close: closeGuard }),
}))

const routerPush = vi.hoisted(() => vi.fn())
const listMock = vi.hoisted(() => vi.fn())
const saveMock = vi.hoisted(() => vi.fn())
const deleteMock = vi.hoisted(() => vi.fn())
const confirmDeleteMock = vi.hoisted(() => vi.fn())
const showToast = vi.hoisted(() => vi.fn())
const closeGuard = vi.hoisted(() => vi.fn())

import DeviceTemplates from './DeviceTemplates.vue'
import DataTable from '@/components/DataTable.vue'
import ProtocolSelector from '@/components/industrial/ProtocolSelector.vue'
import type { DeviceTemplate } from '@/api/templates'

const DialogStub = {
  name: 'ElDialog',
  props: ['modelValue', 'title', 'width', 'beforeClose', 'closeOnClickModal'],
  template: '<div class="dialog-stub"><slot /><slot name="footer" /></div>',
}

const TPL: DeviceTemplate[] = [
  {
    name: 'tpl-a', vendor: '', model: '', protocol: 'modbus', default_interval: 1,
    description: '水表模板', bus: '/dev/ttyUSB0',
    points: [
      { name: 'p1', address: '0', default_address: 0, data_type: 'float' },
      { name: 'p2', address: '10', default_address: 10, data_type: 'int16' },
    ],
    createdAt: 1_760_000_000_000,
  } as unknown as DeviceTemplate,
  { name: 'tpl-b', protocol: 'modbus-tcp', points: [], createdAt: null } as unknown as DeviceTemplate,
]

async function settle() {
  await flushPromises()
  await nextTick()
}

function mountPage() {
  return mount(DeviceTemplates, { global: { stubs: { ElDialog: DialogStub } } })
}

function button(wrapper: ReturnType<typeof mountPage>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().trim() === text)
  expect(btn, `找不到按钮「${text}」`).toBeTruthy()
  return btn!
}

function input(wrapper: ReturnType<typeof mountPage>, index: number) {
  return wrapper.findAllComponents(ElInput)[index]!
}

async function openCreate(wrapper: ReturnType<typeof mountPage>) {
  await wrapper.findAll('button').filter((b) => b.text().trim() === '新建模板')[0]!.trigger('click')
  await settle()
}

async function setField(wrapper: ReturnType<typeof mountPage>, index: number, value: string) {
  input(wrapper, index).vm.$emit('update:modelValue', value)
  await settle()
}

/** 点位行内还有 SlaveIdInput 自带的输入框，按行取才不会串位 */
async function setPointField(wrapper: ReturnType<typeof mountPage>, row: number, which: 0 | 1, value: string) {
  wrapper.findAll('.point-row')[row]!.findAllComponents(ElInput)[which]!.vm.$emit('update:modelValue', value)
  await settle()
}

describe('DeviceTemplates 设备模板', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    listMock.mockResolvedValue(TPL.map((t) => ({ ...t, points: [...(t.points ?? [])] })))
    saveMock.mockResolvedValue({ status: 'ok' })
    deleteMock.mockResolvedValue({ status: 'ok' })
    confirmDeleteMock.mockResolvedValue(true)
  })

  it('空列表显示空态，并提供「新建模板」入口', async () => {
    listMock.mockResolvedValue([])
    const wrapper = mountPage()
    await settle()

    expect(wrapper.text()).toContain('暂无模板，新建模板可批量复用设备点位配置')
    expect(wrapper.findAll('button').filter((b) => b.text().trim() === '新建模板').length).toBeGreaterThanOrEqual(2)
  })

  it('列表渲染：点位数与空创建时间按真实值展示', async () => {
    const wrapper = mountPage()
    await settle()

    const table = wrapper.findComponent(DataTable)
    expect((table.props() as unknown as { data: DeviceTemplate[] }).data).toHaveLength(2)
    expect(wrapper.text()).toContain('水表模板')
    // createdAt 缺失时显示 '-'，不能出现 1970 年
    expect(wrapper.findAll('td').some((td) => td.text() === '-')).toBe(true)
    expect(wrapper.text()).toContain('2025/10/9')
  })

  it('加载失败必须可见报错，不能装成「没有模板」', async () => {
    listMock.mockRejectedValue(new Error('tpl 500'))
    const wrapper = mountPage()
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('tpl 500'), '加载设备模板失败')
    expect(wrapper.exists()).toBe(true)
    expect(wrapper.text()).toContain('暂无模板')
  })

  it('新建：名称为空/点位名称为空时本地拦下，不发请求', async () => {
    const wrapper = mountPage()
    await openCreate(wrapper)

    await button(wrapper, '创建').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请输入模板名称')
    expect(saveMock).not.toHaveBeenCalled()

    await setField(wrapper, 0, 'my-tpl')
    await button(wrapper, '创建').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请填写所有点位名称')
    expect(saveMock).not.toHaveBeenCalled()
  })

  it('创建：提交契约带 default_interval / default_address，空地址不下发 0', async () => {
    const wrapper = mountPage()
    await openCreate(wrapper)
    await setField(wrapper, 0, '  水表模板  ')
    await setField(wrapper, 1, '现场水表')
    // 点位行：名称 / 地址
    await setPointField(wrapper, 0, 0, ' 电量 ')
    await setPointField(wrapper, 0, 1, '10')
    await button(wrapper, '新增点位').trigger('click')
    await setPointField(wrapper, 1, 0, '状态')
    await setPointField(wrapper, 1, 1, '')

    await button(wrapper, '创建').trigger('click')
    await vi.waitFor(() => expect(saveMock).toHaveBeenCalledOnce())

    const payload = saveMock.mock.calls[0]![0] as DeviceTemplate
    expect(payload).toMatchObject({
      name: '水表模板',
      vendor: '',
      model: '',
      protocol: 'modbus',
      default_interval: 1,
    })
    expect(payload.points).toHaveLength(2)
    expect(payload.points[0]).toMatchObject({ name: '电量', default_address: 10, address: '10' })
    // 空地址不转成 0，交给后端用模板默认地址
    expect(payload.points[1]!.default_address).toBeUndefined()
    expect(ElMessage.success).toHaveBeenCalledWith('已创建')
    expect(listMock).toHaveBeenCalledTimes(2)
  })

  it('创建失败：可见报错且弹窗不关，用户可改后重试', async () => {
    saveMock.mockRejectedValue(new Error('name exists'))
    const wrapper = mountPage()
    await openCreate(wrapper)
    await setField(wrapper, 0, 'dup')
    await setField(wrapper, 3, 'p1')

    await button(wrapper, '创建').trigger('click')
    await settle()
    expect(showToast).toHaveBeenCalledWith(new Error('name exists'), '创建设备模板失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(wrapper.find('.dialog-stub').exists()).toBe(true)
    expect(closeGuard).not.toHaveBeenCalled()
  })

  it('编辑：预填既有模板并可保存覆盖', async () => {
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '编辑')[0]!.trigger('click')
    await settle()

    expect(input(wrapper, 0).props('modelValue')).toBe('tpl-a')
    expect(input(wrapper, 1).props('modelValue')).toBe('水表模板')
    expect(wrapper.findComponent(ProtocolSelector).props('modelValue')).toBe('modbus')

    await setField(wrapper, 1, '水表模板-改')
    await button(wrapper, '保存').trigger('click')
    await vi.waitFor(() => expect(saveMock).toHaveBeenCalledOnce())

    const payload = saveMock.mock.calls[0]![0] as DeviceTemplate
    expect(payload.name).toBe('tpl-a')
    expect(payload.points).toHaveLength(2)
    expect(ElMessage.success).toHaveBeenCalledWith('已保存')
  })

  it('编辑无点位的模板时自动补一行，避免保存时被「点位名称为空」卡死', async () => {
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '编辑')[1]!.trigger('click')
    await settle()

    expect(wrapper.findAll('.point-row')).toHaveLength(1)
    await button(wrapper, '保存').trigger('click')
    expect(ElMessage.warning).toHaveBeenCalledWith('请填写所有点位名称')
  })

  it('点位行可增删', async () => {
    const wrapper = mountPage()
    await openCreate(wrapper)
    await button(wrapper, '新增点位').trigger('click')
    expect(wrapper.findAll('.point-row')).toHaveLength(2)

    await wrapper.findAll('.point-row')[0]!.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    expect(wrapper.findAll('.point-row')).toHaveLength(1)
  })

  it('删除：取消不发请求；确认后提示并刷新', async () => {
    confirmDeleteMock.mockResolvedValueOnce(false)
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()
    expect(deleteMock).not.toHaveBeenCalled()

    confirmDeleteMock.mockResolvedValue(true)
    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()
    expect(deleteMock).toHaveBeenCalledWith('tpl-a')
    expect(ElMessage.success).toHaveBeenCalledWith('已删除')
    expect(listMock).toHaveBeenCalledTimes(2)
  })

  it('删除失败：提示原因，不谎报已删除', async () => {
    deleteMock.mockRejectedValue(new Error('被引用'))
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '删除')[0]!.trigger('click')
    await settle()

    expect(showToast).toHaveBeenCalledWith(new Error('被引用'), '删除设备模板失败')
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('用此模板新建设备：跳转到设备清单并带模板参数', async () => {
    const wrapper = mountPage()
    await settle()
    await wrapper.findAll('button').filter((b) => b.text().trim() === '用此模板新建设备')[0]!.trigger('click')

    expect(routerPush).toHaveBeenCalledWith({
      path: '/devices',
      query: { tab: 'devices', action: 'create', template: 'tpl-a' },
    })
  })
})
