/**
 * API 调用限流（网关真实后端：/api/access/rate-rules、rate-stats、rate-logs）
 *
 * 规则持久化在网关 SQLite，由限流中间件真实执行：
 * 分钟窗口允许 limitPerMinute + burst 次、小时窗口 limitPerHour 次，
 * 同一请求命中多条规则时任一超限即拒（最严生效），超限返回 429 + Retry-After。
 */
import { request } from './index'

export interface ApiLimitRule {
  id: string
  name: string
  path: string
  method: string
  limitPerMinute: number
  limitPerHour: number
  burst: number
  enabled: boolean
}

export interface ApiLog {
  id: number
  time: number
  path: string
  method: string
  status: number
  latencyMs: number
  clientIp: string
  limited: boolean
}

export interface RateLimitStats {
  totalRequests: number
  limitedRequests: number
  avgLatencyMs: number
}

/** 后端访问日志行：时间戳字段为 ts，这里映射为页面惯用的 time */
interface RawAccessLog {
  id: number
  ts: number
  method: string
  path: string
  status: number
  latencyMs: number
  clientIp: string
  limited: boolean
}

export function listRules(): Promise<ApiLimitRule[]> {
  return request({ url: '/api/access/rate-rules', method: 'GET' })
}

export function createRule(rule: Omit<ApiLimitRule, 'id'>): Promise<ApiLimitRule> {
  return request({ url: '/api/access/rate-rules', method: 'POST', data: rule })
}

/** 后端为全量更新（PUT 语义），调用方传入完整规则行 */
export function updateRule(rule: ApiLimitRule): Promise<ApiLimitRule> {
  return request({
    url: `/api/access/rate-rules/${encodeURIComponent(rule.id)}`,
    method: 'PUT',
    data: {
      name: rule.name,
      path: rule.path,
      method: rule.method,
      limitPerMinute: rule.limitPerMinute,
      limitPerHour: rule.limitPerHour,
      burst: rule.burst,
      enabled: rule.enabled,
    },
  })
}

export function deleteRule(id: string): Promise<void> {
  return request({ url: `/api/access/rate-rules/${encodeURIComponent(id)}`, method: 'DELETE' })
}

export async function getRecentLogs(limit = 100): Promise<ApiLog[]> {
  const rows = await request<RawAccessLog[] | null | undefined>({
    url: '/api/access/rate-logs',
    method: 'GET',
    params: { limit },
  })
  // 后端在「尚未写入任何访问日志」时会返回 null/缺省，与 auditLog 的
  // `r?.logs ?? []` 保持同一防御口径，避免页面直接 map 空值崩掉。
  return (rows ?? []).map((r) => ({
    id: r.id,
    time: r.ts,
    method: r.method,
    path: r.path,
    status: r.status,
    latencyMs: r.latencyMs,
    clientIp: r.clientIp,
    limited: r.limited,
  }))
}

export function getStats(): Promise<RateLimitStats> {
  return request({ url: '/api/access/rate-stats', method: 'GET' })
}
