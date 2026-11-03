import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listPausedBuses, pauseBusCollect, resumeBusCollect } from './buses'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('buses 总线采集暂停/恢复契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定暂停路径：POST /api/buses/:name/pause（后端 config/routes.rs 的路由），
  // 误写成 /api/buses/:name/pause-collect 会 404。
  it('pauseBusCollect 走 POST /api/buses/:bus/pause', async () => {
    requestMock.mockResolvedValue({ bus: 'modbus-1', paused: true, paused_buses: ['modbus-1'] })

    await pauseBusCollect('modbus-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/buses/modbus-1/pause')
  })

  // 锁定恢复路径：POST /api/buses/:name/resume（与 pause 成对，不可混写）
  it('resumeBusCollect 走 POST /api/buses/:bus/resume', async () => {
    requestMock.mockResolvedValue({ bus: 'modbus-1', paused: false, paused_buses: [] })

    await resumeBusCollect('modbus-1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/buses/modbus-1/resume')
  })

  // 锁定列表路径：GET /api/buses/paused（注意不是 /api/buses/:name/paused）
  it('listPausedBuses 走 GET /api/buses/paused 并原样返回', async () => {
    const resp = { paused_buses: ['modbus-1', 'serial-1'] }
    requestMock.mockResolvedValue(resp)

    await expect(listPausedBuses()).resolves.toBe(resp)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/buses/paused')
  })

  // 边界：总线名含特殊字符时必须做 URL 编码，否则路径被截断打到错误的总线
  it('总线名做 URL 编码，避免特殊字符破坏路径', async () => {
    requestMock.mockResolvedValue({ bus: 'a/b', paused: true, paused_buses: [] })

    await pauseBusCollect('a/b')

    expect(firstCallConfig().url).toBe('/api/buses/a%2Fb/pause')
  })

  // 错误分支：后端失败原样 reject，由页面给出「暂停失败」等具体文案
  it('后端报错时 pauseBusCollect 原样 reject', async () => {
    requestMock.mockRejectedValueOnce(new Error('请求失败(500): 暂停总线失败'))
    await expect(pauseBusCollect('modbus-1')).rejects.toThrow('暂停总线失败')
  })
})
