import { describe, it, expect, vi } from 'vitest'
import { mount, flushPromises } from '@vue/test-utils'
import { ref } from 'vue'

vi.mock('@/api/protocolTools', () => ({
  startScan: vi.fn(),
  getScanResult: vi.fn(),
  listScanResults: vi.fn().mockResolvedValue([]),
  deleteScanResult: vi.fn(),
  generatePointSuggestions: vi.fn().mockResolvedValue([]),
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

import RegisterScan from './RegisterScan.vue'
import { listProtocolCapabilities } from '@/api/protocols'

/**
 * B3：寄存器扫描入口必须由协议能力位（supports_register_scan）驱动，
 * 而不是按协议名硬编码——能力声明为 false / 元数据缺失时按钮不可用。
 */
function capabilityList(supportsRegisterScan: boolean) {
  return [
    {
      name: 'modbus',
      capabilities: {
        supports_batch_read: true,
        supports_frame_extract: true,
        supports_write: true,
        supports_scan_slaves: true,
        supports_register_scan: supportsRegisterScan,
        supports_auto_detect: true,
        supports_write_multi: false,
        supports_diag: false,
      },
    },
  ]
}

function scanButton(wrapper: ReturnType<typeof mount>) {
  return wrapper.findAll('button').find((b) => b.text().includes('开始扫描'))
}

describe('RegisterScan 能力位门控（B3）', () => {
  it('supports_register_scan=true：扫描按钮可用', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(capabilityList(true))
    const wrapper = mount(RegisterScan)
    await flushPromises()
    expect(scanButton(wrapper)!.attributes('disabled')).toBeUndefined()
  })

  it('supports_register_scan=false：扫描按钮禁用并说明原因', async () => {
    vi.mocked(listProtocolCapabilities).mockResolvedValue(capabilityList(false))
    const wrapper = mount(RegisterScan)
    await flushPromises()
    expect(scanButton(wrapper)!.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('不支持寄存器扫描')
    // 口径钉子（C2）：协议语义上不适用 ≠ 尚未实现，现场不得误判为缺功能
    expect(wrapper.text()).toContain('寄存器扫描不适用（不是尚未实现）')
  })

  it('能力元数据加载失败：按不支持处理，保守禁用', async () => {
    vi.mocked(listProtocolCapabilities).mockRejectedValue(new Error('network'))
    const wrapper = mount(RegisterScan)
    await flushPromises()
    expect(scanButton(wrapper)!.attributes('disabled')).toBeDefined()
    expect(wrapper.text()).toContain('协议能力元数据加载失败')
  })
})
