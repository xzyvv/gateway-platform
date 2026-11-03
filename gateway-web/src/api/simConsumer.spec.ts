import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  getSimConsumerStatus,
  startSimConsumer,
  stopSimConsumer,
} from './simConsumer'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('simConsumer（模拟消费者接口契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定状态路径：真实后端是 GET /api/sim_consumer/status
  it('getSimConsumerStatus 走 GET /api/sim_consumer/status 并原样返回状态', async () => {
    const status = {
      running: false,
      enabled: true,
      consumed: 12n,
      batches: 3n,
      queue_len: 7,
      last_consumed_at_ms: null,
      started_at_ms: null,
      uptime_ms: null,
      batch_size: 100,
      interval_ms: 1000n,
      log_every: 100,
      blocked_reason: 'mqtt_enabled',
    }
    requestMock.mockResolvedValue(status)

    await expect(getSimConsumerStatus()).resolves.toEqual(status)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/sim_consumer/status')
  })

  // 锁定启动请求体：后端 SimConsumerStartReq 收 camelCase，不是 snake_case
  it('startSimConsumer 走 POST /api/sim_consumer/start，请求体为 camelCase', async () => {
    requestMock.mockResolvedValue({ running: true })

    await startSimConsumer({ batchSize: 50, intervalMs: 500, logEvery: 10 })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/sim_consumer/start')
    expect(cfg.data).toEqual({ batchSize: 50, intervalMs: 500, logEvery: 10 })
  })

  // 不传参数时也要发空对象，避免后端拿到 undefined 请求体
  it('startSimConsumer 不传参数时请求体为空对象', async () => {
    requestMock.mockResolvedValue({ running: true })

    await startSimConsumer()

    expect(firstCallConfig().data).toEqual({})
  })

  it('stopSimConsumer 走 POST /api/sim_consumer/stop 且不带请求体', async () => {
    requestMock.mockResolvedValue({ running: false })

    await stopSimConsumer()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/sim_consumer/stop')
    expect(cfg.data).toBeUndefined()
  })

  // 错误分支：后端 409（已在运行 / MQTT 已启用）原样 reject，由页面翻译成可读提示
  it('后端报错时 startSimConsumer 原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(409): 模拟消费者已在运行'))

    await expect(startSimConsumer()).rejects.toThrow('模拟消费者已在运行')
  })
})
