import { describe, it, expect, vi, beforeEach } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

vi.mock('@/api/protocolTools', () => ({
  scanBus: vi.fn(),
  scanBusStream: vi.fn(),
  canStreamScan: vi.fn(),
}))
vi.mock('@/api/protocols', () => ({
  listProtocolCapabilities: vi.fn(),
}))
vi.mock('@/composables/acquisition/useDebugTargets', () => ({
  useDebugTargets: () => ({
    buses: ref([{ port_name: '/dev/ttyUSB0', protocol: 'modbus' }]),
    busOptions: ref([{ label: '调试总线', value: '/dev/ttyUSB0' }]),
    loading: ref(false),
    selectedBus: ref('/dev/ttyUSB0'),
  }),
}))

import SlaveScan from './SlaveScan.vue'
import {
  scanBus,
  scanBusStream,
  canStreamScan,
  type SlaveScanResult,
} from '@/api/protocolTools'
import { listProtocolCapabilities } from '@/api/protocols'

/**
 * P0-2 契约 §7：扫描模式选择的回归钉子——
 * 环境支持流式时走 SSE（逐格点亮 + AbortSignal 可停止）；
 * 旧浏览器 / Tauri（canStreamScan=false）回落阻塞 scanBus，不建流。
 */
const online: SlaveScanResult = { slaveId: 2, responded: true, deviceName: null, funcCode: '03', latencyMs: 7 }

async function mountAndClickScan() {
  const wrapper = mount(SlaveScan)
  await flushPromises()
  const btn = wrapper.findAll('button').find((b) => b.text() === '扫描')
  expect(btn).toBeTruthy()
  await btn!.trigger('click')
  await flushPromises()
  return wrapper
}

describe('SlaveScan 扫描模式选择（p0-2 §7）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    // 默认：modbus 支持从站扫描，既有扫描流程测试不受门控影响
    vi.mocked(listProtocolCapabilities).mockResolvedValue([
      { name: 'modbus', capabilities: { supports_batch_read: true, supports_frame_extract: true, supports_write: true, supports_scan_slaves: true, supports_register_scan: true, supports_auto_detect: true, supports_write_multi: false, supports_diag: false } },
    ])
  })

  it('环境支持流式：走 scanBusStream（带 AbortSignal），summary 落地渲染', async () => {
    vi.mocked(canStreamScan).mockReturnValue(true)
    vi.mocked(scanBusStream).mockImplementation(async (_bus, _s, _e, handlers) => {
      handlers.onStarted?.({
        bus: '/dev/ttyUSB0', rangeStart: 1, rangeEnd: 20, total: 20,
        timeoutMs: 500, intervalMs: 50, startedAtMs: 1000,
      })
      handlers.onSlave?.({ slaveId: 1, responded: false })
      handlers.onSlave?.(online)
      handlers.onSummary?.({
        bus: '/dev/ttyUSB0', scanned: 20, found: 1, noise: 0, durationMs: 1200,
        results: [{ slaveId: 1, responded: false }, online],
      })
    })

    const wrapper = await mountAndClickScan()

    expect(scanBusStream).toHaveBeenCalledOnce()
    const [bus, rangeStart, rangeEnd, handlers] = vi.mocked(scanBusStream).mock.calls[0]!
    expect(bus).toBe('/dev/ttyUSB0')
    expect(rangeStart).toBe(1)
    expect(rangeEnd).toBe(20)
    expect(handlers.signal).toBeInstanceOf(AbortSignal)
    expect(scanBus).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('发现 1 台设备')
  })

  it('环境不支持流式（canStreamScan=false）：回落阻塞 scanBus，不建流', async () => {
    vi.mocked(canStreamScan).mockReturnValue(false)
    vi.mocked(scanBus).mockResolvedValue([{ slaveId: 1, responded: false }, online])

    const wrapper = await mountAndClickScan()

    expect(scanBus).toHaveBeenCalledWith('/dev/ttyUSB0', 1, 20)
    expect(scanBusStream).not.toHaveBeenCalled()
    expect(wrapper.text()).toContain('发现 1 台设备')
  })

  it('扫描中按钮变「停止」，点击调 abort（已扫部分保留）', async () => {
    vi.mocked(canStreamScan).mockReturnValue(true)
    vi.mocked(scanBusStream).mockImplementation(async (_bus, _s, _e, handlers) => {
      handlers.onSlave?.(online)
      // 挂起直到用户停止（真实场景是等待总线响应/心跳）
      await new Promise<void>((resolve) => {
        if (handlers.signal?.aborted) return resolve()
        handlers.signal?.addEventListener('abort', () => resolve())
      })
    })

    const wrapper = mount(SlaveScan)
    await flushPromises()
    const scanBtn = wrapper.findAll('button').find((b) => b.text() === '扫描')
    await scanBtn!.trigger('click')
    await flushPromises()

    const stopBtn = wrapper.findAll('button').find((b) => b.text() === '停止')
    expect(stopBtn).toBeTruthy()
    // 已扫的格子已点亮
    expect(wrapper.text()).toContain('已扫 1 / 20')

    await stopBtn!.trigger('click')
    await flushPromises()

    const handlers = vi.mocked(scanBusStream).mock.calls[0]![3]
    expect(handlers.signal?.aborted).toBe(true)
    // 停止后按钮恢复「扫描」，可再次发起
    expect(wrapper.findAll('button').some((b) => b.text() === '扫描')).toBe(true)
  })
})

/**
 * 审查#3b：从站扫描能力门控。
 * DLT645/CJT188 等协议未实现从站扫描，前端应按协议能力位禁用按钮并给出提示，
 * 而非等后端返回错误。能力元数据缺失（未加载/加载失败）时同样禁用：
 * 后端仅对 Modbus 总线开放扫描，放行等于给用户一个「点了必然失败」的按钮。
 */
describe('SlaveScan 协议能力门控（#3b）', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('协议 supports_scan_slaves=true：扫描按钮可用，无禁用提示', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue([
      { name: 'modbus', capabilities: { supports_batch_read: true, supports_frame_extract: true, supports_write: true, supports_scan_slaves: true, supports_register_scan: true, supports_auto_detect: true, supports_write_multi: false, supports_diag: false } },
    ])
    const wrapper = mount(SlaveScan)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text() === '扫描')!
    expect(btn.attributes('disabled')).toBeUndefined()
    expect(wrapper.text()).not.toContain('不支持从站扫描')
  })

  it('协议 supports_scan_slaves=false：扫描按钮禁用，展示「不支持从站扫描」提示', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue([
      { name: 'modbus', capabilities: { supports_batch_read: true, supports_frame_extract: true, supports_write: true, supports_scan_slaves: false, supports_register_scan: false, supports_auto_detect: false, supports_write_multi: false, supports_diag: false } },
    ])
    const wrapper = mount(SlaveScan)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text() === '扫描')!
    expect(btn.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('不支持从站扫描')
    // 口径钉子（C2）：协议语义上不适用 ≠ 尚未实现，现场不得误判为缺功能
    expect(wrapper.text()).toContain('从站扫描不适用（不是尚未实现）')
    expect(wrapper.text()).toContain('当前总线协议（modbus）不支持从站扫描')
  })

  it('能力元数据加载失败：按不支持处理，扫描按钮禁用并提示加载失败', async () => {
    vi.mocked(listProtocolCapabilities).mockRejectedValue(new Error('network'))
    const wrapper = mount(SlaveScan)
    await flushPromises()
    const btn = wrapper.findAll('button').find((b) => b.text() === '扫描')!
    expect(btn.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('协议能力元数据加载失败')
  })
})
