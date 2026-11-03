// QueueTriage.vue 单元测试：状态渲染 / 启动调用 / 409 提示。
// mock 边界同仓内其他页面测试：@/api/simConsumer 全量 mock、ElMessage 替换为 vi.fn。
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { formatDateTime } from '@/utils/datetime'
import type { SimConsumerStatus } from '@/api/types'

const {
  getSimConsumerStatusMock,
  startSimConsumerMock,
  stopSimConsumerMock,
} = vi.hoisted(() => ({
  getSimConsumerStatusMock: vi.fn(),
  startSimConsumerMock: vi.fn(),
  stopSimConsumerMock: vi.fn(),
}))

vi.mock('@/api/simConsumer', () => ({
  getSimConsumerStatus: getSimConsumerStatusMock,
  startSimConsumer: startSimConsumerMock,
  stopSimConsumer: stopSimConsumerMock,
}))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

import QueueTriage from './QueueTriage.vue'
import { ElMessage } from 'element-plus'
import { useAuthStore } from '@/stores/auth'

/** 后端状态快照 */
function status(over: Partial<SimConsumerStatus> = {}): SimConsumerStatus {
  return {
    running: true,
    enabled: true,
    consumed: 1234n,
    batches: 12n,
    queue_len: 7,
    last_consumed_at_ms: 1780000000000n,
    started_at_ms: 1779999000000n,
    uptime_ms: 65000n,
    batch_size: 100,
    interval_ms: 1000n,
    log_every: 100,
    blocked_reason: null,
    ...over,
  }
}

const wrappers: ReturnType<typeof mount>[] = []

afterEach(() => {
  for (const w of wrappers) w.unmount()
  wrappers.length = 0
})

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(QueueTriage)
  wrappers.push(wrapper)
  await settle()
  return wrapper
}

async function findButton(wrapper: Awaited<ReturnType<typeof mountPage>>, text: string) {
  const btn = wrapper.findAll('button').find((b) => b.text().includes(text))
  expect(btn, `未找到「${text}」按钮`).toBeTruthy()
  return btn!
}

describe('QueueTriage.vue', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    getSimConsumerStatusMock.mockResolvedValue(status())
    startSimConsumerMock.mockResolvedValue(status())
    stopSimConsumerMock.mockResolvedValue(status({ running: false }))
  })

  it('进入页面拉取状态并渲染运行态/指标', async () => {
    const wrapper = await mountPage()

    expect(getSimConsumerStatusMock).toHaveBeenCalled()

    const text = wrapper.text()
    // 页面归属与用途
    expect(text).toContain('队列排障')
    // 运行态与配置意图分开呈现
    expect(text).toContain('运行中')
    expect(text).toContain('已启用')
    // 指标
    expect(text).toContain('1234')
    expect(text).toContain('12')
    expect(text).toContain('7 条')
    // 运行时长转可读文案（65 秒 → 1 分 5 秒）
    expect(text).toContain('1 分 5 秒')
    // 最近消费时刻走 formatDateTime
    expect(text).toContain(formatDateTime(1780000000000))
  })

  it('点击启动调用 start 接口并带上表单参数', async () => {
    getSimConsumerStatusMock.mockResolvedValue(status({ running: false }))
    const wrapper = await mountPage()

    await (await findButton(wrapper, '启动')).trigger('click')
    await settle()

    expect(startSimConsumerMock).toHaveBeenCalledTimes(1)
    // 参数取后端下发的当前值，字段名为后端要求的 camelCase
    expect(startSimConsumerMock).toHaveBeenCalledWith({
      batchSize: 100,
      intervalMs: 1000,
      logEvery: 100,
    })
    expect(ElMessage.success).toHaveBeenCalledWith('模拟消费者已启动')
  })

  it('启动被拒（409 已在运行）时给出可读提示且不提示成功', async () => {
    getSimConsumerStatusMock.mockResolvedValue(status({ running: false }))
    startSimConsumerMock.mockRejectedValueOnce(new Error('请求失败(409): 模拟消费者已在运行'))
    const wrapper = await mountPage()

    await (await findButton(wrapper, '启动')).trigger('click')
    await settle()

    expect(ElMessage.error).toHaveBeenCalledWith(expect.stringContaining('已在运行'))
    expect(ElMessage.success).not.toHaveBeenCalled()
  })

  it('MQTT 已启用时展示告警并禁用启动按钮', async () => {
    getSimConsumerStatusMock.mockResolvedValue(
      status({ running: false, blocked_reason: 'mqtt_enabled' }),
    )
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('MQTT 已启用，模拟消费者不可用')

    const startBtn = await findButton(wrapper, '启动')
    expect(startBtn.attributes('disabled')).toBeDefined()
  })

  it('点击停止调用 stop 接口并提示已停止', async () => {
    const wrapper = await mountPage()

    await (await findButton(wrapper, '停止')).trigger('click')
    await settle()

    expect(stopSimConsumerMock).toHaveBeenCalledTimes(1)
    expect(ElMessage.success).toHaveBeenCalledWith('模拟消费者已停止')
  })

  it('无 config:write 权限时禁用动作并提示只读', async () => {
    // 启停走 config:write（后端 /api/sim_consumer 同口径鉴权），只读角色须提前禁用，
    // 而不是点了才吃 403。
    useAuthStore().roleId = 'viewer'
    getSimConsumerStatusMock.mockResolvedValue(status({ running: false }))
    const wrapper = await mountPage()

    expect(wrapper.text()).toContain('无配置写权限')
    const startBtn = await findButton(wrapper, '启动')
    expect(startBtn.attributes('disabled')).toBeDefined()

    // 切回管理员后同一按钮恢复可用：证明禁用确由权限驱动，而非运行态
    useAuthStore().roleId = 'admin'
    await nextTick()
    const again = await findButton(wrapper, '启动')
    expect(again.attributes('disabled')).toBeUndefined()
  })
})
