import { describe, it, expect, vi, beforeEach } from 'vitest'

// request 经 vi.hoisted 提前声明，供 vi.mock 工厂引用（工厂会被提升到 import 之前）
const { requestMock } = vi.hoisted(() => ({ requestMock: vi.fn() }))
vi.mock('./index', () => ({ request: requestMock }))

import { testWebhook, type WebhookTestRequest, type WebhookTestResponse } from './webhook'

interface RequestConfig {
  url: string
  method: string
  data?: unknown
}

function firstCallConfig(): RequestConfig {
  return requestMock.mock.calls[0]![0] as RequestConfig
}

describe('webhook 测试契约', () => {
  // 重置 mock 必须用花括号块：mockReset() 的返回值就是 mock 本身，箭头简写会把它
  // 交给 Vitest 5 当作 beforeEach 的「清理回调」，用例结束后再调一次 mock，
  // 残留的 mockRejectedValue 会在清理阶段抛出未处理的 rejection，把用例判红。
  beforeEach(() => {
    requestMock.mockReset()
  })

  // 锁定端点契约：唯一端点是 POST /api/webhook/test
  // （易回退点：不能写成 /api/webhook/test/send 或 /api/webhooks/test）
  it('Webhook 测试走 POST /api/webhook/test', async () => {
    requestMock.mockResolvedValue({ ok: true, status: 200, status_text: 'OK', latency: 12, response_body: 'pong', response_headers: {} })

    await testWebhook({ url: 'https://example.com/hook', method: 'POST', headers: {}, body: 'ping' })

    const cfg = firstCallConfig()
    expect(cfg.method).toBe('POST')
    expect(cfg.url).toBe('/api/webhook/test')
  })

  // 锁定请求体契约：url / method / headers / body 原样透传（后端按此四项转发）
  it('请求体原样透传 url / method / headers / body', async () => {
    const req: WebhookTestRequest = {
      url: 'https://example.com/hook',
      method: 'PUT',
      headers: { 'X-Token': 'abc', 'Content-Type': 'application/json' },
      body: '{"a":1}',
    }
    requestMock.mockResolvedValue({ ok: true, status: 200, status_text: 'OK', latency: 5, response_body: '{}', response_headers: {} })

    await testWebhook(req)

    expect(firstCallConfig().data).toEqual(req)
  })

  // 锁定成功响应契约：状态码 / 响应头 / 响应体原样透传给调用方
  it('成功响应原样透传（status / latency / response_body / response_headers）', async () => {
    const resp: WebhookTestResponse = {
      ok: true,
      status: 201,
      status_text: 'Created',
      latency: 42,
      response_body: '{"ok":1}',
      response_headers: { 'content-type': 'application/json' },
    }
    requestMock.mockResolvedValue(resp)

    await expect(testWebhook({ url: 'https://example.com/hook', method: 'GET', headers: {}, body: '' })).resolves.toEqual(resp)
  })

  // 锁定失败分支：后端返回 ok=false + error 时原样透传，不在 api 层判定成败
  it('失败响应（ok=false + error）原样透传', async () => {
    const resp: WebhookTestResponse = {
      ok: false,
      status: 0,
      status_text: '',
      latency: 0,
      response_body: '',
      response_headers: {},
      error: '连接超时',
    }
    requestMock.mockResolvedValue(resp)

    await expect(testWebhook({ url: 'https://example.com/hook', method: 'POST', headers: {}, body: '' })).resolves.toEqual(resp)
  })

  // 锁定错误分支：网络层异常时 promise reject 原样抛出（不在 api 层吞掉）
  it('请求失败时原样 reject', async () => {
    requestMock.mockRejectedValue(new Error('网关不可达'))

    await expect(testWebhook({ url: 'https://example.com/hook', method: 'POST', headers: {}, body: '' })).rejects.toThrow('网关不可达')
  })
})
