import { describe, expect, it } from 'vitest'
import {
  buildChecklist,
  buildSelfCheckMarkdown,
  checklistToMarkdown,
  evalBuses,
  evalCertificates,
  evalClock,
  evalNorth,
  evalProtocols,
  evalWatchdog,
  summarizeLevel,
  type CheckItem,
} from './selfCheck'
import type { GatewayConfig } from '@/api/types'
import type { HealthReport } from '@/api/status'
import type { ProtocolCapabilityEntry } from '@/api/protocols'
import type { NorthStatus } from '@/api/uplink'
import type { Certificate } from '@/api/certificates'
import type { ServiceInfo } from '@/api/watchdog'

function config(overrides: Partial<GatewayConfig> = {}): GatewayConfig {
  return {
    schema_version: 1,
    buses: [],
    points: [],
    devices: [],
    ...overrides,
  } as GatewayConfig
}

function report(overrides: Partial<HealthReport> = {}): HealthReport {
  return {
    generated_at: '2026-11-03T02:00:00Z',
    summary: {
      level: 'ok',
      total_devices: 1,
      online_devices: 1,
      total_reads: 10,
      success_rate: 99,
      avg_latency_ms: 20,
      uptime_secs: 3600,
    },
    buses: [],
    problems: [],
    ...overrides,
  } as HealthReport
}

describe('summarizeLevel', () => {
  it('未通过优先于提醒与通过', () => {
    expect(summarizeLevel(['pass', 'warn', 'fail'])).toBe('fail')
    expect(summarizeLevel(['pass', 'warn'])).toBe('warn')
    expect(summarizeLevel(['pass'])).toBe('pass')
    expect(summarizeLevel([])).toBe('pass')
  })
})

describe('evalProtocols', () => {
  const caps: ProtocolCapabilityEntry[] = [{ name: 'modbus', available: true } as unknown as ProtocolCapabilityEntry]

  it('配置未使用总线时未通过', () => {
    expect(evalProtocols(caps, config()).level).toBe('fail')
  })

  it('使用了未注册协议时未通过', () => {
    const cfg = config({
      buses: [{ port_name: 'COM3', protocol: 'iec104' } as never],
    })
    expect(evalProtocols(caps, cfg).level).toBe('fail')
  })

  it('协议全部注册时通过', () => {
    const cfg = config({
      buses: [{ port_name: 'COM3', protocol: 'modbus' } as never],
    })
    expect(evalProtocols(caps, cfg).level).toBe('pass')
  })
})

describe('evalBuses', () => {
  const cfg = config({ buses: [{ port_name: 'COM3', protocol: 'modbus' } as never] })

  it('设备全离线时未通过', () => {
    const r = report({
      buses: [
        { bus: 'COM3', protocol: 'modbus', online_devices: 0, total_devices: 2, total_reads: 10, success_rate: 80 } as never,
      ],
    })
    expect(evalBuses(r, cfg).level).toBe('fail')
  })

  it('部分在线或成功率偏低时提醒', () => {
    const r = report({
      buses: [
        { bus: 'COM3', protocol: 'modbus', online_devices: 1, total_devices: 2, total_reads: 10, success_rate: 85 } as never,
      ],
    })
    expect(evalBuses(r, cfg).level).toBe('warn')
  })

  it('无采集统计时提醒而非误判失败', () => {
    expect(evalBuses(report(), cfg).level).toBe('warn')
  })
})

describe('evalNorth', () => {
  it('未启用通道时提醒', () => {
    const status = {
      channels: [],
      servers: [],
      aggregated: { pending_messages: 0, lagged_dropped_batches: 0 },
    } as unknown as NorthStatus
    expect(evalNorth(status).level).toBe('warn')
  })

  it('启用通道断连时未通过', () => {
    const status = {
      channels: [{ name: 'MQTT', enabled: true, connected: false }],
      servers: [],
      aggregated: { pending_messages: 0 },
    } as unknown as NorthStatus
    expect(evalNorth(status).level).toBe('fail')
  })

  it('通道连接且无积压时通过', () => {
    const status = {
      channels: [{ name: 'MQTT', enabled: true, connected: true }],
      servers: [],
      aggregated: { pending_messages: 10 },
    } as unknown as NorthStatus
    expect(evalNorth(status).level).toBe('pass')
  })
})

