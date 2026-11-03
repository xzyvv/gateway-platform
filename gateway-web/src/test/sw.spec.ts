import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { runInNewContext } from 'node:vm'

/**
 * public/sw.js 缓存策略单元测试。
 *
 * sw.js 是 Service Worker 独立脚本（不走 Vite 构建、不在模块图里），
 * 测试用 fs 读源文本，在伪造的 SW 全局环境（self / caches / fetch）里
 * 用 node:vm 执行，再手动派发 install / activate / fetch 事件。
 */

// vitest 以项目根为 cwd（vmThreads 池里 import.meta.url 不是 file: URL，不能用）
const swSource = readFileSync(resolve(process.cwd(), 'public/sw.js'), 'utf-8')

const ORIGIN = 'https://gw.local'

// ===== 伪造 Cache Storage =====
// 真实 Cache 存的是 Response 且可重复读；undici Response 的 body 只能消费一次，
// 所以这里落库时序列化，match 时再重建一个新的 Response。

interface FakeRequest {
  url: string
  method: string
}

interface StoredEntry {
  status: number
  statusText: string
  headers: Record<string, string>
  body: string
}

class FakeCache {
  readonly entries = new Map<string, StoredEntry>()

  async addAll(urls: string[]) {
    for (const url of urls) {
      this.entries.set(url, { status: 200, statusText: 'OK', headers: {}, body: `shell:${url}` })
    }
  }

  async put(request: FakeRequest, response: Response) {
    const headers: Record<string, string> = {}
    response.headers.forEach((value, key) => {
      headers[key] = value
    })
    this.entries.set(request.url, {
      status: response.status,
      statusText: response.statusText,
      headers,
      body: await response.text(),
    })
  }

  async match(request: FakeRequest): Promise<Response | undefined> {
    const entry = this.entries.get(request.url)
    if (!entry) return undefined
    return new Response(entry.body, {
      status: entry.status,
      statusText: entry.statusText,
      headers: entry.headers,
    })
  }

  async delete(request: FakeRequest) {
    return this.entries.delete(request.url)
  }
}

function createCaches() {
  const stores = new Map<string, FakeCache>()
  return {
    stores,
    async open(name: string) {
      let cache = stores.get(name)
      if (!cache) {
        cache = new FakeCache()
        stores.set(name, cache)
      }
      return cache
    },
    async match(request: FakeRequest) {
      for (const cache of stores.values()) {
        const hit = await cache.match(request)
        if (hit) return hit
      }
      return undefined
    },
    async keys() {
      return [...stores.keys()]
    },
    async delete(name: string) {
      return stores.delete(name)
    },
  }
}

// ===== 伪造 SW 全局环境并执行脚本 =====

interface SwEnv {
  self: {
    location: { origin: string }
    addEventListener: (type: string, fn: (e: unknown) => void) => void
    skipWaiting: ReturnType<typeof vi.fn>
    clients: { claim: ReturnType<typeof vi.fn> }
  }
  caches: ReturnType<typeof createCaches>
  fetchMock: ReturnType<typeof vi.fn>
  listeners: Record<string, Array<(e: unknown) => void>>
}

function makeEnv(): SwEnv {
  const listeners: Record<string, Array<(e: unknown) => void>> = {}
  const self = {
    location: { origin: ORIGIN },
    addEventListener(type: string, fn: (e: unknown) => void) {
      ;(listeners[type] ??= []).push(fn)
    },
    skipWaiting: vi.fn(() => Promise.resolve()),
    clients: { claim: vi.fn(() => Promise.resolve()) },
  }
  const caches = createCaches()
  const fetchMock = vi.fn()
  runInNewContext(swSource, {
    self,
    caches,
    fetch: fetchMock,
    URL,
    Response,
    Headers,
    Promise,
    Date,
    Math,
    Number,
    String,
  })
  return { self, caches, fetchMock, listeners }
}

// ===== 伪造网络响应 =====
// SW 里 fetch 到的同源响应 type 为 'basic'，undici Response 只有 'default'，
// 且 SW 代码只用了 status/type/headers/clone/blob/text，这里做鸭子类型包装。

interface NetResponseLike {
  status: number
  statusText: string
  type: string
  headers: Headers
  clone(): NetResponseLike
  blob(): Promise<Blob>
  text(): Promise<string>
}

