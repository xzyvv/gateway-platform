import { describe, it, expect, beforeEach, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import DataFlowTrace from './DataFlowTrace.vue'

// Mock API modules
const mockListDevices = vi.fn()
const mockListPoints = vi.fn()
const mockListPointQuality = vi.fn()
const mockReadPointNow = vi.fn()
const mockGetUplinkConfig = vi.fn()
const mockTestUplink = vi.fn()

vi.mock('@/api/devices', () => ({
  listDevices: () => mockListDevices(),
}))
vi.mock('@/api/points', () => ({
  listPoints: (id: string) => mockListPoints(id),
  readPointNow: (id: string) => mockReadPointNow(id),
}))
vi.mock('@/api/dataQuality', () => ({
  listPointQuality: () => mockListPointQuality(),
}))
vi.mock('@/api/uplink', () => ({
  getUplinkConfig: () => mockGetUplinkConfig(),
  testUplink: (payload: unknown) => mockTestUplink(payload),
}))

// Element Plus ElMessage mock
vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return { ...actual, ElMessage: { error: vi.fn(), success: vi.fn(), warning: vi.fn() } }
})

const devices = [
  { id: 'dev1', name: '设备A', bus: 'bus1', protocol: 'modbus', address: '1', template: 'tpl' },
  { id: 'dev2', name: '设备B', bus: 'bus1', protocol: 'modbus', address: '2', template: 'tpl' },
]
const points = [
  { id: 'pt1', name: '电压', bus: 'bus1', protocol: 'modbus', address: '0', slave_id: 1, device_id: 'dev1' },
  { id: 'pt2', name: '电流', bus: 'bus1', protocol: 'modbus', address: '2', slave_id: 1, device_id: 'dev1' },
]

/** 获取组件 setupState（script setup 内部状态，refs 已自动解包） */
// 测试辅助：访问 Vue 内部 setupState，此处 any 合理（内部状态无公开类型）
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function setupState(wrapper: ReturnType<typeof mount>): any {
  // @ts-expect-error access internal setup state
  return wrapper.vm.$.setupState
}

