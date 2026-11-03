import { describe, it, expect, afterEach, vi } from 'vitest'

// getToken / isTauri 经 vi.hoisted 提前声明，供 vi.mock 工厂引用
const { getTokenMock, isTauriMock } = vi.hoisted(() => ({
  getTokenMock: vi.fn<() => Promise<string | null>>(async () => 'test-token'),
  isTauriMock: vi.fn<() => boolean>(() => false),
}))
vi.mock('../token', () => ({ getToken: getTokenMock }))
vi.mock('../env', () => ({ isTauri: isTauriMock }))

import { canUseSseStream, postSseStream } from './sse'

interface FakeReader {
  read: () => Promise<{ done: boolean; value?: Uint8Array }>
}

/** 构造一个 SSE 风格的 fetch 桩：按 chunks 逐块吐数据 */
function sseFetch(chunks: string[]) {
  return vi.fn(async (_url: unknown, _init?: unknown) => ({
    ok: true,
    status: 200,
    headers: { get: (k: string) => (k.toLowerCase() === 'content-type' ? 'text/event-stream' : null) },
    body: {
      getReader: (): FakeReader => {
        const encoded = chunks.map((c) => new TextEncoder().encode(c))
        let i = 0
        return {
          read: async () =>
            i < encoded.length
              ? { done: false, value: encoded[i++] }
              : { done: true },
        }
      },
    },
  })) as unknown as typeof fetch
}

afterEach(() => {
  vi.unstubAllGlobals()
  getTokenMock.mockReset()
  getTokenMock.mockResolvedValue('test-token')
  isTauriMock.mockReset()
  isTauriMock.mockReturnValue(false)
})

describe('canUseSseStream 环境探测', () => {
  it('fetch + ReadableStream 齐备且非 Tauri：可用', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', class {})
    expect(canUseSseStream()).toBe(true)
  })

  it('缺 ReadableStream（旧浏览器）：不可用', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', undefined)
    expect(canUseSseStream()).toBe(false)
  })

  it('缺 fetch：不可用', () => {
    vi.stubGlobal('fetch', undefined)
    vi.stubGlobal('ReadableStream', class {})
    expect(canUseSseStream()).toBe(false)
  })

  it('Tauri 桌面端即使 API 齐备也不可用（相对 URL 的 fetch 不指向网关）', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', class {})
    isTauriMock.mockReturnValue(true)
    expect(canUseSseStream()).toBe(false)
  })
})

describe('postSseStream 请求形态契约', () => {
  it('POST + Accept: text/event-stream + Bearer 头 + JSON 请求体', async () => {
    vi.stubGlobal('fetch', sseFetch(['event: ping\ndata: {"a":1}\n\n']))

    await postSseStream('/api/protocol-tools/slave-scan', { bus: 'X' }, { onEvent: () => {} }, '扫描失败')

    const [url, init] = vi.mocked(fetch).mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/protocol-tools/slave-scan')
    expect(init.method).toBe('POST')
    const headers = init.headers as Record<string, string>
    expect(headers['Content-Type']).toBe('application/json')
    expect(headers.Accept).toBe('text/event-stream')
    expect(headers.Authorization).toBe('Bearer test-token')
    expect(JSON.parse(init.body as string)).toEqual({ bus: 'X' })
  })

  it('无 token 时不带 Authorization 头', async () => {
    getTokenMock.mockResolvedValue(null)
    vi.stubGlobal('fetch', sseFetch([]))

    await postSseStream('/api/x', {}, { onEvent: () => {} }, '失败')

    const [, init] = vi.mocked(fetch).mock.calls[0] as unknown as [string, RequestInit]
    expect((init.headers as Record<string, string>).Authorization).toBeUndefined()
  })
})

describe('postSseStream 事件分发契约', () => {
  it('逐事件回调（event 名 + data JSON），容忍 TCP 拆包/粘包', async () => {
    // 事件 1 完整，事件 2 跨两块
    const part1 = 'event: started\ndata: {"bus":"X","total":2}\n\nevent: sl'
    const part2 = 'ave\ndata: {"slave_id":1}\n\n'
    vi.stubGlobal('fetch', sseFetch([part1, part2]))

    const events: Array<[string, Record<string, unknown>]> = []
    await postSseStream('/api/x', {}, { onEvent: (e, d) => events.push([e, d]) }, '失败')

    expect(events).toEqual([
      ['started', { bus: 'X', total: 2 }],
      ['slave', { slave_id: 1 }],
    ])
  })

  it('心跳注释行（:...）与无 data 的块被忽略', async () => {
    vi.stubGlobal(
      'fetch',
      sseFetch([': hb\n\n', 'event: only_event\n\n', 'event: ok\ndata: {"x":1}\n\n']),
    )

    const events: string[] = []
    await postSseStream('/api/x', {}, { onEvent: (e) => events.push(e) }, '失败')

    expect(events).toEqual(['ok'])
  })

  it('半截 JSON 容错跳过该事件，不中断整条流', async () => {
    vi.stubGlobal('fetch', sseFetch(['event: bad\ndata: {broken\n\nevent: good\ndata: {"x":1}\n\n']))

    const events: string[] = []
    await postSseStream('/api/x', {}, { onEvent: (e) => events.push(e) }, '失败')

    expect(events).toEqual(['good'])
  })
})

describe('postSseStream 错误与中断契约', () => {
  it('准入失败：流建立前的 JSON 错误体，抛出服务端 message', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: false,
        status: 400,
        headers: { get: () => 'application/json' },
        json: async () => ({ code: 4002, message: '总线忙' }),
        body: null,
      })) as unknown as typeof fetch,
    )

    await expect(postSseStream('/api/x', {}, { onEvent: () => {} }, '扫描失败')).rejects.toThrow('总线忙')
  })

  it('准入失败且无服务端 message：抛出带前缀与 HTTP 状态码的错误', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: false,
        status: 404,
        headers: { get: () => 'application/json' },
        json: async () => ({}),
        body: null,
      })) as unknown as typeof fetch,
    )

    await expect(postSseStream('/api/x', {}, { onEvent: () => {} }, '扫描失败')).rejects.toThrow('扫描失败（HTTP 404）')
  })

  it('content-type 不是 text/event-stream：同样按准入失败处理', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => ({
        ok: true,
        status: 200,
        headers: { get: () => 'application/json' },
        json: async () => ({ message: '不是流' }),
        body: null,
      })) as unknown as typeof fetch,
    )

    await expect(postSseStream('/api/x', {}, { onEvent: () => {} }, '扫描失败')).rejects.toThrow('不是流')
  })

  it('用户 abort：属正常收尾，resolve 不 reject', async () => {
    const controller = new AbortController()
    vi.stubGlobal(
      'fetch',
      vi.fn(async (_url: unknown, init?: RequestInit) => ({
        ok: true,
        status: 200,
        headers: { get: () => 'text/event-stream' },
        body: {
          getReader: (): FakeReader => ({
            read: () =>
              new Promise((_, reject) => {
                if (init?.signal?.aborted) return reject(new DOMException('Aborted', 'AbortError'))
                init?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')))
              }),
          }),
        },
      })) as unknown as typeof fetch,
    )

    const p = postSseStream('/api/x', {}, { onEvent: () => {}, signal: controller.signal }, '扫描失败')
    controller.abort()
    await expect(p).resolves.toBeUndefined()
  })

  it('非 abort 的网络异常：原样 reject', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new Error('network down')
      }) as unknown as typeof fetch,
    )

    await expect(postSseStream('/api/x', {}, { onEvent: () => {} }, '扫描失败')).rejects.toThrow('network down')
  })
})