describe('evalClock', () => {
  it('校时失败未通过', () => {
    expect(evalClock({ status: 'error: timeout' }).level).toBe('fail')
  })

  it('尚未校时提醒', () => {
    expect(evalClock({ status: 'idle' }).level).toBe('warn')
  })

  it('偏差超限分级', () => {
    expect(evalClock({ status: 'synced', offset_ms: 300 }).level).toBe('pass')
    expect(evalClock({ status: 'synced', offset_ms: 2000 }).level).toBe('warn')
    expect(evalClock({ status: 'synced', offset_ms: 8000 }).level).toBe('fail')
  })

  // 时钟不可信优先于 NTP 状态判定：网关已据此暂停计划任务，属交付级问题
  it('时钟不可信判失败（即使 NTP 已同步）', () => {
    const item = evalClock({
      status: 'synced',
      offset_ms: 100,
      clock_trusted: false,
      clock_untrust_reason: '系统时间早于 2020-01-01（疑似无 RTC 掉电回退）',
    })
    expect(item.level).toBe('fail')
    expect(item.detail).toContain('计划任务已暂停')
    expect(item.detail).toContain('2020-01-01')
  })

  it('时钟不可信但未下发原因时仍判失败', () => {
    const item = evalClock({ status: 'synced', clock_trusted: false })
    expect(item.level).toBe('fail')
    expect(item.detail).toContain('原因未知')
  })

  it('时钟可信时沿用原有 NTP 判定', () => {
    expect(evalClock({ status: 'synced', offset_ms: 300, clock_trusted: true }).level).toBe('pass')
  })
})

describe('evalCertificates', () => {
  it('无证书时提醒（明文可交付内网）', () => {
    expect(evalCertificates([]).level).toBe('warn')
  })

  it('证书过期未通过，临近过期提醒', () => {
    const base = { id: 'c1', name: 'server', enabled: true } as Certificate
    expect(evalCertificates([{ ...base, days_remaining: -1 }]).level).toBe('fail')
    expect(evalCertificates([{ ...base, days_remaining: 10 }]).level).toBe('warn')
    expect(evalCertificates([{ ...base, days_remaining: 120 }]).level).toBe('pass')
  })
})

describe('evalWatchdog', () => {
  it('服务停止未通过，重启提醒', () => {
    const svc = (status: string): ServiceInfo =>
      ({ id: status, name: status, status, restarts: 0, last_heartbeat: null } as unknown as ServiceInfo)
    expect(evalWatchdog([svc('Stopped')]).level).toBe('fail')
    expect(evalWatchdog([svc('Restarting')]).level).toBe('warn')
    expect(evalWatchdog([svc('Running')]).level).toBe('pass')
  })
})

describe('报告与检查单', () => {
  const items: CheckItem[] = [
    { id: 'protocol', title: '协议', level: 'pass', detail: '全部可用' },
    { id: 'bus', title: '总线', level: 'warn', detail: '1 台离线' },
  ]

  it('Markdown 报告包含结论与明细', () => {
    const md = buildSelfCheckMarkdown(
      { generatedAt: Date.now(), items, level: 'warn' },
      { version: 'v1.0.0', build_time: '', git_hash: '' },
    )
    expect(md).toContain('# 网关交付自检报告')
    expect(md).toContain('提醒（0 项未通过，1 项提醒）')
    expect(md).toContain('| 协议 | 通过 | 全部可用 |')
  })

  it('检查单包含自动项与人工项，未勾选为待确认', () => {
    const list = buildChecklist({ generatedAt: Date.now(), items, level: 'warn' }, {}, '张三')
    expect(list.kind).toBe('gateway-delivery-checklist')
    expect(list.operator).toBe('张三')
    expect(list.checks.filter((c) => c.source === 'auto')).toHaveLength(2)
    expect(list.checks.some((c) => c.status === 'todo')).toBe(true)
    expect(list.overall).toBe('warn')
    expect(checklistToMarkdown(list)).toContain('| 协议 | 自动自检 | 通过 |')
  })

  it('人工项全部勾选且自动全通过时总体通过', () => {
    const ok: CheckItem[] = [{ id: 'protocol', title: '协议', level: 'pass', detail: 'ok' }]
    const manual = Object.fromEntries(
      ['wiring', 'addr', 'value', 'recovery'].map((id) => [id, { checked: true, note: '' }]),
    )
    const list = buildChecklist({ generatedAt: Date.now(), items: ok, level: 'pass' }, manual, '')
    expect(list.overall).toBe('pass')
  })
})
