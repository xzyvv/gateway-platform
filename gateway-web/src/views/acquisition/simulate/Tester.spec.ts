import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { nextTick } from 'vue'
import { ElMessage } from 'element-plus'

/**
 * D1：采集测试 › 点位读写（Tester）行为测试。
 *
 * 现场调试页最容易出问题的不是"读不到值"，而是"读不到值却看不出哪里不对"。
 * 这里钉住三条行为契约：
 *   1. 能力未知（协议元数据没拿到）按**不支持**处理——不能放行一个必然报
 *      Unsupported 的写入按钮让用户误判成设备故障；
 *   2. 读写失败必须进操作记录并带上排障提示，不能静默；
 *   3. 写入必须先过二次确认，且成功后自动复读（现场靠复读确认"真写进去了"）。
 */

const { readOnce, writeOnce, confirmPointWrite, selectedDeviceId, listDevices, listPoints, listProtocolCapabilities, getProtocolDiag, getPackets } =
  vi.hoisted(() => ({
    readOnce: vi.fn(),
    writeOnce: vi.fn(),
    confirmPointWrite: vi.fn(),
    selectedDeviceId: { value: '' },
    listDevices: vi.fn(),
    listPoints: vi.fn(),
    listProtocolCapabilities: vi.fn(),
    getProtocolDiag: vi.fn(),
    getPackets: vi.fn(),
  }))

vi.mock('element-plus', async () => {
  const actual = await vi.importActual<typeof import('element-plus')>('element-plus')
  return {
    ...actual,
    ElMessage: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
  }
})

vi.mock('@/api/devices', () => ({ listDevices }))
vi.mock('@/api/points', () => ({ listPoints }))
vi.mock('@/api/protocols', () => ({ listProtocolCapabilities, getProtocolDiag }))
vi.mock('@/api/protocolTools/packetCapture', () => ({ getPackets }))
vi.mock('@/composables/data/useReadPointNow', () => ({ useReadPointNow: () => ({ readOnce }) }))
vi.mock('@/composables/data/usePointWrite', () => ({
  usePointWrite: () => ({ writeOnce }),
  confirmPointWrite,
}))
vi.mock('@/composables/acquisition/useDebugTargets', async () => {
  // 共享调试目标：真实实现会在设备列表就绪后回退到第一台设备，这里直接给
  // 「已选中 dev-1」这一稳定前提，让测试聚焦在读写行为本身
  const { ref } = await vi.importActual<typeof import('vue')>('vue')
  return { useSharedDebugTarget: () => ({ selectedDeviceId: ref('dev-1') }) }
})

import Tester from './Tester.vue'
import CurrentValuePanel from './parts/CurrentValuePanel.vue'
import WriteSection from './parts/WriteSection.vue'
import OperationLogPanel from './parts/OperationLogPanel.vue'
import DevicePointSelector from './parts/DevicePointSelector.vue'

const DEVICE = { id: 'dev-1', name: '现场从站', bus: 'bus-1', protocol: 'modbus' }
const POINT = { id: 'p-1', name: '温度', device_id: 'dev-1', bus: 'bus-1', protocol: 'modbus', data_type: 'int16' }

const CAPS = {
  supports_batch_read: true,
  supports_frame_extract: false,
  supports_write: true,
  supports_scan_slaves: true,
  supports_register_scan: false,
  supports_auto_detect: false,
  supports_write_multi: false,
  supports_diag: true,
}

async function settle() {
  await flushPromises()
  await nextTick()
}

async function mountPage() {
  const wrapper = mount(Tester)
  await settle()
  return wrapper
}

/** 选中点位（设备已由共享调试目标选中） */
async function selectDeviceAndPoint(wrapper: ReturnType<typeof mount>) {
  wrapper.findComponent(DevicePointSelector).vm.$emit('update:selectedPoint', 'p-1')
  wrapper.findComponent(DevicePointSelector).vm.$emit('point-change')
  await settle()
}

function logsOf(wrapper: ReturnType<typeof mount>) {
  return wrapper.findComponent(OperationLogPanel).props('logs') as Array<{ type: string; tag: string; message: string }>
}

