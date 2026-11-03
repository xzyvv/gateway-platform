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
import { scopedKey, scopedSet } from '@/utils/gatewayStorage'

vi.mock('@/api/devices', () => ({ listDevices: vi.fn() }))
vi.mock('@/api/config', () => ({ getConfig: vi.fn() }))

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
  const devicesApi = await import('@/api/devices')
  const configApi = await import('@/api/config')
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
})
