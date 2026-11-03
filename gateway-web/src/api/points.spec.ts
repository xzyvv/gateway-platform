import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { readPointNow, writePoints } from './points'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('points 写接口契约', () => {
  beforeEach(() => requestMock.mockReset())

  // B2 统一写入口契约：所有写入（含单点）都走 POST /api/points/write，
  // 请求体固定 { writes: [{ id, value }] }，单/批分发由后端按条数决定。
  // 防止回退到旧的 /api/points/:id/write 或 /api/points/write-batch 造成双入口。
  it('单点写也走统一入口 POST /api/points/write，入参为单元素数组', async () => {
    requestMock.mockResolvedValue({ status: 'ok', total: 1, succeeded: 1, failed: 0, results: [] })
    await writePoints([{ id: 'p1', value: 12.5 }])

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/write')
    expect(cfg.url).not.toBe('/api/points/write-batch')
    expect(cfg.url).not.toBe('/api/points/p1/write')
    expect(cfg.data).toEqual({ writes: [{ id: 'p1', value: 12.5 }] })
  })

  it('单点读走 GET /api/points/:id/read', async () => {
    requestMock.mockResolvedValue({ status: 'ok' })
    await readPointNow('p1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/points/p1/read')
  })

  it('多点写同样走统一入口，请求体为 { writes: [{ id, value }] }', async () => {
    requestMock.mockResolvedValue({ status: 'ok', total: 2, succeeded: 2, failed: 0, results: [] })
    await writePoints([
      { id: 'p1', value: 1 },
      { id: 'p2', value: 2 },
    ])

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/points/write')
    expect(cfg.data).toEqual({
      writes: [
        { id: 'p1', value: 1 },
        { id: 'p2', value: 2 },
      ],
    })
  })

  it('原样透传后端响应（results 与入参逐位对应）', async () => {
    const resp = {
      status: 'partial',
      total: 2,
      succeeded: 1,
      failed: 1,
      results: [
        { point_id: 'p1', status: 'ok', value: 1 },
        { point_id: 'p2', status: 'error', error: 'mock write failure', quality: null },
      ],
    }
    requestMock.mockResolvedValue(resp)

    await expect(
      writePoints([
        { id: 'p1', value: 1 },
        { id: 'p2', value: 2 },
      ]),
    ).resolves.toEqual(resp)
  })

  // 边界：写 0 是合法值，不能因为 falsy 而被丢掉或改成默认值
  it('写入 0 值时仍原样发送', async () => {
    requestMock.mockResolvedValue({ status: 'ok', total: 1, succeeded: 1, failed: 0, results: [] })
    await writePoints([{ id: 'p1', value: 0 }])

    expect(firstCallConfig().data).toEqual({ writes: [{ id: 'p1', value: 0 }] })
  })

  // 错误分支：后端失败原样 reject（写值失败必须让用户知道，不能假装成功）
  it('后端报错时原样 reject', async () => {
    requestMock.mockRejectedValueOnce(new Error('请求失败(502): 设备无响应'))
    await expect(writePoints([{ id: 'p1', value: 1 }])).rejects.toThrow('设备无响应')
  })
})
