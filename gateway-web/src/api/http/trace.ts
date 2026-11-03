/**
 * 响应元信息读取
 *
 * - trace ID（`x-request-id`）：后端对**每个**响应（含限速拒绝 / panic 兜底）写入，
 *   前端任何失败分支都能拿到它；用户复制诊断信息后，研发按 ID 可直接在网关日志中定位请求。
 * - 离线陈旧标记（`X-Data-Stale` / `X-Data-Age`）：Service Worker 在网关不可达时回退到
 *   API 缓存并注入，页面据此区分「真实新数据」与「离线旧数据」，不再把旧快照当成刚刚更新。
 */

/** 读取单个响应头（兼容 AxiosHeaders.get 与普通对象两种形态），空串视为缺失 */
function readHeader(headers: unknown, name: string): string | undefined {
  if (!headers || typeof headers !== 'object') return undefined
  const get = (headers as { get?: (name: string) => unknown }).get
  const raw = typeof get === 'function'
    ? get.call(headers, name)
    : (headers as Record<string, unknown>)[name]
  const value = Array.isArray(raw) ? raw[0] : raw
  if (typeof value !== 'string') return undefined
  const trimmed = value.trim()
  return trimmed || undefined
}

/**
 * 从响应头读取后端 trace ID（`x-request-id`，见 gateway-api 的 request_id 中间件）。
 *
 * 该头由后端对**每个**响应（含限速拒绝 / panic 兜底）写入，因此前端任何失败分支
 * 都能拿到它；用户复制诊断信息后，研发按 ID 可直接在网关日志中定位请求。
 *
 * 可见性：**仅供测试**（http.spec.ts）与本模块内部使用，业务代码请勿引用。
 */
export function readTraceId(headers: unknown): string | undefined {
  return readHeader(headers, 'x-request-id')
}

/**
 * 离线陈旧元信息。
 *
 * Service Worker 在网关不可达时回退到 API 缓存，并注入
 * `X-Data-Stale: 1` 与 `X-Data-Age`（缓存写入距今秒数）。
 * 页面据此区分「真实新数据」与「离线旧数据」，不再把旧快照当成刚刚更新。
 */
export interface ResponseMeta {
  stale: boolean
  staleAgeSeconds: number | null
}

/** 仅 requestWithMeta 使用，不对外导出 */
export function readResponseMeta(headers: unknown): ResponseMeta {
  const ageRaw = readHeader(headers, 'x-data-age')
  const age = ageRaw === undefined ? NaN : Number(ageRaw)
  return {
    stale: readHeader(headers, 'x-data-stale') === '1',
    staleAgeSeconds: Number.isFinite(age) ? Math.max(0, Math.round(age)) : null,
  }
}
