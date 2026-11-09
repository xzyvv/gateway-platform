import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

const { updatePointMock, notifyMock, confirmMock } = vi.hoisted(() => ({
  updatePointMock: vi.fn(),
  notifyMock: vi.fn(),
  confirmMock: vi.fn(),
}))

vi.mock('@/api/points', () => ({
  updatePoint: updatePointMock,
}))

vi.mock('@/api/config', () => ({
  getConfig: vi.fn(),
}))

vi.mock('@/utils/confirm', () => ({
  confirmAction: confirmMock,
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElNotification: notifyMock,
  }
})

import PointsBatchEditDialog from './PointsBatchEditDialog.vue'
import { ElMessage, ElSelect, ElInputNumber } from 'element-plus'
import { getConfig } from '@/api/config'
import type { PointConfig } from '@/api/types'

/**
 * PointsBatchEditDialog 测试：弹窗只改所选点位，逐条 PUT（worker pool），
 * 结果必须按「全成功 / 部分失败 / 全失败」如实聚合，禁止只在失败时笼统报错。
 */

/** 最小合法点位配置：满足 PointConfig 全部必填字段 */
function makePoint(overrides: Partial<PointConfig> = {}): PointConfig {
  return {
    id: '',
    name: null,
    device_id: null,
    bus: '',
    protocol: 'modbus',
    address: '',
    slave_id: null,
    address_num: null,
    data_id: null,
    function_code: null,
    data_type: null,
    byte_order: null,
    register_count: null,
    interval: 60,
    scale: 1,
    offset: 0,
    formula: null,
    ct_ratio: null,
    pt_ratio: null,
    deadband: 0,
    on_change: false,
    unit: null,
    alarm_high: null,
    alarm_low: null,
    aggregation: null,
    modbus_register: null,
    writable: false,
    ...overrides,
  }
}

/** 把 ElNotification 的 VNode message 摊平成文本，便于断言失败明细 */
function vnodeText(node: unknown): string {
  if (node == null || typeof node === 'boolean') return ''
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(vnodeText).join(' ')
  return vnodeText((node as { children?: unknown }).children)
}

/** 挂载弹窗、勾选「名称」字段并点击「应用到」 */
async function submitWithNameFlag(selection: PointConfig[]) {
  const wrapper = mount(PointsBatchEditDialog, {
    props: { modelValue: true, selection, selectedDevice: 'dev1' },
    attachTo: document.body,
  })
  wrappers.push(wrapper)
  await nextTick()

  const checkboxes = wrapper.findAll('input[type="checkbox"]')
  expect(checkboxes.length).toBeGreaterThan(0)
  await checkboxes[0]!.setValue(true)
  const submitBtn = wrapper.findAll('button').find((b) => b.text().includes('应用到'))
  expect(submitBtn).toBeTruthy()
  await submitBtn!.trigger('click')
  await flushPromises()
  return wrapper
}

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
  document.body.innerHTML = ''
})

