// BatchCommand.vue 冒烟测试：批量读写命令是核心写操作页（向 4 步向导下发指令）。
// mock 三个数据源（设备/点位/即时读写），验证挂载加载与向导骨架。
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import BatchCommand from './DeviceBatchCommand.vue'

// vi.hoisted：vi.mock 工厂被提升到文件顶部执行，工厂内直接引用顶层 const 会 TDZ 报错，
// 必须用 vi.hoisted 让 mock 函数与 vi.mock 一起提升。
const { readPointNow, writePoints } = vi.hoisted(() => ({
  readPointNow: vi.fn(async () => ({ status: 'ok', value: 20 })),
  writePoints: vi.fn(async (writes: { id: string; value: number }[]) => ({
    status: 'ok',
    total: writes.length,
    succeeded: writes.length,
    failed: 0,
    results: writes.map((w) => ({ point_id: w.id, status: 'ok', value: w.value })),
  })),
}))

vi.mock('@/api/devices', () => ({
  listDevices: vi.fn(async () => [
    { id: 'dev_1', name: '电表1' },
    { id: 'dev_2', name: '温控器2' },
  ]),
}))

vi.mock('@/api/points', () => ({
  listPoints: vi.fn(async (deviceId: string) => [{ id: `${deviceId}_p1`, name: '温度' }]),
  readPointNow,
  writePoints,
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

async function settle() {
  await flushPromises()
  await nextTick()
}

describe('BatchCommand.vue 冒烟', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('挂载渲染向导，下一步后显示已加载的待选点位', async () => {
    const wrapper = mount(BatchCommand)
    await settle()

    // step 0：选择操作
    expect(wrapper.text()).toContain('选择操作')
    expect(wrapper.text()).toContain('批量读取')

    // step 0 → step 1：待选列表（每设备前 5 个点位）
    const nextBtn = wrapper.findAll('button').find((b) => b.text().includes('下一步'))
    expect(nextBtn, '未找到「下一步」按钮').toBeTruthy()
    await nextBtn!.trigger('click')
    await settle()

    expect(wrapper.text()).toContain('温度')
    expect(wrapper.text()).toContain('电表1')
    wrapper.unmount()
  })

  it('listPoints 按设备逐个调用', async () => {
    const { listPoints } = await import('@/api/points')
    mount(BatchCommand)
    await settle()

    expect(listPoints).toHaveBeenCalledWith('dev_1')
    expect(listPoints).toHaveBeenCalledWith('dev_2')
  })

  it('批量读取走真实即时读取接口', async () => {
    const wrapper = mount(BatchCommand)
    await settle()

    // 选操作 → 选目标（全部加入）→ 确认 → 执行
    const nextBtns = wrapper.findAll('button').filter((b) => b.text().includes('下一步'))
    await nextBtns[0]!.trigger('click')
    await settle()
    const addAllBtn = wrapper.findAll('button').find((b) => b.text().includes('全部加入'))
    expect(addAllBtn).toBeTruthy()
    await addAllBtn!.trigger('click')
    await settle()
    const nextBtns2 = wrapper.findAll('button').filter((b) => b.text().includes('下一步'))
    await nextBtns2[nextBtns2.length - 1]!.trigger('click')
    await settle()

    const execBtn = wrapper.findAll('button').find((b) => b.text().includes('执行'))
    expect(execBtn).toBeTruthy()
    await execBtn!.trigger('click')
    await settle()

    expect(readPointNow).toHaveBeenCalled()
    expect(wrapper.text()).toContain('执行完成')
    wrapper.unmount()
  })

  // B2：批量写入不再逐点拼端点，整批一次调用统一入口 writePoints，
  // 单/批分发由后端按条数决定（前端只传数组）。
  it('批量写入整批一次调用统一入口 writePoints', async () => {
    const wrapper = mount(BatchCommand)
    await settle()

    // step 0：切到「批量写入」并填写写入值
    const radios = wrapper.findAll('.el-radio-button input')
    expect(radios.length).toBe(2)
    await radios[1]!.setValue(true)
    await settle()
    const valueInput = wrapper.find('input[placeholder="请输入要写入的数值"]')
    expect(valueInput.exists()).toBe(true)
    await valueInput.setValue('42')
    await settle()

    await wrapper.findAll('button').find((b) => b.text().includes('下一步'))!.trigger('click')
    await settle()
    await wrapper.findAll('button').find((b) => b.text().includes('全部加入'))!.trigger('click')
    await settle()
    const nextBtns = wrapper.findAll('button').filter((b) => b.text().includes('下一步'))
    await nextBtns[nextBtns.length - 1]!.trigger('click')
    await settle()
    await wrapper.findAll('button').find((b) => b.text().includes('执行'))!.trigger('click')
    await settle()

    expect(writePoints).toHaveBeenCalledTimes(1)
    expect(writePoints).toHaveBeenCalledWith([
      { id: 'dev_1_p1', value: 42 },
      { id: 'dev_2_p1', value: 42 },
    ])
    expect(readPointNow).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('执行完成')
    wrapper.unmount()
  })
})
