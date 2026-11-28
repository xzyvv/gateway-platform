/**
 * 共享调试目标（审查#5）测试。
 *
 * 注意：selectedDeviceId / selectedBus 是模块级单例并读 localStorage 初始化，
 * 每条用例需 vi.resetModules() 后重新 import 才能得到干净的单例状态；
 * localStorage 预置值必须在 import 之前写入（模块求值时读取）。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'
import { scopedKey, scopedSet } from '@/utils/storage/gatewayStorage'

vi.mock('@/api/acquisition/devices', () => ({ listDevices: vi.fn() }))
vi.mock('@/api/uplink/config', () => ({ getConfig: vi.fn() }))

const DEVICES = [
  { id: 'dev_1', name: '一号机' },
  { id: 'dev_2', name: '二号机' },
]
const BUSES = [
  { port_name: '/dev/ttyUSB0', protocol: 'modbus-rtu' },
  { port_name: '192.168.1.50:502', protocol: 'modbus-tcp', tcp_addr: '192.168.1.50:502' },
]

/** 重置模块缓存、配置好 mock 返回值后重新加载 composable 模块 */
async function setup() {
  vi.resetModules()
  const devicesApi = await import('@/api/acquisition/devices')
  const configApi = await import('@/api/uplink/config')
  vi.mocked(devicesApi.listDevices).mockResolvedValue(DEVICES as never)
  vi.mocked(configApi.getConfig).mockResolvedValue({ buses: BUSES } as never)
  const mod = await import('./useDebugTargets')
  return { ...mod, devicesApi, configApi }
}

/** 挂载一个调用 useDebugTargets 的空组件，返回 composable 暴露的 API */
function mountTargets(useDebugTargets: Awaited<ReturnType<typeof setup>>['useDebugTargets']) {
  let api: ReturnType<typeof useDebugTargets> | undefined
  mount(defineComponent({
    setup() {
      api = useDebugTargets()
      return () => h('div')
    },
  }))
  return () => api!
}

beforeEach(() => {
  localStorage.clear()
})

