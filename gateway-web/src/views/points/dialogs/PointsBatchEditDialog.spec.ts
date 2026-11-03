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
import { ElMessage } from 'element-plus'
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
