import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { createPinia } from 'pinia'
import { ElMessage, ElSelect } from 'element-plus'
import type { DeviceConfig, PointConfig } from '@/api/types'

vi.mock('@/api/points', () => ({
  listPoints: vi.fn(),
  createPoint: vi.fn(),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

import PointsCopyDialog from './PointsCopyDialog.vue'
import { listPoints, createPoint } from '@/api/points'

/**
 * 复制点位对话框的错误口径。
 *
 * 背景：写操作（POST createPoint）失败已被全局拦截器静默，若对话框只做
 * `created++ / 静默跳过`，用户会看到「复制完成：成功 0 个」这种看似成功的提示。
 * 这里钉住两件事：部分失败必须降级为 warning 并给出失败数；整体失败必须报错。
 */
const devices = [
  { id: 'src', name: '源设备' },
  { id: 'dst', name: '目标设备' },
] as unknown as DeviceConfig[]

function makePoint(id: string, address: string): PointConfig {
  return {
    id,
    name: id,
    device_id: 'src',
    bus: 'bus1',
    protocol: 'modbus',
    address,
    slave_id: 1,
    address_num: 1,
    data_id: null,
    function_code: 3,
    data_type: 'u16',
    byte_order: 'big',
    register_count: 1,
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
  }
}

async function mountDialog() {
  return mount(PointsCopyDialog, {
    props: {
      modelValue: true,
      devices,
      selectedDevice: 'dst',
      selectedDeviceName: '目标设备',
      points: [],
    },
    global: { plugins: [createPinia()] },
  })
}

/** 选源设备 + 点「开始复制」（对话框内部状态未 defineExpose，只能走 UI） */
async function startCopy(wrapper: Awaited<ReturnType<typeof mountDialog>>) {
  wrapper.findComponent(ElSelect).vm.$emit('update:modelValue', 'src')
  await flushPromises()
  const btn = wrapper.findAll('button').find((b) => b.text().includes('开始复制'))
  expect(btn).toBeTruthy()
  await btn!.trigger('click')
  await flushPromises()
}

describe('PointsCopyDialog 失败提示', () => {
  beforeEach(() => {
    vi.mocked(ElMessage.success).mockClear()
    vi.mocked(ElMessage.error).mockClear()
    vi.mocked(ElMessage.warning).mockClear()
  })

  it('全部创建失败：降级为 warning 并给出失败数，不能提示成功', async () => {
    vi.mocked(listPoints).mockResolvedValue([makePoint('p1', '40001'), makePoint('p2', '40002')])
    vi.mocked(createPoint).mockRejectedValue(new Error('500'))

    const wrapper = await mountDialog()
    await startCopy(wrapper)

    expect(ElMessage.success).not.toHaveBeenCalled()
    expect(ElMessage.warning).toHaveBeenCalledWith(expect.stringContaining('失败 2 个'))
  })

  it('读取源设备点位失败：给出动作语境的错误提示', async () => {
    vi.mocked(listPoints).mockRejectedValue(new Error('502'))

    const wrapper = await mountDialog()
    await startCopy(wrapper)

    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('复制点位失败'))
  })
})
