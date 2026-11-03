import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'

const { scanBusMock } = vi.hoisted(() => ({
  scanBusMock: vi.fn(),
}))

vi.mock('@/api/protocolTools/slaveScan', () => ({
  scanBus: scanBusMock,
  respondedSlaves: (rows: Array<{ responded: boolean }>) => (rows ?? []).filter((r) => r.responded),
}))

vi.mock('@/api/devices', () => ({
  listDevices: vi.fn(async () => []),
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

import DeviceDiscoverDialog from './DeviceDiscoverDialog.vue'
import { ElMessage } from 'element-plus'

/** DeviceDiscoverDialog 测试：设备扫描（POST）失败必须给出动作语境提示 */

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
  document.body.innerHTML = ''
})

describe('DeviceDiscoverDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('扫描失败时 ElMessage.error 含「设备扫描失败」，且不 emit add', async () => {
    scanBusMock.mockRejectedValueOnce(new Error('500'))
    const wrapper = mount(DeviceDiscoverDialog, {
      props: {
        modelValue: true,
        protocolOptions: [{ value: 'modbus', label: 'Modbus RTU' }],
        devices: [],
      },
      attachTo: document.body,
    })
    wrappers.push(wrapper)
    await nextTick()

    // 总线为空时按钮 disabled，先填入总线名
    await wrapper.find('input[placeholder="如 bus1"]').setValue('bus1')
    const scanBtn = wrapper.findAll('button').find((b) => b.text().includes('开始扫描'))
    expect(scanBtn).toBeTruthy()
    await scanBtn!.trigger('click')
    await flushPromises()

    expect(scanBusMock).toHaveBeenCalledWith('bus1', 1, 247)
    // 写操作全局静默，组件必须自己给出带动作语境的可见提示
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('设备扫描失败'))
    // 失败不得假装扫描出设备
    expect(wrapper.emitted('add')).toBeUndefined()
  })
})
