/**
 * 设备 → 协议能力门控测试。
 *
 * 核心要钉住的是**能力未知时的保守口径**：元数据缺失（未加载 / 加载失败 /
 * 未知协议）一律按「不支持」处理，绝不因为「没加载到」就放行。放行的后果是
 * 用户点到一个必然返回 Unsupported 的按钮，然后去怀疑设备坏了。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { defineComponent, h, ref, nextTick } from 'vue'
import { mount, flushPromises } from '@vue/test-utils'

vi.mock('@/api/acquisition/protocols', () => ({ listProtocolCapabilities: vi.fn() }))

import { listProtocolCapabilities } from '@/api/acquisition/protocols'
import { useProtocolGates } from './useProtocolGates'
import { resetProtocolCapsCache } from './useProtocolCaps'
import type { DeviceConfig } from '@/api/types'

const CAPS = {
  supports_write: true,
  supports_batch_read: true,
  supports_scan_slaves: false,
  supports_diag: true,
  supports_frame_extract: true,
  supports_register_scan: false,
  supports_auto_detect: false,
  supports_write_multi: false,
}

const DEVICES = [
  { id: 'd1', name: '温控器', protocol: 'modbus-tcp' },
  { id: 'd2', name: '神秘协议设备', protocol: 'unknown-proto' },
] as unknown as DeviceConfig[]

/** 挂一个调用 useProtocolGates 的空组件，返回门控 API */
function mountGates(devices = ref(DEVICES), deviceId = ref('d1')) {
  let api: ReturnType<typeof useProtocolGates> | undefined
  mount(defineComponent({
    setup() {
      api = useProtocolGates(devices, deviceId)
      return () => h('div')
    },
  }))
  return { get: () => api!, deviceId }
}

beforeEach(() => {
    // 协议能力是模块级单例：清掉上一条用例灌进去的快照，否则本用例的 mock 返回值不会被重新请求
    resetProtocolCapsCache()
  vi.clearAllMocks()
  vi.mocked(listProtocolCapabilities).mockResolvedValue([
    { name: 'modbus-tcp', capabilities: CAPS },
  ] as never)
})

describe('useProtocolGates 协议能力门控', () => {
  it('元数据就绪后按声明返回各能力位', async () => {
    const { get } = mountGates()
    await flushPromises()

    expect(get().selectedProtocol.value).toBe('modbus-tcp')
    expect(get().capsKnown.value).toBe(true)
    expect(get().supportsWrite.value).toBe(true)
    expect(get().supportsBatchRead.value).toBe(true)
    expect(get().supportsScanSlaves.value).toBe(false)
    expect(get().supportsDiag.value).toBe(true)
  })

  it('元数据加载失败：capsKnown 为 false，能力位一律按不支持处理', async () => {
    vi.mocked(listProtocolCapabilities).mockRejectedValue(new Error('network down'))
    const { get } = mountGates()
    await flushPromises()

    expect(get().capsKnown.value).toBe(false)
    expect(get().supportsWrite.value).toBe(false)
    expect(get().supportsBatchRead.value).toBe(false)
    expect(get().supportsScanSlaves.value).toBe(false)
    expect(get().supportsDiag.value).toBe(false)
  })

  it('未选设备时协议名为空且能力位全 false', async () => {
    const { get } = mountGates(ref(DEVICES), ref(''))
    await flushPromises()

    expect(get().selectedProtocol.value).toBe('')
    expect(get().capsKnown.value).toBe(false)
    expect(get().supportsWrite.value).toBe(false)
  })

  it('协议在元数据里查不到：按能力未知处理，不推断为具备', async () => {
    const { get } = mountGates(ref(DEVICES), ref('d2'))
    await flushPromises()

    expect(get().selectedProtocol.value).toBe('unknown-proto')
    expect(get().capsKnown.value).toBe(false)
    expect(get().supportsWrite.value).toBe(false)
  })

  it('切换设备后门控跟随变化', async () => {
    const { get, deviceId } = mountGates()
    await flushPromises()
    expect(get().supportsWrite.value).toBe(true)

    deviceId.value = 'd2'
    await nextTick()
    expect(get().selectedProtocol.value).toBe('unknown-proto')
    expect(get().supportsWrite.value).toBe(false)
  })
})