describe('useDebugTargets 共享调试目标', () => {
  it('选项加载后默认选中第一台设备 / 第一条总线', async () => {
    const { useDebugTargets } = await setup()
    const get = mountTargets(useDebugTargets)
    await flushPromises()
    expect(get().selectedDeviceId.value).toBe('dev_1')
    expect(get().selectedBus.value).toBe('/dev/ttyUSB0')
  })

  it('跨组件实例共享同一选择，并持久化到 localStorage', async () => {
    const { useDebugTargets, useSharedDebugTarget } = await setup()
    const a = mountTargets(useDebugTargets)
    let shared: ReturnType<typeof useSharedDebugTarget> | undefined
    mount(defineComponent({
      setup() {
        shared = useSharedDebugTarget()
        return () => h('div')
      },
    }))
    await flushPromises()

    a().selectedDeviceId.value = 'dev_2'
    a().selectedBus.value = '192.168.1.50:502'
    await flushPromises()

    expect(shared!.selectedDeviceId.value).toBe('dev_2')
    expect(shared!.selectedBus.value).toBe('192.168.1.50:502')
    expect(localStorage.getItem(scopedKey('gateway.debug-target.device-id'))).toBe('dev_2')
    expect(localStorage.getItem(scopedKey('gateway.debug-target.bus'))).toBe('192.168.1.50:502')
  })

  it('持久化的有效选择被保留，不强制改选第一台', async () => {
    scopedSet('gateway.debug-target.device-id', 'dev_2')
    const { useDebugTargets } = await setup()
    const get = mountTargets(useDebugTargets)
    await flushPromises()
    expect(get().selectedDeviceId.value).toBe('dev_2')
  })

  it('持久化值已失效（设备被删/配置变更）时回退到第一个可选项', async () => {
    scopedSet('gateway.debug-target.device-id', 'ghost')
    scopedSet('gateway.debug-target.bus', 'COM99')
    const { useDebugTargets } = await setup()
    const get = mountTargets(useDebugTargets)
    await flushPromises()
    expect(get().selectedDeviceId.value).toBe('dev_1')
    expect(get().selectedBus.value).toBe('/dev/ttyUSB0')
  })

  it('useSharedDebugTarget 只读写共享选择，不触发设备/配置加载', async () => {
    const { useSharedDebugTarget, devicesApi, configApi } = await setup()
    mount(defineComponent({
      setup() {
        useSharedDebugTarget()
        return () => h('div')
      },
    }))
    await flushPromises()
    expect(devicesApi.listDevices).not.toHaveBeenCalled()
    expect(configApi.getConfig).not.toHaveBeenCalled()
  })

  /**
   * 选项列表是模块级单例：一个页面上常有两处调用（Hub 的 ConsoleTargetBar +
   * 页面本身），此前每处各打一轮 GET /api/devices + GET /api/config。
   */
  it('同一时刻多个组件挂载，只发一轮请求（单飞）', async () => {
    const { useDebugTargets, devicesApi, configApi } = await setup()
    // 请求挂起，制造「两个组件同时挂载」的时间窗
    let release: (() => void) | null = null
    vi.mocked(devicesApi.listDevices).mockImplementation(
      () => new Promise((resolve) => { release = () => resolve(DEVICES as never) }),
    )

    mountTargets(useDebugTargets)
    mountTargets(useDebugTargets)
    await flushPromises()

    expect(devicesApi.listDevices).toHaveBeenCalledTimes(1)
    expect(configApi.getConfig).toHaveBeenCalledTimes(1)

    release!()
    await flushPromises()
  })

  it('加载完成后新挂载的页面复用单例选项，不再请求', async () => {
    const { useDebugTargets, devicesApi, configApi } = await setup()
    const first = mountTargets(useDebugTargets)
    await flushPromises()
    expect(devicesApi.listDevices).toHaveBeenCalledTimes(1)

    // 模拟切到另一个 tab：新组件挂载，应直接复用已加载的选项
    const second = mountTargets(useDebugTargets)
    await flushPromises()
    expect(devicesApi.listDevices).toHaveBeenCalledTimes(1)
    expect(configApi.getConfig).toHaveBeenCalledTimes(1)
    expect(second().busOptions.value).toHaveLength(BUSES.length)
    expect(first().devices.value).toHaveLength(DEVICES.length)
  })

  it('快照超过 TTL 后新挂载会重新拉取，配置变更不会被长期遮住', async () => {
    const { useDebugTargets, devicesApi } = await setup()
    const now = vi.spyOn(Date, 'now')
    try {
      now.mockReturnValue(1_000_000)
      mountTargets(useDebugTargets)
      await flushPromises()
      expect(devicesApi.listDevices).toHaveBeenCalledTimes(1)

      // 刚过 TTL：应重新拉取（否则新建设备后进扫描页看不到）
      now.mockReturnValue(1_000_000 + 5_001)
      mountTargets(useDebugTargets)
      await flushPromises()
      expect(devicesApi.listDevices).toHaveBeenCalledTimes(2)
    } finally {
      now.mockRestore()
    }
  })

  it('reload 强制重新拉取，不因已加载而跳过', async () => {
    const { useDebugTargets, devicesApi } = await setup()
    const get = mountTargets(useDebugTargets)
    await flushPromises()
    expect(devicesApi.listDevices).toHaveBeenCalledTimes(1)

    // 调试向导的「总线检查」会 await reload() 后按最新配置判定
    vi.mocked(devicesApi.listDevices).mockResolvedValue([{ id: 'dev_3', name: '三号机' }] as never)
    await get().reload()

    expect(devicesApi.listDevices).toHaveBeenCalledTimes(2)
    expect(get().devices.value).toHaveLength(1)
  })

  it('加载失败不清空已持久化的选择，且下次挂载自动重试', async () => {
    scopedSet('gateway.debug-target.device-id', 'dev_2')
    const { useDebugTargets, devicesApi } = await setup()
    vi.mocked(devicesApi.listDevices).mockRejectedValue(new Error('network down'))

    const first = mountTargets(useDebugTargets)
    await flushPromises()
    // 没有真实选项可供校正：「加载失败」不等于「选择失效」，不能清掉持久值
    expect(first().selectedDeviceId.value).toBe('dev_2')
    expect(first().devices.value).toHaveLength(0)

    // 后端恢复后，下一次挂载（如重新进入页面）自动重试成功
    vi.mocked(devicesApi.listDevices).mockResolvedValue(DEVICES as never)
    const second = mountTargets(useDebugTargets)
    await flushPromises()
    expect(devicesApi.listDevices).toHaveBeenCalledTimes(2)
    expect(second().devices.value).toHaveLength(DEVICES.length)
    expect(second().selectedDeviceId.value).toBe('dev_2')
  })
})
