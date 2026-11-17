/**
 * 协议能力门控（`useProtocolCaps`）契约。
 *
 * 这个 composable 是「调试工具按钮是否可点」的唯一判据：从站扫描 / 寄存器扫描 /
 * 参数探测三个页签都按能力位渲染入口，后端注册表新增能力即为前端新增入口。
 *
 * 因此最关键的一条口径是：**能力未知 ≠ 具备能力**。
 * 元数据加载失败或协议未注册时一律返回 null（调用方按「不支持」处理），
 * 绝不能放行一个「点了必然失败」的按钮——那会让运维把网关故障误判成设备故障。
 *
 * 另两条：
 * - `loaded` 用来区分「还在加载」与「确实不支持」，两者在 UI 上必须能分开；
 * - `failed` 只表示本次加载出错，不代表能力为空，二者不可混用。
 *
 * 注意：caps / loaded / failed 是**模块级单例**（协议注册表在一个网关进程内静态不变），
 * 每条用例都要 `vi.resetModules()` 后重新 import 才能拿到干净的单例状态。
 */
import { describe, it, expect, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('@/api/protocols', () => ({
  listProtocolCapabilities: vi.fn(),
}))

/** 构造一份完整的 8 位能力位，避免测试只关心其中一位时漏填其它位 */
function caps(overrides: Partial<Record<string, boolean>> = {}) {
  return {
    supports_batch_read: true,
    supports_frame_extract: false,
    supports_write: false,
    supports_scan_slaves: false,
    supports_register_scan: false,
    supports_auto_detect: false,
    supports_write_multi: false,
    supports_diag: true,
    ...overrides,
  }
}

const CAPS_RESPONSE = [
  { name: 'modbus', capabilities: caps({ supports_scan_slaves: true, supports_register_scan: true, supports_write: true }) },
  { name: 'iec104', capabilities: caps({ supports_write: true }) },
  { name: 'dlt645', capabilities: caps({ supports_frame_extract: true, supports_write: true }) },
]

type CapsModule = typeof import('./useProtocolCaps')
type Api = ReturnType<CapsModule['useProtocolCaps']>

/**
 * 重置模块缓存并重新加载 composable 模块，拿到干净的单例状态。
 * 返回模块本身与 mock 好的接口（便于断言请求次数）。
 */
async function setup() {
  vi.resetModules()
  const protocolsApi = await import('@/api/protocols')
  const mod = await import('./useProtocolCaps')
  return { mod, protocolsApi }
}

/**
 * 挂载一个调用 useProtocolCaps 的空组件。
 * 必须走组件上下文：`onMounted(load)` 只在组件内注册，直接在测试里裸调
 * composable 不会触发自动加载（Vue 会告警且钩子不执行）。
 */
function mountCaps(useProtocolCaps: CapsModule['useProtocolCaps']) {
  let api: Api | undefined
  mount(defineComponent({
    setup() {
      api = useProtocolCaps()
      return () => h('div')
    },
  }))
  return () => api!
}

describe('useProtocolCaps', () => {
  it('加载成功后按协议名建映射，能力位原样透传', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities).mockResolvedValue(CAPS_RESPONSE as never)
    const api = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(api().loaded.value).toBe(true)
    expect(api().failed.value).toBe(false)
    expect(Object.keys(api().caps.value).sort()).toEqual(['dlt645', 'iec104', 'modbus'])
    expect(api().capsOf('modbus')?.supports_scan_slaves).toBe(true)
    expect(api().capsOf('modbus')?.supports_register_scan).toBe(true)
    // 未声明的能力位保持 false，而不是缺字段/undefined
    expect(api().capsOf('iec104')?.supports_scan_slaves).toBe(false)
  })

  it('加载失败时标记 failed，并把所有协议按「不支持」处理', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities).mockRejectedValue(new Error('network down'))
    const api = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(api().loaded.value).toBe(true)
    expect(api().failed.value).toBe(true)
    // 关键口径：元数据拿不到就不能放行按钮
    expect(api().capsOf('modbus')).toBeNull()
    expect(api().caps.value).toEqual({})
  })

  it('协议未注册时返回 null（按不支持处理），不返回 undefined 之外的东西', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities).mockResolvedValue(CAPS_RESPONSE as never)
    const api = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(api().capsOf('opcua')).toBeNull()
    expect(api().capsOf('')).toBeNull()
  })

  it('加载前 loaded=false，调用方据此区分「加载中」与「不支持」', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities).mockReturnValue(new Promise(() => {}) as never)
    const api = mountCaps(mod.useProtocolCaps)

    expect(api().loaded.value).toBe(false)
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')).toBeNull()
  })

  it('load 可手动重试：首次失败后重试成功，failed 复位', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities)
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce(CAPS_RESPONSE as never)
    const api = mountCaps(mod.useProtocolCaps)
    await flushPromises()
    expect(api().failed.value).toBe(true)

    await api().load()
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')?.supports_write).toBe(true)
  })

  it('后端返回空列表时不算失败（网关无协议也算正常态）', async () => {
    const { mod, protocolsApi } = await setup()
    vi.mocked(protocolsApi.listProtocolCapabilities).mockResolvedValue([] as never)
    const api = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(api().loaded.value).toBe(true)
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')).toBeNull()
  })

  /**
   * 单例契约：同一页面上三个调用方（useRegisterScan / useAutoDetect / useProtocolGates）
   * 只应产生一次 `GET /api/protocols`。
   *
   * 旧实现状态建在函数内部且每挂载一次就 load，一个调试台页签会连打 3 轮同一请求；
   * 后来者还看不到先到者的结果，白等一轮网络往返。
   *
   * 变异验证：把 caps/loaded 挪回 useProtocolCaps() 内部 → 用例红（请求数变 3）。
   */
  it('单例：多处同时挂载只打一次接口，后挂载者复用结果不再请求', async () => {
    const { mod, protocolsApi } = await setup()
    const spy = vi.mocked(protocolsApi.listProtocolCapabilities)
    spy.mockResolvedValue(CAPS_RESPONSE as never)

    // 三个组件同时挂载（模拟同一页面的三个调试入口）
    const a = mountCaps(mod.useProtocolCaps)
    const b = mountCaps(mod.useProtocolCaps)
    const c = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(spy).toHaveBeenCalledTimes(1)
    // 三家拿到的是同一份数据，且都已完成加载
    expect(a().capsOf('modbus')?.supports_scan_slaves).toBe(true)
    expect(b().loaded.value).toBe(true)
    expect(c().capsOf('iec104')?.supports_write).toBe(true)

    // 已加载成功后再挂载（如切到另一个调试页签）不应重复请求
    const d = mountCaps(mod.useProtocolCaps)
    await flushPromises()
    expect(spy).toHaveBeenCalledTimes(1)
    expect(d().capsOf('modbus')?.supports_write).toBe(true)
  })

  /**
   * 失败后下一次挂载要自动重试：用户常常是「先开了页面发现没数据，
   * 去修好网络再回来」，若 loaded 一旦置真就永不重试，回来仍是满屏灰按钮。
   */
  it('加载失败后下一次挂载自动重试，成功后 failed 复位', async () => {
    const { mod, protocolsApi } = await setup()
    const spy = vi.mocked(protocolsApi.listProtocolCapabilities)
    spy.mockRejectedValueOnce(new Error('network down'))
    mountCaps(mod.useProtocolCaps)
    await flushPromises()
    expect(spy).toHaveBeenCalledTimes(1)

    spy.mockResolvedValueOnce(CAPS_RESPONSE as never)
    const second = mountCaps(mod.useProtocolCaps)
    await flushPromises()

    expect(spy).toHaveBeenCalledTimes(2)
    expect(second().failed.value).toBe(false)
    expect(second().capsOf('modbus')?.supports_scan_slaves).toBe(true)
  })
})
