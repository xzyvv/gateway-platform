/**
 * Webhook 测试 API
 *
 * 路由（与后端 gateway-api/src/web/api/api_system/handlers/webhook.rs 对齐）：
 * - POST /api/webhook/test -> testWebhook
 */
import { request } from './index'

export interface WebhookTestRequest {
  url: string
  method: 'POST' | 'PUT' | 'GET'
  headers: Record<string, string>
  body: string
}

export interface WebhookTestResponse {
  ok: boolean
  status: number
  status_text: string
  latency: number
  response_body: string
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