function netResponse(
  body: string | object,
  init: { status?: number; headers?: Record<string, string> } = {},
): NetResponseLike {
  const payload = typeof body === 'string' ? body : JSON.stringify(body)
  const inner = new Response(payload, {
    status: init.status ?? 200,
    headers: { 'content-type': 'application/json', ...init.headers },
  })
  const wrap = (r: Response): NetResponseLike => ({
    status: r.status,
    statusText: r.statusText,
    type: 'basic',
    headers: r.headers,
    clone: () => wrap(r.clone()),
    blob: () => r.clone().blob(),
    text: () => r.clone().text(),
  })
  return wrap(inner)
}

// ===== 事件派发 =====

function fireLifecycle(env: SwEnv, type: 'install' | 'activate'): Promise<unknown>[] {
  const waited: Promise<unknown>[] = []
  const event = {
    waitUntil: (p: Promise<unknown>) => {
      waited.push(p)
    },
  }
  for (const fn of env.listeners[type] ?? []) fn(event)
  return waited
}

type SwResponse = Pick<Response, 'status' | 'headers' | 'text'>

async function dispatchFetch(env: SwEnv, path: string, method = 'GET') {
  const event = {
    request: { url: `${ORIGIN}${path}`, method },
    responded: null as Promise<unknown> | null,
    respondWith(p: Promise<unknown>) {
      this.responded = p
    },
  }
  for (const fn of env.listeners.fetch ?? []) fn(event)
  const response = event.responded ? ((await event.responded) as SwResponse) : null
  return { intercepted: event.responded !== null, response }
}

/** cacheFirst 的 cache.put 不阻塞响应，等几拍微任务让回填完成 */
async function flushAsync(rounds = 10) {
  for (let i = 0; i < rounds; i++) await Promise.resolve()
}

function cacheBySuffix(env: SwEnv, suffix: string): FakeCache {
  const name = [...env.caches.stores.keys()].find((k) => k.endsWith(suffix))
  if (!name) throw new Error(`cache *${suffix} not found`)
  return env.caches.stores.get(name)!
}

