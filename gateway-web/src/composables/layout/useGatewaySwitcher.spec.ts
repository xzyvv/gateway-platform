import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h, nextTick } from 'vue'
import { mount } from '@vue/test-utils'

// 必须在 useGatewaySwitcher 引入之前 mock '@/api'
// 因为 composable 内部静态 import 了 abortAllInflight / loadGatewayConfig / setActiveGateway 等
const mocks = vi.hoisted(() => ({
  isTauri: vi.fn(() => false),
  loadGatewayConfig: vi.fn(),
  setActiveGateway: vi.fn(),
  saveGatewayConfig: vi.fn(),
  scanGatewayLan: vi.fn(),
  checkUrlHealth: vi.fn(),
  getInflightCount: vi.fn(() => 0),
  onInflightCountChange: vi.fn(() => () => { /* unsub */ }),
  abortAllInflight: vi.fn(),
  refreshAfterGatewaySwitch: vi.fn(),
}))

vi.mock('@/api', () => ({
  isTauri: mocks.isTauri,
  loadGatewayConfig: mocks.loadGatewayConfig,
  setActiveGateway: mocks.setActiveGateway,
  saveGatewayConfig: mocks.saveGatewayConfig,
  scanGatewayLan: mocks.scanGatewayLan,
  checkUrlHealth: mocks.checkUrlHealth,
  getInflightCount: mocks.getInflightCount,
  onInflightCountChange: mocks.onInflightCountChange,
  abortAllInflight: mocks.abortAllInflight,
  refreshAfterGatewaySwitch: mocks.refreshAfterGatewaySwitch,
}))

import { useGatewaySwitcher } from './useGatewaySwitcher'

/** 宿主组件：把 composable 暴露到 window 上供断言 */
function harness() {
  let api: ReturnType<typeof useGatewaySwitcher> | null = null
  const Comp = defineComponent({
    setup() {
      api = useGatewaySwitcher()
      return () => h('div')
    },
  })
  const wrapper = mount(Comp)
  return { wrapper, get api() { return api! } }
}

