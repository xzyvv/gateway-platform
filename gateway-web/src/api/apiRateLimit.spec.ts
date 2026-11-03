import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import {
  listRules,
  createRule,
  updateRule,
  deleteRule,
  getRecentLogs,
  getStats,
  type ApiLimitRule,
} from './apiRateLimit'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
  params?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

function ruleRow(patch: Partial<ApiLimitRule> = {}): ApiLimitRule {
  return {
    id: 'r1',
    name: '写操作限速',
    path: '/api/points/*/write',
    method: 'POST',
    limitPerMinute: 60,
    limitPerHour: 1000,
    burst: 10,
    enabled: true,
    ...patch,
  }
}

describe('apiRateLimit（access 域限流规则 + 访问日志契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定规则列表路径：真实后端是 /api/access/rate-rules，
  // 不可与 /api/access/rate-logs（访问日志）混写。
  it('listRules 走 GET /api/access/rate-rules 并原样返回规则列表', async () => {
    const rows = [ruleRow()]
    requestMock.mockResolvedValue(rows)

    await expect(listRules()).resolves.toEqual(rows)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/access/rate-rules')
  })

  // 锁定创建请求体：整条规则透传（不含 id —— id 由后端生成）
  it('createRule 走 POST /api/access/rate-rules，请求体为整条规则', async () => {
    requestMock.mockResolvedValue(ruleRow())

    await createRule({
      name: '写操作限速',
      path: '/api/points/*/write',
      method: 'POST',
      limitPerMinute: 60,
      limitPerHour: 1000,
      burst: 10,
      enabled: true,
    })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/access/rate-rules')
    expect(cfg.data).toEqual({
      name: '写操作限速',
      path: '/api/points/*/write',
      method: 'POST',
      limitPerMinute: 60,
      limitPerHour: 1000,
      burst: 10,
      enabled: true,
    })
  })

  // 锁定全量更新语义：PUT + 显式字段白名单（不含 id）。
  // 字段名必须与后端 RateRuleReq 的 camelCase 对齐（limitPerMinute / limitPerHour）。
  it('updateRule 走 PUT /:id，请求体只含可编辑字段且不含 id', async () => {
    requestMock.mockResolvedValue(ruleRow({ name: '改名后' }))

    await updateRule(ruleRow({ name: '改名后' }))

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('PUT')
    expect(cfg.url).toBe('/api/access/rate-rules/r1')
    expect(cfg.data).toEqual({
      name: '改名后',
      path: '/api/points/*/write',
      method: 'POST',
      limitPerMinute: 60,
      limitPerHour: 1000,
      burst: 10,
      enabled: true,
    })
    expect((cfg.data as Record<string, unknown>).id).toBeUndefined()
  })

  it('deleteRule 走 DELETE /api/access/rate-rules/:id', async () => {
    requestMock.mockResolvedValue(undefined)

    await deleteRule('r1')

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('DELETE')
    expect(cfg.url).toBe('/api/access/rate-rules/r1')
  })

  // 回归（W3a-2）：规则 id 同样必须 URL 转义（与 apiKeys 一致）
  it('updateRule / deleteRule 对 id 做 URL 编码', async () => {
    requestMock.mockResolvedValue(undefined)

    await updateRule(ruleRow({ id: 'a/b?c' }))
    expect(firstCallConfig().url).toBe('/api/access/rate-rules/a%2Fb%3Fc')

    requestMock.mockReset()
    requestMock.mockResolvedValue(undefined)
    await deleteRule('a/b?c')
    expect(firstCallConfig().url).toBe('/api/access/rate-rules/a%2Fb%3Fc')
  })

  // 回归（W3a-2）：后端在无访问日志时可能返回 null/缺省，
  // 必须回退空数组而不是对 null 直接 map（与 auditLog 防御口径一致）
  it('getRecentLogs 在响应为 null 时回退空数组', async () => {
    requestMock.mockResolvedValue(null)

    await expect(getRecentLogs()).resolves.toEqual([])
  })

  // 锁定访问日志路径与默认条数：GET /api/access/rate-logs?limit=100。
  // 默认值与后端 LogsQuery::default_log_limit 一致（100）。
  it('getRecentLogs 默认走 GET /api/access/rate-logs?limit=100', async () => {
    requestMock.mockResolvedValue([])

    await getRecentLogs()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/access/rate-logs')
    expect(cfg.params).toEqual({ limit: 100 })
  })

  it('getRecentLogs 传入自定义条数时 params.limit 随之改变', async () => {
    requestMock.mockResolvedValue([])

    await getRecentLogs(20)

    const cfg = firstCallConfig()
    expect(cfg.params).toEqual({ limit: 20 })
  })

  // 锁定字段映射：后端返回 ts（时间戳），前端映射为页面惯用的 time。
  // 这一步是最易回退的点（改成直传 ts 会让表格时间列全空）。
  it('getRecentLogs 把后端 ts 字段映射为 time，其余字段逐位透传', async () => {
    requestMock.mockResolvedValue([
      {
        id: 9,
        ts: 1700000000000,
        method: 'GET',
        path: '/api/points',
        status: 200,
        latencyMs: 12,
        clientIp: '192.168.1.10',
        limited: false,
      },
    ])

    const rows = await getRecentLogs()

    expect(rows).toEqual([
      {
        id: 9,
        time: 1700000000000,
        method: 'GET',
        path: '/api/points',
        status: 200,
        latencyMs: 12,
        clientIp: '192.168.1.10',
        limited: false,
      },
    ])
    // 不应把后端原字段 ts 一并透出（避免页面两套时间字段并存）
    expect((rows[0] as unknown as Record<string, unknown>).ts).toBeUndefined()
  })

  // 锁定统计路径：/api/access/rate-stats（不是 /api/access/rate-rules/stats）
  it('getStats 走 GET /api/access/rate-stats 并原样返回统计', async () => {
    const stats = { totalRequests: 120, limitedRequests: 3, avgLatencyMs: 18 }
    requestMock.mockResolvedValue(stats)

    await expect(getStats()).resolves.toEqual(stats)

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/access/rate-stats')
  })

  // 错误分支：后端 400（限额非法）原样 reject，由页面展示后端中文文案
  it('后端报错时 listRules 原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(400): 每分钟/每小时限额必须 >= 1'))

    await expect(listRules()).rejects.toThrow('限额必须 >= 1')
  })
})
