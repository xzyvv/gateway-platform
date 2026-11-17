import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
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
    ElMessageBox: { ...actual.ElMessageBox, prompt: vi.fn(), confirm: vi.fn() },
  }
})

import SlaveSimulator from './SlaveSimulator.vue'
import { getRegisters, updateRegister, getSimulatorStatus, startSimulate, type SimRegister } from '@/api/protocolTools'
import { ElMessage, ElMessageBox } from 'element-plus'

/**
 * 寄存器编辑的错误口径。
 *
 * 原实现把「用户点取消」和「PUT 失败」放在同一个 catch 里静默吞掉：取消是正常
 * 操作，失败却会让用户以为改成功了。写操作全局静默后这个问题会更明显，
 * 所以这里分别钉住两条路径：取消不提示、失败必须提示。
 */
const row: SimRegister = { addr: 0, name: '温度', value: 25, unit: '℃', type: 'u16' }

const IDLE = { running: false, slaveId: null, port: 1502, listenAddr: null, requests: 0, errors: 0, startedAtMs: null }
const LIVE = { running: true, slaveId: 1, port: 1502, listenAddr: '0.0.0.0:1502', requests: 3, errors: 0, startedAtMs: 1 }

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
    vi.mocked(getSimulatorStatus).mockResolvedValue(IDLE)
    vi.mocked(ElMessage.error).mockClear()
    vi.mocked(ElMessage.success).mockClear()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('首屏加载失败给出提示（不静默吞掉）', async () => {
    vi.mocked(getRegisters).mockRejectedValue(new Error('网关离线'))

    mount(SlaveSimulator)
    await flushPromises()

    // 原来 onMounted(load) 是裸挂 async 函数：失败只留一个未处理的 rejection，
    // 界面显示「暂无寄存器数据」，现场会以为寄存器表是空的
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('加载从站模拟状态失败'))
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('网关离线'))
  })

  /**
   * 运行中的状态轮询走 usePolling：这里钉住「有轮询」「从站被外部停掉后自动停表」，
   * 后者是手写 setInterval 版本没有的（会一直空转并把界面钉在「运行中」）。
   */
  it('启动后按 2s 轮询状态；从站被外部停掉后停止轮询', async () => {
    vi.useFakeTimers({ toFake: ['setInterval', 'clearInterval'] })
    vi.mocked(ElMessageBox.confirm).mockResolvedValue('confirm' as never)
    vi.mocked(startSimulate).mockResolvedValue(undefined as never)

    const wrapper = mount(SlaveSimulator)
    await flushPromises()

    vi.mocked(getSimulatorStatus).mockResolvedValue(LIVE)
    const startBtn = wrapper.findAll('button').find((b) => b.text().includes('启动模拟'))
    expect(startBtn).toBeTruthy()
    await startBtn!.trigger('click')
    await flushPromises()

    const afterStart = vi.mocked(getSimulatorStatus).mock.calls.length

    vi.advanceTimersByTime(2000)
    await flushPromises()
    expect(vi.mocked(getSimulatorStatus).mock.calls.length).toBe(afterStart + 1)

    // 从站被外部（别的会话 / 网关重启）停掉：下一拍发现 running=false，停表
    vi.mocked(getSimulatorStatus).mockResolvedValue(IDLE)
    vi.advanceTimersByTime(2000)
    await flushPromises()
    const stoppedAt = vi.mocked(getSimulatorStatus).mock.calls.length

    vi.advanceTimersByTime(6000)
    await flushPromises()
    expect(vi.mocked(getSimulatorStatus).mock.calls.length).toBe(stoppedAt)

    wrapper.unmount()
  })

  it('PUT 失败：给出可见错误提示（含原因），且不提示成功', async () => {
    vi.mocked(ElMessageBox.prompt).mockResolvedValue({ value: '30' } as never)
    vi.mocked(updateRegister).mockRejectedValue(new Error('寄存器只读'))

    await mountAndEdit()

    // 只有「修改寄存器 X 失败」不告诉现场为什么失败（只读 / 越界 / 通信超时处置完全不同）
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('修改寄存器「温度」失败'))
    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('寄存器只读'))
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