describe('Tester 点位读写', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    selectedDeviceId.value = ''
    listDevices.mockResolvedValue([DEVICE])
    listPoints.mockResolvedValue([POINT])
    listProtocolCapabilities.mockResolvedValue([{ name: 'modbus', capabilities: CAPS }])
    getProtocolDiag.mockResolvedValue({ entries: [] })
    getPackets.mockResolvedValue([])
    readOnce.mockResolvedValue({ ok: true, value: 25.5, latencyMs: 12 })
    writeOnce.mockResolvedValue({ ok: true, status: 'ok', value: 26.5 })
    confirmPointWrite.mockResolvedValue(true)
  })

  it('加载设备与协议能力：能力声明决定是否放行写入入口', async () => {
    const wrapper = await mountPage()
    expect(listDevices).toHaveBeenCalled()
    expect(listProtocolCapabilities).toHaveBeenCalled()
    expect(wrapper.findComponent(WriteSection).props('supportsWrite')).toBe(true)
    expect(wrapper.findComponent(DevicePointSelector).props('supportsDiag')).toBe(true)
  })

  it('能力元数据拿不到：按不支持处理，不放行写入入口', async () => {
    listProtocolCapabilities.mockRejectedValue(new Error('network down'))
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    // 「能力未知」不等于「能力具备」：写入是真实副作用，必须保守禁用
    expect(wrapper.findComponent(WriteSection).props('supportsWrite')).toBe(false)
    expect(wrapper.findComponent(WriteSection).props('capsKnown')).toBe(false)
  })

  it('读取成功：当前值回显并写入操作记录', async () => {
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    wrapper.findComponent(CurrentValuePanel).vm.$emit('read')
    await settle()

    expect(readOnce).toHaveBeenCalledWith('p-1')
    expect(wrapper.findComponent(CurrentValuePanel).props('currentValue')).toBe(25.5)
    const logs = logsOf(wrapper)
    expect(logs[0]?.tag).toBe('读取')
    expect(logs[0]?.message).toContain('温度 = 25.5')
  })

  it('读取失败：当前值复位为「—」，操作记录带排障提示', async () => {
    readOnce.mockResolvedValue({ ok: false, error: 'timeout after 3 retries', latencyMs: 3000 })
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    wrapper.findComponent(CurrentValuePanel).vm.$emit('read')
    await settle()

    expect(wrapper.findComponent(CurrentValuePanel).props('currentValue')).toBe('—')
    expect(wrapper.findComponent(CurrentValuePanel).props('valueError')).toBe(true)
    const logs = logsOf(wrapper)
    expect(logs[0]?.type).toBe('danger')
    expect(logs[0]?.message).toContain('读取失败')
  })

  it('写入前必须二次确认：未确认则不发起写入请求', async () => {
    confirmPointWrite.mockResolvedValue(false)
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    wrapper.findComponent(WriteSection).vm.$emit('update:writeValue', 26.5)
    await settle()
    wrapper.findComponent(WriteSection).vm.$emit('write')
    await settle()

    expect(confirmPointWrite).toHaveBeenCalledWith([{ id: 'p-1', value: 26.5 }], '温度')
    expect(writeOnce).not.toHaveBeenCalled()
  })

  it('写入成功：记录写入结果并自动复读确认', async () => {
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    wrapper.findComponent(WriteSection).vm.$emit('update:writeValue', 26.5)
    await settle()
    wrapper.findComponent(WriteSection).vm.$emit('write')
    await settle()

    expect(writeOnce).toHaveBeenCalledWith('p-1', 26.5)
    const logs = logsOf(wrapper)
    expect(logs.some((l) => l.tag === '写入' && l.message.includes('26.5'))).toBe(true)
    // 写入后自动复读：现场靠复读确认"真写进去了"
    expect(readOnce).toHaveBeenCalled()
  })

  it('写入失败：给出错误提示并记录失败原因', async () => {
    writeOnce.mockResolvedValue({ ok: false, error: 'Modbus exception 0x02', status: 'error' })
    const wrapper = await mountPage()
    await selectDeviceAndPoint(wrapper)

    wrapper.findComponent(WriteSection).vm.$emit('update:writeValue', 26.5)
    await settle()
    wrapper.findComponent(WriteSection).vm.$emit('write')
    await settle()

    expect(ElMessage.error).toHaveBeenCalled()
    const logs = logsOf(wrapper)
    expect(logs.some((l) => l.tag === '写入' && l.message.includes('写入失败'))).toBe(true)
  })
})
