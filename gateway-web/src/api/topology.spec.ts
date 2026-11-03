import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { getTopology } from './topology'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('topology 设备拓扑契约', () => {
  beforeEach(() => requestMock.mockReset())

  // 锁定拓扑图路径：GET /api/devices/topology（无 /stats 后缀）
  it('getTopology 走 GET /api/devices/topology 并原样返回节点与连线', async () => {
    const topo = {
      nodes: [{ id: 'b1', name: 'modbus-1', node_type: 'bus', status: 'ok', meta: null }],
      links: [{ source: 'b1', target: 'd1' }],
      stats: {
        bus_count: 2,
        device_count: 5,
        online_count: 4,
        offline_count: 1,
        warning_count: 0,
        point_count: 120,
      },
    }
    requestMock.mockResolvedValue(topo)

    await expect(getTopology()).resolves.toEqual(topo)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/devices/topology')
    expect(cfg.url).not.toBe('/api/devices/topology/stats')
  })

  // 第 1 期冗余收口：`GET /api/devices/topology/stats` 已删除，
  // 统计并入拓扑响应内的 stats 字段（同源同判据），且只发一次请求。
  it('拓扑统计随拓扑响应返回，不再请求 /api/devices/topology/stats', async () => {
    const stats = {
      bus_count: 2,
      device_count: 5,
      online_count: 4,
      offline_count: 1,
      warning_count: 0,
      point_count: 120,
    }
    requestMock.mockResolvedValue({ nodes: [], links: [], stats })

    const topo = await getTopology()

    expect(topo.stats).toEqual(stats)
    expect(requestMock).toHaveBeenCalledTimes(1)
    expect(firstCallConfig().url).not.toBe('/api/devices/topology/stats')
  })

  // 错误分支：后端失败原样 reject，由页面给出「加载拓扑失败」文案
  it('后端报错时 getTopology 原样 reject', async () => {
    requestMock.mockRejectedValueOnce(new Error('请求失败(500): 拓扑构建失败'))
    await expect(getTopology()).rejects.toThrow('拓扑构建失败')
  })
})
