import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  DEAD_LETTER_PROTOCOL,
  listDeadLetters,
  purgeDeadLetters,
  replayDeadLetter,
  type DeadLetterList,
} from './deadLetter'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function listResponse(patch: Partial<DeadLetterList> = {}): DeadLetterList {
  return {
    protocol: 'http',
    total: 0,
    items: [],
    // 自动裁剪统计：缺省 0 = 从未裁剪（后端未升级时也按该语义展示）
    trimmed_total: 0,
    last_trim_ms: 0,
    last_trim_rows: 0,
    ...patch,
  }
}

describe('设备端死信 API 契约', () => {
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 契约：显式传 protocol=http（后端缺省 webhook，不能依赖缺省值）
  it('查询死信走 GET /api/north/dead-letters?protocol=http&limit=50&offset=0', async () => {
    const resp = listResponse()
    requestMock.mockResolvedValue(resp)

    await expect(listDeadLetters()).resolves.toEqual(resp)

    const c = firstCallConfig()
    expect(c.method).toBe('GET')
    expect(c.url).toBe('/api/north/dead-letters?protocol=http&limit=50&offset=0')
    expect(DEAD_LETTER_PROTOCOL).toBe('http')
  })

  it('limit/offset 可覆盖并进入 query string', async () => {
    requestMock.mockResolvedValue(listResponse())

    await listDeadLetters({ limit: 100, offset: 200 })

    expect(firstCallConfig().url).toBe('/api/north/dead-letters?protocol=http&limit=100&offset=200')
  })

  // 契约：自动裁剪统计必须原样透传给 UI。这些数据已被永久删除，
  // 界面上少了它就是"历史上从没丢过数据"的错觉。
  it('列表响应透传自动裁剪统计', async () => {
    requestMock.mockResolvedValue(
      listResponse({ trimmed_total: 12, last_trim_ms: 1_700_000_000_000, last_trim_rows: 5 }),
    )

    const got = await listDeadLetters()

    expect(got.trimmed_total).toBe(12)
    expect(got.last_trim_ms).toBe(1_700_000_000_000)
    expect(got.last_trim_rows).toBe(5)
  })

  // 契约：查不到 ≠ 0 条——接口失败必须原样抛出，由 UI 显示「状态未知」
  it('接口失败原样抛出，不在 API 层吞成空列表', async () => {
    requestMock.mockRejectedValue(new Error('死信库不可用'))

    await expect(listDeadLetters()).rejects.toThrow('死信库不可用')
  })

  // 契约：POST /api/north/dead-letters/{id}/replay
  it('重投单条死信走 POST /api/north/dead-letters/{id}/replay', async () => {
    const resp = { id: 7, outbox_id: 88, requeued: true }
    requestMock.mockResolvedValue(resp)

    await expect(replayDeadLetter(7)).resolves.toEqual(resp)

    const c = firstCallConfig()
    expect(c.method).toBe('POST')
    expect(c.url).toBe('/api/north/dead-letters/7/replay')
  })

  // 契约：不传 older_than_ms 表示全清（仍限定 protocol=http）
  it('清理全部死信走 DELETE /api/north/dead-letters?protocol=http', async () => {
    requestMock.mockResolvedValue({ deleted: 3 })

    await expect(purgeDeadLetters()).resolves.toEqual({ deleted: 3 })

    const c = firstCallConfig()
    expect(c.method).toBe('DELETE')
    expect(c.url).toBe('/api/north/dead-letters?protocol=http')
  })

  it('按时间清理时 older_than_ms 进入 query string', async () => {
    requestMock.mockResolvedValue({ deleted: 0 })

    await purgeDeadLetters(1_700_000_000_000)

    expect(firstCallConfig().url).toBe(
      '/api/north/dead-letters?protocol=http&older_than_ms=1700000000000',
    )
  })
})
