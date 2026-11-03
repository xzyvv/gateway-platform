import { describe, it, expect, afterEach, vi } from 'vitest'
import { scanBusStream, canStreamScan } from './slaveScan'

vi.mock('../token', () => ({ getToken: vi.fn(async () => 'test-token') }))

/**
 * P0-2 契约 §4：流式扫描的 SSE 事件分发与中断语义。
 * fetch 用手工 reader 桩（不依赖 jsdom 是否自带 ReadableStream），
 * 事件流文本按契约格式逐字构造。
 */

interface FakeReader { read: () => Promise<{ done: boolean; value?: Uint8Array }> }

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
})

describe('scanBusStream 事件分发（p0-2 §4）', () => {
  it('started → slave×2 → summary 按序分发且字段转换为 camelCase', async () => {
    // 故意拆成两块（事件1 完整，事件2/3 跨块），钉住粘包/拆包容忍
    const part1 =
      'event: started\ndata: {"bus":"/dev/ttyUSB0","range_start":1,"range_end":2,"total":2,"timeout_ms":500,"interval_ms":50,"started_at_ms":1761300000000}\n\n' +
      'event: slave\ndata: {"slave_id":1,"responded":false,"device_name":null,"func_code":null,"latency_ms":null}\n\nevent: sl'
    const part2 =
      'ave\ndata: {"slave_id":2,"responded":true,"device_name":null,"func_code":"03","latency_ms":7}\n\n' +
      ': hb\n\n' +
      'event: summary\ndata: {"bus":"/dev/ttyUSB0","scanned":2,"found":1,"noise":0,"duration_ms":1200,"results":[{"slave_id":1,"responded":false,"device_name":null,"func_code":null,"latency_ms":null},{"slave_id":2,"responded":true,"device_name":null,"func_code":"03","latency_ms":7}]}\n\n'
    vi.stubGlobal('fetch', sseFetch([part1, part2]))

    const events: string[] = []
    let summaryFound = -1
    let summaryResultsLen = -1
    await scanBusStream('/dev/ttyUSB0', 1, 2, {
      onStarted: (info) => {
        events.push('started')
        expect(info).toEqual({
          bus: '/dev/ttyUSB0', rangeStart: 1, rangeEnd: 2, total: 2,
          timeoutMs: 500, intervalMs: 50, startedAtMs: 1761300000000,
        })
      },
      onSlave: (r) => {
        events.push(`slave:${r.slaveId}`)
        if (r.slaveId === 2) {
          expect(r.responded).toBe(true)
          expect(r.funcCode).toBe('03')
          expect(r.latencyMs).toBe(7)
        }
      },
      onSummary: (s) => {
        events.push('summary')
        summaryFound = s.found
        summaryResultsLen = s.results.length
      },
    })

    expect(events).toEqual(['started', 'slave:1', 'slave:2', 'summary'])
    expect(summaryFound).toBe(1)
    expect(summaryResultsLen).toBe(2)
    // 请求形态：同一 URL + Accept: text/event-stream + Bearer 头（契约 §2）
    const [url, init] = vi.mocked(fetch).mock.calls[0] as unknown as [string, RequestInit]
    expect(url).toBe('/api/protocol-tools/slave-scan')
    expect((init.headers as Record<string, string>).Accept).toBe('text/event-stream')
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer test-token')
    expect(JSON.parse(init.body as string)).toEqual({ bus: '/dev/ttyUSB0', range_start: 1, range_end: 2 })
  })

  it('准入失败（流建立前 JSON 错误体）抛出服务端 message', async () => {
    vi.stubGlobal('fetch', vi.fn(async () => ({
      ok: false,
      status: 400,
      headers: { get: () => 'application/json' },
      json: async () => ({ code: 4002, message: '总线 X 协议为 cjt188，从站扫描目前仅支持 Modbus 总线' }),
      body: null,
    })) as unknown as typeof fetch)

    await expect(scanBusStream('X', 1, 3, {})).rejects.toThrow('仅支持 Modbus')
  })

  it('用户 abort：read 中断属正常收尾，resolve 不 reject', async () => {
    const controller = new AbortController()
    vi.stubGlobal('fetch', vi.fn(async (_url: unknown, init?: RequestInit) => ({
      ok: true,
      status: 200,
      headers: { get: () => 'text/event-stream' },
      body: {
        getReader: (): FakeReader => ({
          read: () =>
            new Promise((_, reject) => {
              // abort 可能先于 read 发生（同步竞态），两种时序都要覆盖
              if (init?.signal?.aborted) return reject(new DOMException('Aborted', 'AbortError'))
              init?.signal?.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')))
            }),
        }),
      },
    })) as unknown as typeof fetch)

    const p = scanBusStream('/dev/ttyUSB0', 1, 247, { signal: controller.signal })
    controller.abort()
    await expect(p).resolves.toBeUndefined()
  })
})

describe('canStreamScan 环境探测', () => {
  it('与回放同一谓词（fetch + ReadableStream 齐备）', () => {
    vi.stubGlobal('fetch', vi.fn())
    vi.stubGlobal('ReadableStream', class {})
    expect(canStreamScan()).toBe(true)
    vi.stubGlobal('ReadableStream', undefined)
    expect(canStreamScan()).toBe(false)
  })
})
