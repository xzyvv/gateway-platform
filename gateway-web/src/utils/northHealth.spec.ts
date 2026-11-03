import { describe, it, expect } from 'vitest'
import { computeNorthHealth, verdictAlertType, QUEUE_BACKLOG_THRESHOLD } from './northHealth'
import type { NorthChannel, NorthServer } from '@/api/uplink'

const channel = (over: Partial<NorthChannel> = {}): NorthChannel => ({
  name: 'mqtt',
  type: 'mqtt',
  enabled: true,
  connected: true,
  queue_length: 0,
  extra: {},
  ...over,
})

const server = (name: string, running: boolean): NorthServer => ({ name, running })

const noPassthrough = { enabled: false, sessions: [] }

describe('computeNorthHealth —— 统一结论', () => {
  it('无任何北向配置时为 idle', () => {
    const h = computeNorthHealth({ channels: [], servers: [], passthrough: noPassthrough })
    expect(h.verdict).toBe('idle')
    expect(h.issues).toEqual([])
  })

  it('全部通道已连接且无其他问题时为 ok', () => {
    const h = computeNorthHealth({
      channels: [channel()],
      servers: [server('opcua', true)],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('ok')
    expect(h.stats.channelConnected).toBe(1)
    expect(h.title).toBe('上行正常')
  })

  it('所有已启用通道均未连接时为 down', () => {
    const h = computeNorthHealth({
      channels: [channel({ name: 'mqtt', connected: false, extra: { last_error: 'connection refused' } })],
      servers: [server('opcua', true)],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('down')
    expect(h.issues[0]?.text).toContain('connection refused')
  })

  it('部分通道未连接时为 degraded 而非 down', () => {
    const h = computeNorthHealth({
      channels: [
        channel({ name: 'mqtt', connected: true }),
        channel({ name: 'webhook', type: 'http', connected: false }),
      ],
      servers: [],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.summary).toContain('1/2')
  })

  it('连接态未知（connected = null）时按 degraded 处理并提示', () => {
    const h = computeNorthHealth({
      channels: [channel({ name: 'webhook', type: 'http', connected: null })],
      servers: [],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.issues.some(i => i.text.includes('连接状态未知'))).toBe(true)
  })

  it('对外服务停止时 degraded', () => {
    const h = computeNorthHealth({
      channels: [channel()],
      servers: [server('opcua', true), server('modbus-server', false)],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.issues.some(i => i.text.includes('modbus-server') && i.text.includes('已停止'))).toBe(true)
    expect(h.stats.serverRunning).toBe(1)
  })

  it('断路器熔断时 degraded 并给出明确问题', () => {
    const h = computeNorthHealth({
      channels: [channel({ extra: { circuit_open: true } })],
      servers: [],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.issues.some(i => i.text.includes('断路器已熔断'))).toBe(true)
  })

  it('扇出丢帧时 degraded', () => {
    const h = computeNorthHealth({
      channels: [channel()],
      servers: [],
      passthrough: noPassthrough,
      laggedDropped: 7,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.issues.some(i => i.text.includes('丢帧 7 批次'))).toBe(true)
  })

  it(`队列积压超过 ${QUEUE_BACKLOG_THRESHOLD} 条时 degraded`, () => {
    const h = computeNorthHealth({
      channels: [channel()],
      servers: [],
      passthrough: noPassthrough,
      pendingMessages: QUEUE_BACKLOG_THRESHOLD + 1,
    })
    expect(h.verdict).toBe('degraded')
    expect(h.issues.some(i => i.text.includes('队列积压'))).toBe(true)
  })

  it('通道全部停用时为 idle 而不是故障', () => {
    const h = computeNorthHealth({
      channels: [channel({ enabled: false, connected: false })],
      servers: [server('opcua', true)],
      passthrough: noPassthrough,
    })
    expect(h.verdict).toBe('idle')
    expect(h.issues.some(i => i.text === '所有上行通道均已停用')).toBe(true)
  })

  it('统计透传会话数', () => {
    const h = computeNorthHealth({
      channels: [channel()],
      servers: [],
      passthrough: { enabled: true, sessions: [{ id: 's1', bus: 'bus1', started_at: 1, timeout_ms: 60_000 }] },
    })
    expect(h.stats.passthroughEnabled).toBe(true)
    expect(h.stats.passthroughSessions).toBe(1)
  })
})

describe('verdictAlertType', () => {
  it('四种结论映射到 el-alert 类型', () => {
    expect(verdictAlertType('ok')).toBe('success')
    expect(verdictAlertType('degraded')).toBe('warning')
    expect(verdictAlertType('down')).toBe('error')
    expect(verdictAlertType('idle')).toBe('info')
  })
})
