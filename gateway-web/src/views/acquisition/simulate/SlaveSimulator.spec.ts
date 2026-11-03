import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('@/api/protocolTools', () => ({
  getRegisters: vi.fn(),
  updateRegister: vi.fn(),
  startSimulate: vi.fn(),
  stopSimulate: vi.fn(),
  getSimulatorStatus: vi.fn(),
}))
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
    ElMessageBox: { prompt: vi.fn() },
  }
})

import SlaveSimulator from './SlaveSimulator.vue'
import { getRegisters, updateRegister, getSimulatorStatus, type SimRegister } from '@/api/protocolTools'
import { ElMessage, ElMessageBox } from 'element-plus'

/**
 * 寄存器编辑的错误口径。
 *
 * 原实现把「用户点取消」和「PUT 失败」放在同一个 catch 里静默吞掉：取消是正常
 * 操作，失败却会让用户以为改成功了。写操作全局静默后这个问题会更明显，
 * 所以这里分别钉住两条路径：取消不提示、失败必须提示。
 */
const row: SimRegister = { addr: 0, name: '温度', value: 25, unit: '℃', type: 'u16' }

async function mountAndEdit() {
  const wrapper = mount(SlaveSimulator)
  await flushPromises() // onMounted(load)
  const btn = wrapper.findAll('button').find((b) => b.text() === '修改')
  expect(btn).toBeTruthy()
  await btn!.trigger('click')
  await flushPromises()
}

describe('SlaveSimulator 寄存器编辑', () => {
  beforeEach(() => {
    vi.mocked(getRegisters).mockResolvedValue([row])
    vi.mocked(getSimulatorStatus).mockResolvedValue({ running: false, slaveId: null, port: 1502, listenAddr: null, requests: 0, errors: 0, startedAtMs: null })
    vi.mocked(ElMessage.error).mockClear()
    vi.mocked(ElMessage.success).mockClear()
  })

  it('PUT 失败：给出可见错误提示，且不提示成功', async () => {
    vi.mocked(ElMessageBox.prompt).mockResolvedValue({ value: '30' } as never)
    vi.mocked(updateRegister).mockRejectedValue(new Error('500'))

    await mountAndEdit()

    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('修改寄存器 温度 失败'))
    expect(ElMessage.success).not.toHaveBeenCalledWith(expect.stringContaining('已更新'))
  })

  it('用户在输入框点取消：不弹任何错误', async () => {
    vi.mocked(ElMessageBox.prompt).mockRejectedValue('cancel')

    const wrapper = mount(SlaveSimulator)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text() === '修改')
    await btn!.trigger('click')
    await flushPromises()

    expect(ElMessage.error).not.toHaveBeenCalled()
  })
})