describe('public/sw.js 缓存策略', () => {
  let env: SwEnv

  beforeEach(() => {
    env = makeEnv()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('install：预缓存应用外壳并 skipWaiting', async () => {
    await Promise.all(fireLifecycle(env, 'install'))
    expect(env.self.skipWaiting).toHaveBeenCalledTimes(1)
    const shell = cacheBySuffix(env, '-shell')
    expect(shell.entries.has('/')).toBe(true)
    expect(shell.entries.has('/index.html')).toBe(true)
  })

  it('activate：清理旧版本缓存并 clients.claim', async () => {
    const oldCache = await env.caches.open('gateway-web-v1.0.0-api')
    oldCache.entries.set(`${ORIGIN}/api/status`, {
      status: 200,
      statusText: 'OK',
      headers: {},
      body: '{}',
    })
    await Promise.all(fireLifecycle(env, 'activate'))
    expect(await env.caches.keys()).not.toContain('gateway-web-v1.0.0-api')
    expect(env.self.clients.claim).toHaveBeenCalledTimes(1)
  })

  it('白名单接口在线时回源，写入缓存并记录写入时间戳', async () => {
    env.fetchMock.mockResolvedValue(netResponse({ ok: 1 }))
    const { intercepted, response } = await dispatchFetch(env, '/api/status')
    expect(intercepted).toBe(true)
    expect(response!.status).toBe(200)
    expect(await response!.text()).toBe(JSON.stringify({ ok: 1 }))
    // 新鲜网络响应绝不带陈旧标记
    expect(response!.headers.get('x-data-stale')).toBeNull()
    const entry = cacheBySuffix(env, '-api').entries.get(`${ORIGIN}/api/status`)
    expect(entry).toBeTruthy()
    expect(Number(entry!.headers['x-sw-cached-at'])).toBeGreaterThan(0)
  })

  it('离线时回退缓存，注入 X-Data-Stale 与 X-Data-Age（缓存年龄秒数）', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date('2026-10-26T10:00:00Z'))
    // 沙箱在脚本执行时捕获 Date 构造器，需在假时钟就位后重建环境
    env = makeEnv()
    env.fetchMock.mockResolvedValue(netResponse({ v: 1 }, { headers: { 'content-encoding': 'gzip' } }))
    await dispatchFetch(env, '/api/status')

    vi.setSystemTime(new Date('2026-10-26T10:01:30Z')) // +90s
    env.fetchMock.mockRejectedValue(new Error('offline'))
    const { response } = await dispatchFetch(env, '/api/status')

    expect(response!.status).toBe(200)
    expect(response!.headers.get('x-data-stale')).toBe('1')
    expect(response!.headers.get('x-data-age')).toBe('90')
    // 落盘时已剥掉压缩头（blob 是解码后的内容），不能把已解码 body 再标成 gzip
    expect(response!.headers.get('content-encoding')).toBeNull()
    expect(await response!.text()).toBe(JSON.stringify({ v: 1 }))
  })

  it('非白名单 API 完全不拦截：不回源、不写缓存', async () => {
    const r = await dispatchFetch(env, '/api/devices')
    expect(r.intercepted).toBe(false)
    expect(env.fetchMock).not.toHaveBeenCalled()
  })

  it('黑名单关键词优先于白名单前缀（如 /api/status/history）', async () => {
    const r = await dispatchFetch(env, '/api/status/history')
    expect(r.intercepted).toBe(false)
    expect(env.fetchMock).not.toHaveBeenCalled()
  })

  it('/health 探针与非 GET 请求不拦截', async () => {
    expect((await dispatchFetch(env, '/health/ready')).intercepted).toBe(false)
    expect((await dispatchFetch(env, '/api/status', 'POST')).intercepted).toBe(false)
    expect(env.fetchMock).not.toHaveBeenCalled()
  })

  it('超过 100KB 的响应不缓存，离线时回 503 而不是旧数据', async () => {
    const big = 'x'.repeat(101 * 1024)
    env.fetchMock.mockResolvedValue(netResponse(big))
    const online = await dispatchFetch(env, '/api/status')
    expect(online.response!.status).toBe(200)
    // 超限响应连 API cache 都不应创建条目（写入发生在 caches.open 之前的大小判定）
    const apiCache = [...env.caches.stores.entries()].find(([k]) => k.endsWith('-api'))
    expect(apiCache?.[1].entries.size ?? 0).toBe(0)

    env.fetchMock.mockRejectedValue(new Error('offline'))
    const offline = await dispatchFetch(env, '/api/status')
    expect(offline.response!.status).toBe(503)
    expect(await offline.response!.text()).toContain('离线模式')
  })

  it('缓存超过 TTL（24h）不再回退，并清除过期条目', async () => {
    vi.useFakeTimers({ toFake: ['Date'] })
    vi.setSystemTime(new Date('2026-10-26T10:00:00Z'))
    // 沙箱在脚本执行时捕获 Date 构造器，需在假时钟就位后重建环境
    env = makeEnv()
    env.fetchMock.mockResolvedValue(netResponse({ v: 1 }))
    await dispatchFetch(env, '/api/status')
    expect(cacheBySuffix(env, '-api').entries.size).toBe(1)

    vi.setSystemTime(new Date('2026-10-27T11:00:01Z')) // +25h
    env.fetchMock.mockRejectedValue(new Error('offline'))
    const { response } = await dispatchFetch(env, '/api/status')
    expect(response!.status).toBe(503)
    expect(cacheBySuffix(env, '-api').entries.size).toBe(0)
  })

  it('静态资源 cache-first：在线回填，离线返回已缓存外壳', async () => {
    env.fetchMock.mockResolvedValue(
      netResponse('console.log(1)', { headers: { 'content-type': 'text/javascript' } }),
    )
    const online = await dispatchFetch(env, '/assets/app-abc123.js')
    expect(online.intercepted).toBe(true)
    expect(await online.response!.text()).toBe('console.log(1)')
    await flushAsync()

    env.fetchMock.mockRejectedValue(new Error('offline'))
    const offline = await dispatchFetch(env, '/assets/app-abc123.js')
    expect(await offline.response!.text()).toBe('console.log(1)')
    // 外壳资源不涉及"数据陈旧"语义，不带 X-Data-Stale
    expect(offline.response!.headers.get('x-data-stale')).toBeNull()
  })
})
