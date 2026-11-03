import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import ConnectionTest from './ConnectionTest.vue'

const mockListDevices = vi.fn()
const mockListPoints = vi.fn()
const mockReadPointNow = vi.fn()
const mockGetNorthStatus = vi.fn()
const mockGetUplinkConfig = vi.fn()
const mockTestUplink = vi.fn()

vi.mock('@/api/devices', () => ({
  listDevices: () => mockListDevices(),
}))
vi.mock('@/api/points', () => ({
  listPoints: (id: string) => mockListPoints(id),
  readPointNow: (id: string) => mockReadPointNow(id),
}))
vi.mock('@/api/uplink', () => ({
  getNorthStatus: () => mockGetNorthStatus(),
  getUplinkConfig: () => mockGetUplinkConfig(),
  testUplink: (payload: unknown) => mockTestUplink(payload),
}))

// ElMessage 会 teleport 到 body 并触发动画，测试环境用空实现替代，其余 element-plus 保持真实
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() } }
})

const deviceFixture = [
  { id: 'd1', name: '电表1', template: 't', bus: 'b1', protocol: 'modbus', address: '1', slave_id: 1, address_map: null, interval: 5 },
]
const northFixture = {
  healthy: true,
  channels: [{ name: 'mqtt', type: 'mqtt', enabled: true, connected: true, queue_length: 0, extra: { broker: 'tcp://b:1883' } }],
  aggregated: { mqtt: 1, http: 0, mqtt_connected: 1, http_connected: 0, pending_messages: 0, total_messages_sent: 0 },
}

describe('ConnectionTest.vue — S4 容器集成测试', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockListDevices.mockResolvedValue(deviceFixture)
    mockGetNorthStatus.mockResolvedValue(northFixture)
    mockListPoints.mockResolvedValue([
      {
        id: 'p1', name: '电压', bus: 'b1', protocol: 'modbus', address: '0', slave_id: 1,
        address_num: null, data_id: null, function_code: 3, data_type: 'float32', byte_order: 'ABCD',
        register_count: 2, interval: 5, scale: 1, offset: 0, formula: null, ct_ratio: null, pt_ratio: null,
        deadband: 0, on_change: false, unit: 'V', alarm_high: null, alarm_low: null, aggregation: null,
        modbus_register: null, device_id: 'd1',
      },
    ])
    mockReadPointNow.mockResolvedValue({ status: 'ok', value: 220 })
    mockGetUplinkConfig.mockResolvedValue({ enabled: true, template: '{"device":"${device_id}","value":${value}}', encoding: 'json', max_bytes: 1024 })
    mockTestUplink.mockResolvedValue({ ok: true, rendered: '{}', bytes: 2, publish: { topic: 't/q', qos: 1, delivered: true, latency_ms: 5 } })
  })

  it('挂载时加载设备与上行通道，渲染容器与三个子组件', async () => {
    const wrapper = mount(ConnectionTest)
    await flushPromises()
    const text = wrapper.text()
    expect(text).toContain('连接测试中心')
    expect(text).toContain('设备采集链路（1）')
    expect(text).toContain('电表1')
    expect(text).toContain('上行通道（1）')
    expect(text).toContain('mqtt')
    expect(mockListDevices).toHaveBeenCalledTimes(1)
    expect(mockGetNorthStatus).toHaveBeenCalledTimes(1)
  })

  it('点击"一键测试全部"：设备读取 + 通道发送均被调用，汇总面板显示通过统计', async () => {
    const wrapper = mount(ConnectionTest)
    await flushPromises()
    // 初始未测试 → 汇总面板隐藏（total=0）
    expect(wrapper.text()).not.toContain('平均耗时')

    const btn = wrapper.findAll('button').find((b) => b.text().includes('一键测试全部'))
    expect(btn).toBeTruthy()
    await btn!.trigger('click')
    await flushPromises()

    // 设备测试链路：listPoints → readPointNow
    expect(mockListPoints).toHaveBeenCalledWith('d1')
    expect(mockReadPointNow).toHaveBeenCalledWith('p1')
    // 通道测试链路：getUplinkConfig → testUplink(send=true)
    expect(mockGetUplinkConfig).toHaveBeenCalledTimes(1)
    expect(mockTestUplink).toHaveBeenCalledWith(expect.objectContaining({ send: true }))
    // 测试后汇总面板出现
    const text = wrapper.text()
    expect(text).toContain('平均耗时')
    expect(text).toContain('通过')
  })
})