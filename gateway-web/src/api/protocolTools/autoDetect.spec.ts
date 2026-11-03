import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
// protocolTools 下的模块，request 来自上一层 barrel → mock 路径是 '../index'
vi.mock('../index', () => ({ request: requestMock }))

import { startAutoDetect, getAutoDetectResult, stopAutoDetect } from './autoDetect'
import { toAutoDetectResult, toPacket, toSendResult } from './types'
import type { AutoDetectResult as AutoDetectResultRaw, Packet as PacketRaw, SendResult as SendResultRaw } from '../generated'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function detectRow(patch: Partial<AutoDetectResultRaw> = {}): AutoDetectResultRaw {
  return {
    id: 'run-1',
    device_id: 'dev-1',
    device_name: '温度计',
    bus: 'bus1',
    slave_id: 1,
    steps: [{ id: 1, param: 'baudrate', value: '9600', status: 'success', detail: null }],
    status: 'done',
    progress: 100,
    start_time_ms: 1700000000000,
    end_time_ms: 1700000004000,
    ...patch,
  }
}

describe('autoDetect 协议自动探测契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定启动契约：POST /api/protocol-tools/auto-detect/run，请求体 { device_id }
  it('启动探测走 POST /api/protocol-tools/auto-detect/run，请求体为 { device_id }', async () => {
    requestMock.mockResolvedValue('run-1')

    const id = await startAutoDetect('dev-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/protocol-tools/auto-detect/run')
    expect(cfg.data).toEqual({ device_id: 'dev-1' })
    expect(id).toBe('run-1')
  })

  // 锁定轮询契约：GET /api/protocol-tools/auto-detect/run/:id
  it('轮询探测结果走 GET /api/protocol-tools/auto-detect/run/:id', async () => {
    requestMock.mockResolvedValue(detectRow())

    await getAutoDetectResult('run-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/protocol-tools/auto-detect/run/run-1')
  })

  // 锁定 id 编码契约：任务 id 做 URL 编码，避免拼出非法路径
  it('探测任务 id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(null)

    await getAutoDetectResult('a/b')

    expect(firstCallConfig().url).toBe('/api/protocol-tools/auto-detect/run/a%2Fb')
  })

  // 锁定中止契约：POST /api/protocol-tools/auto-detect/run/:id/stop
  it('中止探测走 POST /api/protocol-tools/auto-detect/run/:id/stop', async () => {
    requestMock.mockResolvedValue(undefined)

    await stopAutoDetect('run-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/protocol-tools/auto-detect/run/run-1/stop')
  })

  // 锁定结果映射契约：snake_case → camelCase，bigint 时间戳 → number
  it('探测结果转换为 camelCase 视图', async () => {
    requestMock.mockResolvedValue(detectRow())

    const r = await getAutoDetectResult('run-1')

    expect(r).toEqual({
      id: 'run-1',
      deviceId: 'dev-1',
      deviceName: '温度计',
      bus: 'bus1',
      slaveId: 1,
      steps: [{ id: 1, param: 'baudrate', value: '9600', status: 'success', detail: null }],
      status: 'done',
      progress: 100,
      startTime: 1700000000000,
      endTime: 1700000004000,
    })
  })

  // 锁定空结果契约：后端返回 null（任务不存在）时返回 null，不做转换
  it('后端返回 null 时透传 null', async () => {
    requestMock.mockResolvedValue(null)

    await expect(getAutoDetectResult('missing')).resolves.toBeNull()
  })
})

describe('protocolTools 类型转换函数契约', () => {
  // 锁定 toAutoDetectResult 的边界：end_time_ms 为 null 时 endTime 保持 null（探测未结束）
  it('toAutoDetectResult：未结束的探测 endTime 为 null', () => {
    const raw = detectRow({ status: 'running', progress: 40, end_time_ms: null as unknown as number })

    const v = toAutoDetectResult(raw)

    expect(v.endTime).toBeNull()
    expect(v.status).toBe('running')
    expect(v.progress).toBe(40)
  })

  // 锁定 toPacket 的转换：time_ms → time（number），matched_id/delay_ms 的 null 归一为 undefined
  it('toPacket：snake_case → camelCase 且 null 字段归一为 undefined', () => {
    const raw: PacketRaw = {
      id: 'pk-1',
      time_ms: 1700000000000,
      direction: 'response',
      device_id: 'dev-1',
      device_name: '温度计',
      bus: 'bus1',
      hex: '0103',
      ascii: '',
      parsed: '{}',
      matched_id: null,
      delay_ms: null,
    }

    const v = toPacket(raw)

    expect(v.time).toBe(1700000000000)
    expect(v.matchedId).toBeUndefined()
    expect(v.delayMs).toBeUndefined()
    expect(v.direction).toBe('response')
  })

  // 锁定 toSendResult 的转换：snake_case → camelCase，error 保持 null（成功时为 null 而非 undefined）
  it('toSendResult：字段名转换且 error 保留 null', () => {
    const raw: SendResultRaw = {
      request_hex: '0103',
      response_hex: '010304',
      success: true,
      latency_ms: 7,
      parsed: '{}',
      error: null,
    }

    const v = toSendResult(raw)

    expect(v.requestHex).toBe('0103')
    expect(v.responseHex).toBe('010304')
    expect(v.latencyMs).toBe(7)
    expect(v.error).toBeNull()
  })
})
