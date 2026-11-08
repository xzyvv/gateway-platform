import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listCommands, listWritablePoints } from './remoteControl'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('remoteControl 远程控制契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定可写点位列表路径：GET /api/devices/control/writable
  it('listWritablePoints 走 GET /api/devices/control/writable 并原样返回列表', async () => {
    const rows = [
      { id: 'p1', name: '设定值', data_type: 'f32', device_id: 'd1', device_name: '温控仪' },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listWritablePoints()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/control/writable')
  })

  // 锁定控制命令历史路径：GET /api/devices/control/commands（与 writable 是两个端点）
  it('listCommands 走 GET /api/devices/control/commands 并原样返回列表', async () => {
    const rows = [
      {
        id: 'c1',
        device_id: 'd1',
        device_name: '温控仪',
        point_id: 'p1',
        point_name: '设定值',
        write_value: 12.5,
        operator: 'admin',
        status: 'success',
        created_at: 1700000000,
        executed_at: 1700000001,
        result: 'ok',
      },
    ]
    requestMock.mockResolvedValue(rows)

    await expect(listCommands()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/control/commands')
  })

  // 写操作已统一到 @/api/points 的 writePoints（POST /api/point-tools/write，B2 单一入口），
  // 本模块只保留只读端点契约，写入契约见 points.spec.ts。
})
