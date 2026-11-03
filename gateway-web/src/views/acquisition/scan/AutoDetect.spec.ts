import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

vi.mock('@/api/protocolTools', () => ({
  startAutoDetect: vi.fn(),
  getAutoDetectResult: vi.fn(),
  stopAutoDetect: vi.fn(),
}))
vi.mock('@/api/protocols', () => ({
  listProtocolCapabilities: vi.fn(),
}))
vi.mock('@/composables/acquisition/useDebugTargets', () => ({
  useDebugTargets: () => ({
    devices: ref([{ id: 'dev_1', name: '1号表', protocol: 'modbus' }]),
    deviceOptions: ref([{ label: '1号表 (dev_1)', value: 'dev_1' }]),
    loading: ref(false),
    selectedDeviceId: ref('dev_1'),
  }),
}))
vi.mock('vue-router', () => ({ useRouter: () => ({ push: vi.fn() }) }))

import AutoDetect from './AutoDetect.vue'
import { listProtocolCapabilities } from '@/api/protocols'

/**
 * B3：参数探测入口必须由协议能力位（supports_auto_detect）驱动，
 * 而不是按协议名硬编码——能力声明为 false / 元数据缺失时按钮不可用。
 */
function capabilityList(supportsAutoDetect: boolean) {
  return [
    {
      name: 'modbus',
      capabilities: {
        supports_batch_read: true,
        supports_frame_extract: true,
        supports_write: true,
        supports_scan_slaves: true,
        supports_register_scan: true,
        supports_auto_detect: supportsAutoDetect,
        supports_write_multi: false,
        supports_diag: false,
      },
    },
  ]
}

function detectButton(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('button').find((b) => b.text().includes('开始探测'))
}

describe('AutoDetect 能力位门控（B3）', () => {
  it('supports_auto_detect=true：探测按钮可用', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(capabilityList(true))
    const wrapper = mount(AutoDetect)
    await flushPromises()
    expect(detectButton(wrapper)!.attributes('disabled')).toBeUndefined()
  })

  it('supports_auto_detect=false：探测按钮禁用并说明原因', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(capabilityList(false))
    const wrapper = mount(AutoDetect)
    await flushPromises()
    expect(detectButton(wrapper)!.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('不支持参数探测')
    // 口径钉子（C2）：协议语义上不适用 ≠ 尚未实现，现场不得误判为缺功能
    expect(wrapper.text()).toContain('参数探测不适用（不是尚未实现）')
  })

  it('能力元数据加载失败：按不支持处理，保守禁用', async () => {
    vi.mocked(listProtocolCapabilities).mockRejectedValue(new Error('network'))
    const wrapper = mount(AutoDetect)
    await flushPromises()
    expect(detectButton(wrapper)!.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('协议能力元数据加载失败')
  })
})
