/**
 * Webhook 测试 API
 *
 * 路由（与后端 gateway-api/src/web/api/api_system/handlers/webhook.rs 对齐）：
 * - POST /api/webhook/test -> testWebhook
 */
import { request } from '../index'
import type { WebhookTestResponse as WebhookTestResponseRaw } from '../generated/WebhookTestResponse'

export interface WebhookTestRequest {
  url: string
  method: 'POST' | 'PUT' | 'GET'
  headers: Record<string, string>
  body: string
}

/**
 * 视图类型从 ts-rs generated 契约派生：字段名以契约为真源，
 * 仅覆盖视图差异——bigint latency 转 number、可空 error 转可选字段。
 * response_headers 契约为可选索引签名，收窄为必填 Record 供页面直接读取。
 */
export type WebhookTestResponse = Omit<WebhookTestResponseRaw, 'latency' | 'response_headers' | 'error'> & {
  latency: number
  response_headers: Record<string, string>
  error?: string
}

export function testWebhook(req: WebhookTestRequest): Promise<WebhookTestResponse> {
  return request<WebhookTestResponse>({
    url: '/api/webhook/test',
    method: 'POST',
    data: req,
  })
}