describe('PointsBatchEditDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    confirmMock.mockResolvedValue(true)
    vi.mocked(getConfig).mockResolvedValue({ buses: [], points: [] } as never)
  })

  it('存在总线时「总线」渲染为下拉，勾选后所选总线应用到每个点位', async () => {
    vi.mocked(getConfig).mockResolvedValue({
      buses: [{ port_name: '/dev/ttyS1', protocol: 'modbus' }],
      points: [],
    } as never)
    updatePointMock.mockResolvedValue(makePoint({ id: 'ok' }))
    const wrapper = mount(PointsBatchEditDialog, {
      props: { modelValue: true, selection: [makePoint({ id: 'p1', bus: 'old-bus' })], selectedDevice: 'dev1' },
      attachTo: document.body,
    })
    wrappers.push(wrapper)
    await nextTick()
    await flushPromises()

    const busSelect = wrapper.findAllComponents(ElSelect).find((c) => c.props('placeholder') === '选择总线')
    expect(busSelect, '总线应改为下拉').toBeTruthy()
    // 勾选总线字段（名称在最前，总线是第二个勾选框）
    await wrapper.findAll('input[type="checkbox"]')[1]!.setValue(true)
    busSelect!.vm.$emit('update:modelValue', '/dev/ttyS1')
    await nextTick()

    await wrapper.findAll('button').find((b) => b.text().includes('应用到'))!.trigger('click')
    await flushPromises()

    expect(updatePointMock).toHaveBeenCalledTimes(1)
    expect((updatePointMock.mock.calls[0]![1] as PointConfig).bus).toBe('/dev/ttyS1')
  })

  it('勾选 CT/PT 变比后按输入值覆盖到每个点位（不勾选则保持原值）', async () => {
    updatePointMock.mockResolvedValue(makePoint({ id: 'ok' }))
    const wrapper = mount(PointsBatchEditDialog, {
      props: {
        modelValue: true,
        selection: [makePoint({ id: 'p1', ct_ratio: null, pt_ratio: 100 })],
        selectedDevice: 'dev1',
      },
      attachTo: document.body,
    })
    wrappers.push(wrapper)
    await nextTick()
    await flushPromises()

    // 勾选框顺序：名称 0、总线 1、协议 2、从站号 3、CT 4、PT 5
    const checkboxes = wrapper.findAll('input[type="checkbox"]')
    await checkboxes[4]!.setValue(true)
    await checkboxes[5]!.setValue(true)
    const ratioInputs = wrapper
      .findAllComponents(ElInputNumber)
      .filter((c) => c.props('placeholder') === '留空=清空该变比')
    expect(ratioInputs).toHaveLength(2)
    ratioInputs[0]!.vm.$emit('update:modelValue', 30)
    ratioInputs[1]!.vm.$emit('update:modelValue', 100)
    await nextTick()

    await wrapper.findAll('button').find((b) => b.text().includes('应用到'))!.trigger('click')
    await flushPromises()

    expect(updatePointMock).toHaveBeenCalledTimes(1)
    const payload = updatePointMock.mock.calls[0]![1] as PointConfig
    expect(payload.ct_ratio).toBe(30)
    expect(payload.pt_ratio).toBe(100)
  })

  it('勾选数据类型/字节序/寄存器数后一并覆盖到每个点位', async () => {
    updatePointMock.mockResolvedValue(makePoint({ id: 'ok' }))
    const wrapper = mount(PointsBatchEditDialog, {
      props: {
        modelValue: true,
        selection: [makePoint({ id: 'p1', data_type: 'u16', byte_order: 'ABCD', register_count: 1 })],
        selectedDevice: 'dev1',
      },
      attachTo: document.body,
    })
    wrappers.push(wrapper)
    await nextTick()
    await flushPromises()

    // 勾选框顺序：名称 0、总线 1、协议 2、从站号 3、CT 4、PT 5、数据类型 6、字节序 7、寄存器数 8
    const checkboxes = wrapper.findAll('input[type="checkbox"]')
    await checkboxes[6]!.setValue(true)
    await checkboxes[7]!.setValue(true)
    await checkboxes[8]!.setValue(true)

    const decodeSelects = wrapper
      .findAllComponents(ElSelect)
      .filter((c) => c.props('placeholder') === '不修改（留空=清空）')
    expect(decodeSelects).toHaveLength(2)
    decodeSelects[0]!.vm.$emit('update:modelValue', 'f32')
    decodeSelects[1]!.vm.$emit('update:modelValue', 'CDAB')
    const countInput = wrapper
      .findAllComponents(ElInputNumber)
      .find((c) => c.props('placeholder') === '留空=按数据类型自动')
    expect(countInput, '应有寄存器数输入框').toBeTruthy()
    countInput!.vm.$emit('update:modelValue', 2)
    await nextTick()

    await wrapper.findAll('button').find((b) => b.text().includes('应用到'))!.trigger('click')
    await flushPromises()

    expect(updatePointMock).toHaveBeenCalledTimes(1)
    const payload = updatePointMock.mock.calls[0]![1] as PointConfig
    expect(payload.data_type).toBe('f32')
    expect(payload.byte_order).toBe('CDAB')
    expect(payload.register_count).toBe(2)
  })

  it('全部失败时 ElMessage.error 带首条原因，且不 emit edited', async () => {
    updatePointMock.mockRejectedValue(new Error('设备忙'))
    const wrapper = await submitWithNameFlag([makePoint({ id: 'p1', bus: 'bus1', address: '40001' })])

    expect(updatePointMock).toHaveBeenCalledTimes(1)
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('批量修改失败'))
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('设备忙'))
    expect(wrapper.emitted('edited')).toBeUndefined()
    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(notifyMock).not.toHaveBeenCalled()
  })

  it('部分失败时出现常驻 warning 通知，并包含失败点位 id 与原因', async () => {
    updatePointMock.mockImplementation((id: string) =>
      id === 'p2' ? Promise.reject(new Error('寄存器越界')) : Promise.resolve(makePoint({ id })),
    )
    const wrapper = await submitWithNameFlag([
      makePoint({ id: 'p1', name: '温度' }),
      makePoint({ id: 'p2', name: '压力' }),
      makePoint({ id: 'p3', name: '流量' }),
    ])

    expect(updatePointMock).toHaveBeenCalledTimes(3)
    // 部分成功要如实报成功数，不能沉默
    expect(ElMessage.success).toHaveBeenCalledWith(expect.stringContaining('2/3'))
    expect(ElMessage.error).not.toHaveBeenCalled()

    expect(notifyMock).toHaveBeenCalledTimes(1)
    const options = notifyMock.mock.calls[0]![0] as {
      type: string
      duration: number
      title: string
      message: unknown
    }
    expect(options.type).toBe('warning')
    expect(options.duration).toBe(0)
    expect(options.title).toContain('1 个点位未成功')
    // 手机/现场排查需要能直接定位到具体点位，而不是只报「1 个失败」
    const text = vnodeText(options.message)
    expect(text).toContain('p2')
    expect(text).toContain('寄存器越界')
    expect(text).not.toContain('p1')

    // 有成功项，列表需要刷新
    expect(wrapper.emitted('edited')).toEqual([[]])
  })

  it('全部成功时只给出成功提示，不出现「未改动任何点位」这类假信息', async () => {
    updatePointMock.mockResolvedValue(makePoint({ id: 'ok' }))
    const wrapper = await submitWithNameFlag([
      makePoint({ id: 'p1', name: '温度' }),
      makePoint({ id: 'p2', name: '压力' }),
    ])

    expect(updatePointMock).toHaveBeenCalledTimes(2)
    expect(ElMessage.success).toHaveBeenCalledWith('已修改 2 个点位')
    expect(ElMessage.error).not.toHaveBeenCalled()
    expect(notifyMock).not.toHaveBeenCalled()

    const allTexts = [
      ...vi.mocked(ElMessage.success).mock.calls.map((c) => String(c[0])),
      ...vi.mocked(ElMessage.error).mock.calls.map((c) => String(c[0])),
      ...vi.mocked(ElMessage.warning).mock.calls.map((c) => String(c[0])),
    ]
    expect(allTexts.some((t) => t.includes('未改动任何点位'))).toBe(false)
    expect(wrapper.emitted('edited')).toEqual([[]])
  })
})