describe('DataFlowTrace.vue — 数据链路追踪', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    mockListDevices.mockResolvedValue(devices)
    mockListPoints.mockResolvedValue(points)
    mockListPointQuality.mockResolvedValue([])
    mockGetUplinkConfig.mockResolvedValue({
      enabled: true,
      template: '{"device":"${device_id}","value":${value}}',
      encoding: 'json',
      max_bytes: 4096,
    })
    mockTestUplink.mockResolvedValue({ ok: true, rendered: '{"device":"dev1","value":42}', bytes: 28 })
    mockReadPointNow.mockResolvedValue({ status: 'ok', value: 42 })
  })

  it('挂载时调用 listDevices 加载设备列表', async () => {
    mount(DataFlowTrace)
    await flushPromises()
    expect(mockListDevices).toHaveBeenCalledTimes(1)
  })

  it('选择设备后调用 listPoints 加载该设备点位', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    expect(mockListPoints).toHaveBeenCalledWith('dev1')
    expect(s.points).toEqual(points)
  })

  it('模板展开点位（配置 API 为空）由运行时点位补齐，追踪仍可用', async () => {
    mockListPoints.mockResolvedValue([])
    mockListPointQuality.mockResolvedValue([
      { point_id: 'rt1', point_name: '模板点位-温度', device_id: 'dev1', device_name: '设备A', value: 25, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
      { point_id: 'rt2', point_name: '其他设备点位', device_id: 'dev2', device_name: '设备B', value: 1, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
    ])
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    // 只合入当前设备的运行时点位；rt2 属于 dev2 不出现
    expect(s.points).toEqual([{ id: 'rt1', name: '模板点位-温度', device_id: 'dev1' }])
    // 运行时点位可直接用于读取（readPointNow 按 point_id 读）
    s.selectedPoint = 'rt1'
    await s.doRead()
    expect(mockReadPointNow).toHaveBeenCalledWith('rt1')
  })

  it('配置点位与运行时点位同 id 时，以配置点位的元数据为准', async () => {
    mockListPointQuality.mockResolvedValue([
      { point_id: 'pt1', point_name: '运行时名', device_id: 'dev1', device_name: '设备A', value: 1, quality: 'good', consecutive_failures: 0, last_update_at: 0 },
    ])
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    const pt1 = s.points.find((p: { id: string }) => p.id === 'pt1')
    expect(pt1.name).toBe('电压')
    expect(pt1.address).toBe('0')
  })

  it('切换设备时清空已选点位与前序结果', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 1 }
    s.renderedPayload = 'xxx'
    await s.onDeviceChange()
    expect(s.selectedPoint).toBe('')
    expect(s.readResult).toBeNull()
    expect(s.renderedPayload).toBe('')
  })

  it('立即读取：调用 readPointNow 并展示值与状态', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    await s.doRead()
    await flushPromises()
    expect(mockReadPointNow).toHaveBeenCalledWith('pt1')
    expect(s.readResult).toEqual({ status: 'ok', value: 42 })
    expect(s.readLatency).toBeGreaterThanOrEqual(0)
  })

  it('isGood：status 为 ok 或 success 时为 true', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.readResult = { status: 'ok', value: 1 }
    expect(s.isGood).toBe(true)
    s.readResult = { status: 'success', value: 1 }
    expect(s.isGood).toBe(true)
    s.readResult = { status: 'fail', value: 1 }
    expect(s.isGood).toBe(false)
  })

  it('预览渲染：调用 testUplink(send=false) 并展示渲染报文', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 42 }
    await s.doRender()
    await flushPromises()
    expect(mockGetUplinkConfig).toHaveBeenCalled()
    expect(mockTestUplink).toHaveBeenCalledWith(
      expect.objectContaining({ send: false, vars: expect.objectContaining({ device_id: 'dev1', point_id: 'pt1', value: 42 }) }),
    )
    expect(s.renderedPayload).toBe('{"device":"dev1","value":42}')
  })

  it('测试上送：调用 testUplink(send=true) 并展示 publish 结果', async () => {
    mockTestUplink.mockResolvedValue({
      ok: true,
      rendered: '{"device":"dev1","value":42}',
      bytes: 28,
      publish: { topic: 'factory/data', qos: 1, delivered: true, latency_ms: 50 },
    })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 42 }
    s.renderedPayload = '{"device":"dev1","value":42}'
    await s.doSend()
    await flushPromises()
    expect(mockTestUplink).toHaveBeenCalledWith(expect.objectContaining({ send: true }))
    expect(s.sendResult.ok).toBe(true)
    expect(s.sendResult.publish.topic).toBe('factory/data')
    expect(s.sendResult.publish.delivered).toBe(true)
  })

  it('读取失败时 readResult 为 null，readError 记录错误并结束 loading', async () => {
    mockReadPointNow.mockRejectedValue(new Error('timeout'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    await s.doRead()
    await flushPromises()
    expect(s.readResult).toBeNull()
    expect(s.readError).toBe('timeout')
    expect(s.reading).toBe(false)
  })

  it('渲染失败（resp.ok=false）时不展示报文，不缓存模板，并写入 renderError', async () => {
    mockTestUplink.mockResolvedValue({ ok: false, rendered: '', bytes: 0, error: 'template syntax error' })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 42 }
    await s.doRender()
    await flushPromises()
    expect(s.renderedPayload).toBe('')
    expect(s.uplinkTemplate).toBe('')
    expect(s.renderError).toBe('template syntax error')
  })

  it('上送异常时 sendResult.ok=false 并记录错误', async () => {
    mockTestUplink.mockRejectedValue(new Error('broker unreachable'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 42 }
    s.renderedPayload = '{"device":"dev1","value":42}'
    await s.doSend()
    await flushPromises()
    expect(s.sendResult.ok).toBe(false)
    expect(s.sendResult.error).toContain('broker unreachable')
    expect(s.sending).toBe(false)
  })

  it('未选点位时 doRead 直接返回不调用 API', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedPoint = ''
    await s.doRead()
    expect(mockReadPointNow).not.toHaveBeenCalled()
  })

  it('无读取结果时 doRender/doSend 直接返回', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.readResult = null
    await s.doRender()
    await s.doSend()
    expect(mockGetUplinkConfig).not.toHaveBeenCalled()
    expect(mockTestUplink).not.toHaveBeenCalled()
  })

  it('渲染成功后 DOM 展示报文内容', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 42 }
    await s.doRender()
    await flushPromises()
    await nextTick()
    expect(wrapper.text()).toContain('"device":"dev1"')
    expect(wrapper.text()).toContain('"value":42')
  })

  it('resetTrace：清空读取/渲染/上送结果，保留设备与点位选择', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 1 }
    s.readLatency = 123
    s.renderedPayload = '{"x":1}'
    s.sendResult = { ok: true }
    s.resetTrace()
    expect(s.readResult).toBeNull()
    expect(s.readLatency).toBe(0)
    expect(s.readError).toBe('')
    expect(s.renderedPayload).toBe('')
    expect(s.uplinkTemplate).toBe('')
    expect(s.sendResult).toBeNull()
    // 设备/点位选择保留
    expect(s.selectedDevice).toBe('dev1')
    expect(s.selectedPoint).toBe('pt1')
  })

  it('采集失败后 readError 非空，重置链路按钮可用', async () => {
    mockReadPointNow.mockRejectedValue(new Error('timeout'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    await s.doRead()
    await flushPromises()
    // readError 已记录，但 readResult/renderedPayload/sendResult 都为空 → 仅靠 readError 触发按钮可用
    expect(s.readError).toBe('timeout')
    const btn = wrapper.findAll('button').find((b) => b.text().includes('重置链路'))
    expect(btn).toBeTruthy()
    expect(btn!.attributes('disabled')).toBeUndefined()
  })

  it('重新读取 doRead 会清空旧的渲染报文与上送结果（避免旧数据残留）', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    // 模拟已有旧结果
    s.renderedPayload = '{"old":true}'
    s.sendResult = { ok: true }
    await s.doRead()
    await flushPromises()
    expect(s.renderedPayload).toBe('')
    expect(s.sendResult).toBeNull()
  })

  it('重新渲染 doRender 会清空旧的上送结果（避免旧数据残留）', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    s.readResult = { status: 'ok', value: 1 }
    s.sendResult = { ok: true }
    await s.doRender()
    await flushPromises()
    expect(s.sendResult).toBeNull()
  })

  it('stepDone：各步骤完成状态随数据更新', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    // 初始：未选设备/点位
    expect(s.stepDone.s1).toBe(false)
    expect(s.stepDone.s2).toBe(false)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    expect(s.stepDone.s1).toBe(true)
    s.selectedPoint = 'pt1'
    expect(s.stepDone.s2).toBe(true)
    s.readResult = { status: 'ok', value: 1 }
    expect(s.stepDone.s3).toBe(true)
    s.renderedPayload = 'x'
    expect(s.stepDone.s4).toBe(true)
    s.sendResult = { ok: true }
    expect(s.stepDone.s5).toBe(true)
  })

  it('pointInfo：从点位提取 data_type 与 scale，非字符串/数字时回退默认值', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    // pt1 无 data_type/scale，应回退为 '' 和 1
    expect(s.pointInfo.data_type).toBe('')
    expect(s.pointInfo.scale).toBe(1)
    // 注入带类型字段的点位
    s.points = [{ id: 'pt1', name: '电压', bus: 'bus1', protocol: 'modbus', address: '0', slave_id: 1, data_type: 'float32', scale: 0.1 }]
    s.selectedPoint = 'pt1'
    expect(s.pointInfo.data_type).toBe('float32')
    expect(s.pointInfo.scale).toBe(0.1)
  })

  it('loadDevices 失败时提示错误且设备列表为空', async () => {
    mockListDevices.mockRejectedValue(new Error('network down'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    expect(s.devices).toEqual([])
  })

  it('onDeviceChange 加载点位失败时提示错误且 points 为空', async () => {
    mockListPoints.mockRejectedValue(new Error('device offline'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    expect(s.points).toEqual([])
  })

  it('latencyLevel：按耗时返回 fast/normal/slow', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.readLatency = 50
    expect(s.latencyLevel).toBe('fast')
    s.readLatency = 100
    expect(s.latencyLevel).toBe('normal')
    s.readLatency = 499
    expect(s.latencyLevel).toBe('normal')
    s.readLatency = 500
    expect(s.latencyLevel).toBe('slow')
    s.readLatency = 1200
    expect(s.latencyLevel).toBe('slow')
  })

  it('stepStatus：步骤随前置条件完成依次 active→done', async () => {
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    // 初始：无设备
    expect(s.stepStatus).toEqual(['active', 'pending', 'pending', 'pending', 'pending'])
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    expect(s.stepStatus[0]).toBe('done')
    expect(s.stepStatus[1]).toBe('active')
    s.selectedPoint = 'pt1'
    expect(s.stepStatus[1]).toBe('done')
    expect(s.stepStatus[2]).toBe('active')
    s.readResult = { status: 'ok', value: 1 }
    expect(s.stepStatus[2]).toBe('done')
    expect(s.stepStatus[3]).toBe('active')
    s.renderedPayload = 'x'
    expect(s.stepStatus[3]).toBe('done')
    expect(s.stepStatus[4]).toBe('active')
    s.sendResult = { ok: true }
    expect(s.stepStatus).toEqual(['done', 'done', 'done', 'done', 'done'])
  })

  it('copyPayload：报文为空时不调用剪贴板', async () => {
    const writeText = vi.fn()
    Object.assign(navigator, { clipboard: { writeText } })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.renderedPayload = ''
    await s.copyPayload()
    expect(writeText).not.toHaveBeenCalled()
  })

  it('copyPayload：成功时写入剪贴板并提示', async () => {
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.assign(navigator, { clipboard: { writeText } })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.renderedPayload = '{"device":"dev1","value":42}'
    await s.copyPayload()
    expect(writeText).toHaveBeenCalledWith('{"device":"dev1","value":42}')
  })

  it('copyPayload：剪贴板失败时提示错误', async () => {
    const writeText = vi.fn().mockRejectedValue(new Error('denied'))
    Object.assign(navigator, { clipboard: { writeText } })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.renderedPayload = '{"x":1}'
    await s.copyPayload()
    expect(writeText).toHaveBeenCalledWith('{"x":1}')
  })

  it('copyPayload：防重复触发，并发调用只写一次剪贴板', async () => {
    let resolveWrite: () => void = () => {}
    const writeText = vi.fn().mockImplementation(
      () => new Promise<void>((res) => { resolveWrite = res }),
    )
    Object.assign(navigator, { clipboard: { writeText } })
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.renderedPayload = '{"x":1}'
    // 并发触发两次，第二次应被 copying 防抖挡住
    const p1 = s.copyPayload()
    const p2 = s.copyPayload()
    expect(s.copying).toBe(true)
    resolveWrite()
    await p1
    await p2
    expect(writeText).toHaveBeenCalledTimes(1)
    expect(s.copying).toBe(false)
  })

  it('loadDevices 失败时设置 loadDevicesError 供页面错误视图使用', async () => {
    mockListDevices.mockRejectedValue(new Error('network down'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    expect(s.loadDevicesError).toBe('network down')
    expect(s.loadingDevices).toBe(false)
  })

  it('onDeviceChange 失败时设置 loadPointsError，并在切换设备时清空', async () => {
    mockListPoints.mockRejectedValueOnce(new Error('point offline'))
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    expect(s.loadPointsError).toBe('point offline')
    // 切换设备后，loadPointsError 应被清空
    s.selectedDevice = 'dev2'
    mockListPoints.mockResolvedValueOnce([])
    await s.onDeviceChange()
    await flushPromises()
    expect(s.loadPointsError).toBe('')
  })

  it('doRead 并发调用只触发一次 API', async () => {
    let resolveRead!: (v: { status: string; value: number }) => void
    mockReadPointNow.mockImplementation(
      () => new Promise<{ status: string; value: number }>((res) => { resolveRead = res }),
    )
    const wrapper = mount(DataFlowTrace)
    await flushPromises()
    const s = setupState(wrapper)
    s.selectedDevice = 'dev1'
    await s.onDeviceChange()
    await flushPromises()
    s.selectedPoint = 'pt1'
    const p1 = s.doRead()
    const p2 = s.doRead() // 并发第二次，应被 reading 锁挡住
    expect(s.reading).toBe(true)
    resolveRead({ status: 'ok', value: 1 })
    await p1
    await p2
    expect(mockReadPointNow).toHaveBeenCalledTimes(1)
  })
})
