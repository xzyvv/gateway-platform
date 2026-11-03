import { describe, it, expect } from 'vitest'

// 只测纯函数：api/store 依赖在 import 期不执行请求，无需 mock
import { buildConfigSnapshot, buildPacketsCsv } from './useDiagnosticExport'
import type { GatewayConfig } from '@/api/types'
import type { Packet } from '@/api/protocolTools/types'

function pkt(over: Partial<Packet> = {}): Packet {
  return {
    id: 'pkt_1',
    time: new Date('2026-10-26T08:00:00').getTime(),
    direction: 'request',
    deviceId: 'dev1',
    deviceName: '电表1号',
    bus: 'rs485_a',
    hex: '01 03 00 00 00 02 C4 0B',
    ascii: '',
    parsed: 'func=0x03 addr=0x0000',
    ...over,
  }
}

describe('buildPacketsCsv（诊断包 packets.csv）', () => {
  it('空列表：仅 BOM + 表头', () => {
    const csv = buildPacketsCsv([])
    expect(csv.startsWith('\uFEFF')).toBe(true)
    expect(csv).toContain('id,time,direction,bus,device,hex,parsed')
    expect(csv.trimEnd().split('\n')).toHaveLength(1)
  })

  it('字段顺序与方向/总线/报文内容正确', () => {
    const csv = buildPacketsCsv([
      pkt(),
      pkt({ id: 'pkt_2', direction: 'response', hex: '01 03 04 00 64 B8 41', parsed: 'ok' }),
    ])
    const lines = csv.trimEnd().split('\n')
    expect(lines).toHaveLength(3)
    expect(lines[1]).toContain('pkt_1,')
    expect(lines[1]).toContain(',request,rs485_a')
    expect(lines[1]).toContain('01 03 00 00 00 02 C4 0B')
    expect(lines[2]).toContain('pkt_2,')
    expect(lines[2]).toContain(',response')
  })

  it('含逗号/公式危险前缀的字段按 RFC4180 转义并防公式注入', () => {
    const csv = buildPacketsCsv([
      pkt({ deviceName: '电表,一号', parsed: '=cmd|/c calc' }),
    ])
    const lines = csv.trimEnd().split('\n')
    // 含逗号的字段被整体双引号包裹；公式前缀被加单引号标记为文本
    expect(lines[1]).toContain('"电表,一号"')
    expect(lines[1]).toContain("'=cmd|/c calc")
  })
})

describe('buildConfigSnapshot（诊断包 config.snapshot.json）', () => {
  function cfg(): GatewayConfig {
    return {
      schema_version: 1,
      buses: [{ port_name: 'rs485_a', protocol: 'modbus', tcp_addr: null, timeout_ms: 1000 }],
      devices: [{ id: 'dev1', name: '电表1号', bus: 'rs485_a' }],
      mqtt: { broker: 'tcp://10.0.0.1:1883', username: 'gw', password: 'p@ssw0rd' },
      webhook: { url: 'https://example.com/hook', secret: 'whsec_123' },
    } as unknown as GatewayConfig
  }

  it('结构完整保留（排障需要看配置本身）', () => {
    const snap = buildConfigSnapshot(cfg())
    expect(snap.buses?.[0]?.port_name).toBe('rs485_a')
    expect(snap.devices?.[0]?.name).toBe('电表1号')
  })

  it('凭据字段替换为 ***，不得随诊断包外泄', () => {
    const snap = buildConfigSnapshot(cfg())
    const mqtt = (snap as unknown as Record<string, Record<string, unknown>>).mqtt
    const webhook = (snap as unknown as Record<string, Record<string, unknown>>).webhook
    expect(mqtt?.password).toBe('***')
    expect(webhook?.secret).toBe('***')
    // 非敏感字段保持可见
    expect(mqtt?.broker).toBe('tcp://10.0.0.1:1883')
    expect(webhook?.url).toBe('https://example.com/hook')
  })

  it('深拷贝：脱敏不污染页面内存中的原配置', () => {
    const original = cfg()
    buildConfigSnapshot(original)
    const mqtt = (original as unknown as Record<string, Record<string, unknown>>).mqtt
    expect(mqtt?.password).toBe('p@ssw0rd')
  })
})
