import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { healthReady } from './status'

interface RequestConfig {
  url: string
  method: string
  validateStatus: (status: number) => boolean
}

describe('healthReady（就绪探针）', () => {
  beforeEach(() => requestMock.mockReset())

  it('放行 2xx 与 503（降级），其余状态仍判为失败', async () => {
    requestMock.mockResolvedValue({ status: 'ok', components: {} })
    await healthReady()

    const cfg = requestMock.mock.calls[0]![0] as RequestConfig
    expect(cfg.url).toBe('/health/ready')
    expect(cfg.method).toBe('GET')
    // 2xx 正常
    expect(cfg.validateStatus(200)).toBe(true)
    // 503：网关降级时返 503 但 body 仍含健康快照，必须放行
    expect(cfg.validateStatus(503)).toBe(true)
    // 其它错误码仍应触发全局错误处理
    expect(cfg.validateStatus(500)).toBe(false)
    expect(cfg.validateStatus(404)).toBe(false)
  })

  it('降级快照不被丢弃，原样返回给调用方', async () => {
    const degraded = { status: 'degraded', components: { mqtt: 'down' } }
    requestMock.mockResolvedValue(degraded)
    await expect(healthReady()).resolves.toEqual(degraded)
  })
})