describe('useGatewaySwitcher', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.unstubAllGlobals()
    // 重设 mock 实现（vi.clearAllMocks 只清调用历史，mockReturnValue 仍在）
    mocks.isTauri.mockReset().mockImplementation(() => false)
    mocks.getInflightCount.mockReset().mockReturnValue(0)
    mocks.onInflightCountChange.mockReset().mockImplementation(() => () => { /* unsub */ })
  })

  it('Web 模式下 isTauriEnv=false，且 refreshGateways 不调用 loadGatewayConfig', async () => {
    const { api } = harness()
    expect(api.isTauriEnv).toBe(false)
    await api.refreshGateways()
    expect(mocks.loadGatewayConfig).not.toHaveBeenCalled()
  })

  it('Tauri 模式下 refreshGateways 拉配置并填充当前激活', async () => {
    mocks.isTauri.mockReturnValue(true)
    mocks.loadGatewayConfig.mockResolvedValue({
      gateways: [
        { name: 'gw-a', url: 'http://a.local:8080' },
        { name: 'gw-b', url: 'http://b.local:8080' },
      ],
      active: 'gw-b',
    })
    const { api } = harness()
    await api.refreshGateways()
    expect(api.gateways.value).toHaveLength(2)
    expect(api.activeGatewayName.value).toBe('gw-b')
    expect(api.activeGatewayUrl.value).toBe('http://b.local:8080')
  })

  it('Tauri 模式下订阅 in-flight 计数；Web 模式不订阅', () => {
    mocks.isTauri.mockReturnValue(true)
    harness()
    expect(mocks.onInflightCountChange).toHaveBeenCalledTimes(1)

    vi.clearAllMocks()
    mocks.isTauri.mockReturnValue(false)
    harness()
    expect(mocks.onInflightCountChange).not.toHaveBeenCalled()
  })

  it('Web 模式下探测走 fetch + AbortController', async () => {
    const fetchMock = vi.fn().mockResolvedValue({ ok: true })
    vi.stubGlobal('fetch', fetchMock)
    const { api } = harness()
    // 直接调内部 probeNewGateway（已暴露给测试），避免 performGatewaySwitch
    // 路径里其他副作用（loading 提示、reload）干扰断言
    const ok = await api.probeNewGateway('http://x.local:8080/')
    expect(ok).toBe(true)
    expect(fetchMock).toHaveBeenCalled()
    const [url, init] = fetchMock.mock.calls[0]!
    expect(url).toBe('http://x.local:8080/health/live')
    expect(init?.signal).toBeDefined()
  })

  it('扫描弹窗默认关闭，performLanScan 后打开（仅 Tauri）', async () => {
    mocks.isTauri.mockReturnValue(true)
    mocks.scanGatewayLan.mockResolvedValue([])
    const { api } = harness()
    expect(api.showScanDialog.value).toBe(false)
    await api.performLanScan()
    await nextTick()
    expect(api.showScanDialog.value).toBe(true)
    expect(api.scanResults.value).toEqual([])
    expect(api.isScanning.value).toBe(false)
    expect(mocks.scanGatewayLan).toHaveBeenCalledWith(8080)
  })

  it('closeScanDialog 仅关闭弹窗不清空结果', () => {
    mocks.isTauri.mockReturnValue(true)
    const { api } = harness()
    api.scanResults.value = [{ ip: '1.2.3.4', port: 8080, name: 'x', latency_ms: 5 }]
    api.showScanDialog.value = true
    api.closeScanDialog()
    expect(api.showScanDialog.value).toBe(false)
    expect(api.scanResults.value).toHaveLength(1)
  })

  it('切网关（performGatewaySwitch）并发阻断：第二次调用直接返回', async () => {
    mocks.isTauri.mockReturnValue(true)
    // 模拟 setActiveGateway 阻塞
    let release!: () => void
    mocks.setActiveGateway.mockImplementation(() => new Promise<void>((r) => { release = r }))
    mocks.loadGatewayConfig.mockResolvedValue({
      gateways: [{ name: 'new', url: 'http://new:8080' }],
      active: 'new',
    })
    // Tauri 下探测走 Rust 的 check_url_health（须可达才能进入持久化环节）
    mocks.checkUrlHealth.mockResolvedValue({ reachable: true })

    const { api } = harness()
    const p1 = api.performGatewaySwitch('new')
    const p2 = api.performGatewaySwitch('new')
    // 流程里有 drain → 探测 → 持久化 三段 await，等持久化真正发起再断言
    await vi.waitFor(() => expect(mocks.setActiveGateway).toHaveBeenCalledTimes(1))
    expect(api.isSwitchingGateway.value).toBe(true)
    release()
    await p1
    await p2
    // 第二次调用被 isSwitchingGateway 挡住，没有再发起持久化
    expect(mocks.setActiveGateway).toHaveBeenCalledTimes(1)
  })

  it('addDiscoveredGateway URL 已存在则不重复添加', async () => {
    mocks.isTauri.mockReturnValue(true)
    mocks.loadGatewayConfig.mockResolvedValue({
      gateways: [{ name: 'dup', url: 'http://1.2.3.4:8080' }],
      active: 'dup',
    })
    const { api } = harness()
    await api.refreshGateways()
    await api.addDiscoveredGateway({ ip: '1.2.3.4', port: 8080, name: 'dup', latency_ms: 3 })
    expect(mocks.saveGatewayConfig).not.toHaveBeenCalled()
  })

  /**
   * 回归：新网关不可达时，配置与刷新都不应发生。
   *
   * 原实现顺序是「先 setActiveGateway（内部就启动 200ms 后的 reload）→ 再探测」，
   * 目标不可达时页面已经开始刷新：错误提示来不及展示，配置已指向坏网关，
   * 且没有回滚路径——用户被刷新到一个不可达的网关上。
   */
  it('探测不通过：不持久化、不刷新页面', async () => {
    mocks.isTauri.mockReturnValue(true)
    mocks.loadGatewayConfig.mockResolvedValue({
      gateways: [{ name: 'old', url: 'http://old:8080' }],
      active: 'old',
    })
    // Tauri 下探测走 Rust 的 check_url_health
    mocks.checkUrlHealth.mockResolvedValue({ reachable: false })
    const { api } = harness()

    await api.performGatewaySwitch('bad', 'http://bad:8080')

    // 关键断言：持久化与刷新都没发生
    expect(mocks.setActiveGateway).not.toHaveBeenCalled()
    expect(mocks.refreshAfterGatewaySwitch).not.toHaveBeenCalled()
  })

  /**
   * 回归：探测通过后才持久化并刷新，且顺序是「探测 → 持久化 → 刷新」。
   */
  it('探测通过：先探测再持久化，最后刷新页面', async () => {
    mocks.isTauri.mockReturnValue(true)
    mocks.loadGatewayConfig.mockResolvedValue({
      gateways: [{ name: 'new', url: 'http://new:8080' }],
      active: 'new',
    })

    const order: string[] = []
    mocks.checkUrlHealth.mockImplementation(async () => {
      order.push('probe')
      return { reachable: true }
    })
    mocks.setActiveGateway.mockImplementation(async () => {
      order.push('persist')
    })
    mocks.refreshAfterGatewaySwitch.mockImplementation(() => {
      order.push('refresh')
    })

    const { api } = harness()
    await api.performGatewaySwitch('new', 'http://new:8080')

    expect(order).toEqual(['probe', 'persist', 'refresh'])
    expect(mocks.setActiveGateway).toHaveBeenCalledWith('new')
    expect(mocks.refreshAfterGatewaySwitch).toHaveBeenCalledTimes(1)
  })
})