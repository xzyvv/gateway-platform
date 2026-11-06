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
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { defineComponent, h } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('@/api/protocols', () => ({
  listProtocolCapabilities: vi.fn(),
}))

import { listProtocolCapabilities } from '@/api/protocols'
import { useProtocolCaps } from './useProtocolCaps'

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

/**
 * 挂载一个调用 useProtocolCaps 的空组件。
 * 必须走组件上下文：`onMounted(load)` 只在组件内注册，直接在测试里裸调
 * composable 不会触发自动加载（Vue 会告警且钩子不执行）。
 */
function mountCaps() {
  let api: ReturnType<typeof useProtocolCaps> | undefined
  mount(defineComponent({
    setup() {
      api = useProtocolCaps()
      return () => h('div')
    },
  }))
  return () => api!
}

describe('useProtocolCaps', () => {
  beforeEach(() => {
    vi.mocked(listProtocolCapabilities).mockReset()
  })

  it('加载成功后按协议名建映射，能力位原样透传', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(CAPS_RESPONSE as never)
    const api = mountCaps()
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
    vi.mocked(listProtocolCapabilities).mockRejectedValue(new Error('network down'))
    const api = mountCaps()
    await flushPromises()

    expect(api().loaded.value).toBe(true)
    expect(api().failed.value).toBe(true)
    // 关键口径：元数据拿不到就不能放行按钮
    expect(api().capsOf('modbus')).toBeNull()
    expect(api().caps.value).toEqual({})
  })

  it('协议未注册时返回 null（按不支持处理），不返回 undefined 之外的东西', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(CAPS_RESPONSE as never)
    const api = mountCaps()
    await flushPromises()

    expect(api().capsOf('opcua')).toBeNull()
    expect(api().capsOf('')).toBeNull()
  })

  it('加载前 loaded=false，调用方据此区分「加载中」与「不支持」', () => {
    vi.mocked(listProtocolCapabilities).mockReturnValue(new Promise(() => {}) as never)
    const api = mountCaps()

    expect(api().loaded.value).toBe(false)
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')).toBeNull()
  })

  it('load 可手动重试：首次失败后重试成功，failed 复位', async () => {
    vi.mocked(listProtocolCapabilities)
      .mockRejectedValueOnce(new Error('boom'))
      .mockResolvedValueOnce(CAPS_RESPONSE as never)
    const api = mountCaps()
    await flushPromises()
    expect(api().failed.value).toBe(true)

    await api().load()
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')?.supports_write).toBe(true)
  })

  it('后端返回空列表时不算失败（网关无协议也算正常态）', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue([] as never)
    const api = mountCaps()
    await flushPromises()

    expect(api().loaded.value).toBe(true)
    expect(api().failed.value).toBe(false)
    expect(api().capsOf('modbus')).toBeNull()
  })
})
