import { describe, it, expect } from 'vitest'
import {
  protocolDefaultTimeoutMs,
  isProtocolDefaultTimeout,
  DEFAULT_PROTOCOL_TIMEOUT_MS,
  DEFAULT_TAG_COLOR,
  applyProtocolMetadata,
  protocolOptions,
  protocolLabel,
  protocolAddressPlaceholder,
  protocolType,
  protocolTransportOf,
  protocolEndpointPlaceholder,
  protocolVariantsOf,
} from './useProtocol'

/**
 * useProtocol 默认超时映射单元测试：
 * - 串行表计总线取 1s，TCP 类协议取 3s
 * - 未登记协议回落到兜底默认
 * - isProtocolDefaultTimeout 用于协议切换时判断「是否仍为默认值」
 */
describe('protocolDefaultTimeoutMs', () => {
  it('串行表计总线默认 1s', () => {
    expect(protocolDefaultTimeoutMs('modbus')).toBe(1000)
    expect(protocolDefaultTimeoutMs('dlt645')).toBe(1000)
    expect(protocolDefaultTimeoutMs('cjt188')).toBe(1000)
  })

  it('TCP 类协议默认 3s', () => {
    expect(protocolDefaultTimeoutMs('iec104')).toBe(3000)
    expect(protocolDefaultTimeoutMs('s7comm')).toBe(3000)
  })

  it('未登记协议回落兜底默认', () => {
    expect(protocolDefaultTimeoutMs('unknown-proto')).toBe(DEFAULT_PROTOCOL_TIMEOUT_MS)
  })
})

describe('isProtocolDefaultTimeout', () => {
  it('命中任一协议默认值为 true', () => {
    expect(isProtocolDefaultTimeout(1000)).toBe(true)
    expect(isProtocolDefaultTimeout(3000)).toBe(true)
  })

  it('自定义值不视为默认', () => {
    expect(isProtocolDefaultTimeout(800)).toBe(false)
    expect(isProtocolDefaultTimeout(5000)).toBe(false)
  })
})

describe('applyProtocolMetadata', () => {
  const caps = {
    supports_batch_read: true,
    supports_frame_extract: false,
    supports_write: true,
    supports_scan_slaves: false,
    supports_register_scan: false,
    supports_auto_detect: false,
    supports_write_multi: false,
    supports_diag: false,
  }

  it('以注册表元数据刷新下拉/标签/默认超时/地址提示，并过滤点位级协议', () => {
    applyProtocolMetadata([
      {
        name: 's7comm',
        label: '西门子 S7',
        transport: 'tcp',
        default_timeout_ms: 2500,
        address_hint: '如: 192.168.1.10:102',
        tag_color: 'success',
        capabilities: caps,
      },
      {
        name: 'gpio',
        label: 'GPIO',
        transport: 'local',
        default_timeout_ms: 1000,
        address_hint: '如: 17（GPIO 编号）',
        tag_color: 'info',
        capabilities: caps,
      },
    ])

    // gpio 是点位级协议，不作为设备/总线选项
    expect(protocolOptions.map((o) => o.value)).toEqual(['s7comm'])
    expect(protocolOptions[0]?.label).toBe('西门子 S7')
    expect(protocolLabel.s7comm).toBe('西门子 S7')
    expect(protocolDefaultTimeoutMs('s7comm')).toBe(2500)
    expect(protocolAddressPlaceholder('s7comm')).toBe('如: 192.168.1.10:102')
    expect(protocolType('s7comm')).toBe('success')
  })

  it('空列表保持现有协议清单', () => {
    const before = protocolOptions.length
    applyProtocolMetadata([])
    expect(protocolOptions.length).toBe(before)
    expect(protocolOptions.length).toBeGreaterThan(0)
  })
})

describe('protocolType（P5：配色由后端注册表驱动）', () => {
  const caps = {
    supports_batch_read: true,
    supports_frame_extract: false,
    supports_write: true,
    supports_scan_slaves: false,
    supports_register_scan: false,
    supports_auto_detect: false,
    supports_write_multi: false,
    supports_diag: false,
  }

  it('后端下发的 tag_color 直接生效', () => {
    applyProtocolMetadata([
      { name: 'myproto', label: '自定义协议', transport: 'tcp', default_timeout_ms: 3000,
        address_hint: '如: 10.0.0.1:1', tag_color: 'danger', capabilities: caps },
    ])
    expect(protocolType('myproto')).toBe('danger')
  })

  it('未登记协议回落到默认色', () => {
    expect(protocolType('never-registered')).toBe(DEFAULT_TAG_COLOR)
  })
})

describe('链路形态 / 端点提示 / 变体（C1：协议配置面对齐）', () => {
  const caps = {
    supports_batch_read: true,
    supports_frame_extract: false,
    supports_write: true,
    supports_scan_slaves: false,
    supports_register_scan: false,
    supports_auto_detect: false,
    supports_write_multi: false,
    supports_diag: false,
  }

  it('后端下发的 transport / endpoint_hint / variants 生效', () => {
    applyProtocolMetadata([
      {
        name: 'fins',
        label: '欧姆龙 FINS',
        transport: 'tcp',
        default_timeout_ms: 3000,
        address_hint: '如: D100',
        endpoint_hint: '如: 10.0.0.2:9600（UDP 可追加 ?node=NN）',
        variants: [
          { value: 'tcp', label: 'TCP' },
          { value: 'udp', label: 'UDP' },
        ],
        capabilities: caps,
      },
    ])
    expect(protocolTransportOf('fins')).toBe('tcp')
    expect(protocolEndpointPlaceholder('fins')).toBe('如: 10.0.0.2:9600（UDP 可追加 ?node=NN）')
    expect(protocolVariantsOf('fins').map((v) => v.value)).toEqual(['tcp', 'udp'])
  })

  it('未登记协议按串行处理，且无变体项', () => {
    expect(protocolTransportOf('never-registered')).toBe('serial')
    expect(protocolVariantsOf('never-registered')).toEqual([])
  })
})
