/**
 * Service Worker - 工业网关调试控制台离线模式
 *
 * 策略：
 * - 应用外壳（静态资源）：cache-first（离线可访问）
 * - API 白名单小接口（/api/status、/api/version）：network-first，仅在网关不可达时
 *   回退缓存，并注入 `X-Data-Stale: 1` + `X-Data-Age`，前端据此显示「离线旧数据」
 * - 其余 API（history / logs 等大响应）：完全不拦截，交给浏览器。避免每 3s 轮询
 *   把同一份响应反复写盘（写放大），也避免把历史/日志灌进 Cache Storage
 * - 健康检查（/health/*）：network-only，不缓存（离线横幅依赖它的真实失败）
 *
 * 版本：v1.0.1
 * 更新策略：CACHE_VERSION 变化时激活 SW 后清理旧缓存
 */

const CACHE_VERSION = 'gateway-web-v1.0.1'
const APP_SHELL_CACHE = `${CACHE_VERSION}-shell`
const API_CACHE = `${CACHE_VERSION}-api`
const SHELL_ASSETS = [
  '/',
  '/index.html',
  '/manifest.json',
  '/favicon.svg',
]

/** 仅这些 API 允许进缓存：小、纯读、离线时仍有参考价值 */
const API_CACHE_ALLOWLIST = ['/api/status', '/api/version']
/** 防御性黑名单：历史/日志类接口体积大、时效强，即使误加入白名单也不缓存 */
const API_CACHE_DENYLIST = ['history', 'logs', 'audit', 'metrics']
/** 单个响应体上限（解码后字节数），超过则不缓存 */
const API_CACHE_MAX_BYTES = 100 * 1024
/** 离线回退有效期：超过该时长的缓存不再返回，避免把一天前的状态当现状展示 */
const API_CACHE_TTL_MS = 24 * 60 * 60 * 1000
/** 缓存写入时间戳（内部头，回退时转成 X-Data-Age 暴露给页面） */
const SW_CACHED_AT = 'x-sw-cached-at'

// 安装：预缓存应用外壳
self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(APP_SHELL_CACHE)
      .then((cache) => cache.addAll(SHELL_ASSETS).catch(() => {}))
      .then(() => self.skipWaiting())
  )
})

// 激活：清理旧版本缓存
self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys()
      .then((keys) => Promise.all(
        keys
          .filter((key) => !key.startsWith(CACHE_VERSION))
          .map((key) => caches.delete(key))
      ))
      .then(() => self.clients.claim())
  )
})

// 请求拦截：按策略分流
self.addEventListener('fetch', (event) => {
  const { request } = event
  const url = new URL(request.url)

  // 同源请求才处理（跨域请求交给浏览器默认行为）
  if (url.origin !== self.location.origin) return

  // 健康检查：network-only
  if (url.pathname.startsWith('/health')) return

  // 非 GET（WebSocket 无法被 Service Worker 缓存）：不拦截
  if (request.method !== 'GET') return

  // API 请求：只有白名单小接口才纳入 SW，其余直接放行
  if (url.pathname.startsWith('/api/')) {
    if (!isCacheableApiPath(url.pathname)) return
    event.respondWith(networkFirst(request))
    return
  }

  // 静态资源：cache-first
  event.respondWith(cacheFirst(request, APP_SHELL_CACHE))
})

/**
 * API 是否允许进缓存：黑名单关键词优先，其次必须命中白名单（含其子路径）。
 * 未命中白名单的请求完全不经过 SW，自然没有缓存与写盘。
 */
function isCacheableApiPath(pathname) {
  if (API_CACHE_DENYLIST.some((keyword) => pathname.includes(keyword))) return false
  return API_CACHE_ALLOWLIST.some((p) => pathname === p || pathname.startsWith(`${p}/`))
}

/**
 * cache-first：先查缓存，未命中再请求网络并回填缓存
 */
async function cacheFirst(request, cacheName) {
  const cached = await caches.match(request)
  if (cached) return cached
  try {
    const resp = await fetch(request)
    if (resp && resp.status === 200 && resp.type === 'basic') {
      const cache = await caches.open(cacheName)
      cache.put(request, resp.clone())
    }
    return resp
  } catch {
    // 离线且无缓存：返回一个基础离线页面
    return new Response('离线模式：资源未缓存', { status: 503, statusText: 'Offline' })
  }
}

/**
 * network-first：先请求网络，失败时回退缓存并标记为陈旧数据。
 *
 * 缓存体量受两道闸门约束：路径白名单 + 单响应 <= 100KB；离线回退受 TTL 约束。
 */
async function networkFirst(request) {
  try {
    const resp = await fetch(request)
    if (resp && resp.status === 200 && resp.type === 'basic') {
      // 白名单接口都是小 JSON，等待落盘的代价远小于"每 3s 轮询写放大"
      await cacheIfSmall(request, resp.clone())
    }
    return resp
  } catch {
    const cached = await caches.match(request)
    if (cached) {
      const cachedAt = Number(cached.headers.get(SW_CACHED_AT) || 0)
      const ageMs = cachedAt > 0 ? Date.now() - cachedAt : Number.POSITIVE_INFINITY
      if (ageMs <= API_CACHE_TTL_MS) return asStaleResponse(cached, ageMs)
      // 缓存已过期：不再冒充实时状态，清掉后走离线 503
      const cache = await caches.open(API_CACHE)
      await cache.delete(request)
    }
    return new Response('{"error":"离线模式，API 不可用"}', {
      status: 503,
      headers: { 'Content-Type': 'application/json' },
    })
  }
}

/** 响应体不超过上限才写入缓存，并记录写入时间戳（TTL 判定依据） */
async function cacheIfSmall(request, resp) {
  try {
    const body = await resp.blob()
    if (body.size > API_CACHE_MAX_BYTES) return
    const headers = new Headers(resp.headers)
    // blob 已是解码后的内容，去掉编码/长度头，避免浏览器按 gzip 二次解码
    headers.delete('content-encoding')
    headers.delete('content-length')
    headers.set(SW_CACHED_AT, String(Date.now()))
    const cache = await caches.open(API_CACHE)
    await cache.put(request, new Response(body, {
      status: resp.status,
      statusText: resp.statusText,
      headers,
    }))
  } catch {
    // 缓存失败（配额/并发删除）不影响本次响应
  }
}

/** 把缓存响应包装成「陈旧数据」：带 X-Data-Stale 与缓存年龄，供前端展示 */
async function asStaleResponse(cached, ageMs) {
  const body = await cached.blob()
  const headers = new Headers(cached.headers)
  headers.delete(SW_CACHED_AT)
  headers.delete('content-encoding')
  headers.delete('content-length')
  headers.set('X-Data-Stale', '1')
  headers.set('X-Data-Age', String(Math.max(0, Math.round(ageMs / 1000))))
  return new Response(body, {
    status: cached.status,
    statusText: cached.statusText,
    headers,
  })
}
