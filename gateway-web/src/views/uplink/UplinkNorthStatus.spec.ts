import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import UplinkNorthStatus from './UplinkNorthStatus.vue'

const mockGetNorthStatus = vi.fn()
const mockGetPassthroughSessions = vi.fn()
vi.mock('@/api/uplink', () => ({
  getNorthStatus: () => mockGetNorthStatus(),
  getPassthroughSessions: () => mockGetPassthroughSessions(),
}))

const noPassthrough = { enabled: false, sessions: [] }

const fullStatus = {
  healthy: true,
  channels: [
    {
      name: 'mqtt',
      type: 'mqtt',
      enabled: true,
      connected: true,
      queue_length: 5,
      extra: { broker: 'tcp://broker:1883', circuit_open: false },
    },
    {
      name: 'webhook',
      type: 'http',
      enabled: true,
      connected: null,
      queue_length: 0,
      extra: { url: 'https://hook/api', success_count: 10, failed_count: 2, avg_latency_ms: null },
    },
  ],
  aggregated: {
    mqtt: 1,
    http: 1,
    mqtt_connected: 1,
    http_connected: 1,
    pending_messages: 5,
    total_messages_sent: 110,
  },
  // 第 1 期冗余收口：原 `/api/north/servers` 的内容并入北向状态响应
  servers: [{ name: 'opcua', running: true }],
}

describe('UplinkNorthStatus.vue — C1 北向服务统一状态', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockGetPassthroughSessions.mockResolvedValue(noPassthrough)
  })

  it('渲染 aggregated 概览统计卡片', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    // 通道总数 = mqtt+http = 2
    expect(text).toContain('上行通道总数')
    expect(text).toContain('2')
    // 已连接 = 1 (mqtt)
    expect(text).toContain('已连接')
    // 队列积压 = 5
    expect(text).toContain('消息队列积压')
    // 上行消息总数 = 110
    expect(text).toContain('上行消息总数')
    expect(text).toContain('110')
  })

  it('MQTT 通道展示 broker 与断路器状态', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('tcp://broker:1883')
    expect(text).toContain('断路器')
    expect(text).toContain('正常')
  })

  it('Webhook 通道展示成功/失败计数与平均延迟', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    // 成功/失败计数
    expect(text).toContain('10')
    expect(text).toContain('2')
    // avg_latency_ms 为 null 时显示 '-'
    expect(text).toContain('平均延迟')
  })

  it('MQTT 已连接时显示"已启用·已连接"标签', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    expect(wrapper.text()).toContain('已启用·已连接')
  })

  it('API 失败时组件仍可渲染', async () => {
    mockGetNorthStatus.mockRejectedValue(new Error('fail'))
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    expect(wrapper.exists()).toBe(true)
  })

  it('通道存在 last_error 时展示最近错误文本', async () => {
    const withError = {
      ...fullStatus,
      channels: [
        { ...fullStatus.channels[0], connected: false, extra: { broker: 'tcp://broker:1883', circuit_open: true, last_error: 'connection refused' } },
      ],
    }
    mockGetNorthStatus.mockResolvedValue(withError)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('最近错误')
    expect(text).toContain('connection refused')
  })

  it('扇出丢帧数大于 0 时展示告警，为 0 时不展示', async () => {
    mockGetNorthStatus.mockResolvedValue({
      ...fullStatus,
      aggregated: { ...fullStatus.aggregated, lagged_dropped_batches: 7 },
    })
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()
    expect(wrapper.text()).toContain('上行扇出累计丢帧 7 批次')

    mockGetNorthStatus.mockResolvedValue({ ...fullStatus, aggregated: { ...fullStatus.aggregated, lagged_dropped_batches: 0 } })
    const clean = mount(UplinkNorthStatus)
    await flushPromises()
    expect(clean.text()).not.toContain('上行扇出累计丢帧')
  })

  it('C2 统一结论：已启用通道全部未连接时显示「上行中断」', async () => {
    mockGetNorthStatus.mockResolvedValue({
      ...fullStatus,
      channels: [{ ...fullStatus.channels[0], connected: false, extra: { broker: 'tcp://broker:1883' } }],
    })
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('上行中断')
    expect(text).toContain('所有已启用的上行通道均未连接')
  })

  it('C2 统一结论：透传会话纳入北向健康视图', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    mockGetPassthroughSessions.mockResolvedValue({
      enabled: true,
      sessions: [{ id: 's1', bus: 'bus1', started_at: 1767225600, timeout_ms: 60000 }],
    })
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    const text = wrapper.text()
    expect(text).toContain('透传会话')
    expect(text).toContain('1 个活动会话')
    expect(text).toContain('bus1')
    expect(text).toContain('60000 ms')
  })

  it('C2 统一结论：透传未启用时给出启用指引', async () => {
    mockGetNorthStatus.mockResolvedValue(fullStatus)
    const wrapper = mount(UplinkNorthStatus)
    await flushPromises()

    expect(wrapper.text()).toContain('透传模式未启用')
  })
})
