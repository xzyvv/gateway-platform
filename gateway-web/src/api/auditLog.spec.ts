import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { listAuditLogs } from './auditLog'

interface RequestConfig {
  url: string
  method: string
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('auditLog（system 域后端审计日志契约）', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定路径：无过滤条件时必须是裸 /api/audit/logs（不能带多余的 '?'）
  it('无过滤条件时走 GET /api/audit/logs（不带查询串）', async () => {
    requestMock.mockResolvedValue({ logs: [] })

    await listAuditLogs()

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('GET')
    expect(cfg.url).toBe('/api/audit/logs')
    expect(cfg.url).not.toContain('?')
  })

  // 默认参数等价于无过滤：listAuditLogs() 与 listAuditLogs({}) 请求完全一致
  it('默认参数与显式空对象产生相同的请求 URL', async () => {
    requestMock.mockResolvedValue({ logs: [] })

    await listAuditLogs({})

    const cfg = firstCallConfig()
    expect(cfg.url).toBe('/api/audit/logs')
  })

  // 锁定过滤参数名与拼装顺序：后端 AuditQuery 字段为 action/target/source/ts_from/ts_to/limit，
  // 写成 camelCase（tsFrom）后端会当成未知参数丢弃 → 过滤静默失效。
  it('按 action/target/source/ts_from/ts_to/limit 拼装查询串', async () => {
    requestMock.mockResolvedValue({ logs: [] })

    await listAuditLogs({
      action: 'config.update',
      target: 'modbus-1',
      source: 'web',
      ts_from: 1000,
      ts_to: 2000,
      limit: 50,
    })

    const cfg = firstCallConfig()
    expect(cfg.url).toBe(
      '/api/audit/logs?action=config.update&target=modbus-1&source=web&ts_from=1000&ts_to=2000&limit=50',
    )
  })

  // 边界：ts_from / ts_to / limit 用 != null 判定，0 是有效值必须带上；
  // 而 action/target/source 用真值判定，空串视为「不过滤」。
  it('时间戳为 0 时仍带上（0 是有效 epoch），空串过滤条件被忽略', async () => {
    requestMock.mockResolvedValue({ logs: [] })

    await listAuditLogs({ action: '', target: '', source: '', ts_from: 0, ts_to: 0, limit: 0 })

    const cfg = firstCallConfig()
    expect(cfg.url).toBe('/api/audit/logs?ts_from=0&ts_to=0&limit=0')
    expect(cfg.url).not.toContain('action=')
    expect(cfg.url).not.toContain('target=')
    expect(cfg.url).not.toContain('source=')
  })

  // 边界：查询值含特殊字符时必须按 URL 编码转义，避免拼出非法 URL
  it('过滤值含特殊字符时按 URL 编码转义', async () => {
    requestMock.mockResolvedValue({ logs: [] })

    await listAuditLogs({ action: 'a b&c=d' })

    const cfg = firstCallConfig()
    expect(cfg.url).toBe('/api/audit/logs?action=a+b%26c%3Dd')
  })

  // 锁定返回结构：后端返回 { logs: [...] }，前端只取 logs 数组（不是整个响应体）
  it('只解出 logs 数组，其余字段不外泄', async () => {
    requestMock.mockResolvedValue({
      logs: [
        { ts: 1700000000000, action: 'config.update', target: 'cfg', detail: 'd', source: 'web' },
      ],
      extra: 'ignored',
    })

    const rows = await listAuditLogs()

    expect(rows).toEqual([
      { ts: 1700000000000, action: 'config.update', target: 'cfg', detail: 'd', source: 'web' },
    ])
  })

  // 边界：后端返回空对象 / null 时退化为空数组，调用方无需判空
  it('后端未返回 logs 字段时退化为空数组', async () => {
    requestMock.mockResolvedValue({})
    await expect(listAuditLogs()).resolves.toEqual([])

    requestMock.mockResolvedValue(null)
    await expect(listAuditLogs()).resolves.toEqual([])
  })

  // 错误分支：后端 400（ts_from > ts_to）原样 reject，由页面提示
  it('后端报错时原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('请求失败(400): ts_from (2000) > ts_to (1000)'))

    await expect(listAuditLogs({ ts_from: 2000, ts_to: 1000 })).rejects.toThrow('ts_from')
  })
})
