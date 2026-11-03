/**
 * 协议参数探测 API（p0-5：真实总线验证式探测，start→poll）
 * POST /api/protocol-tools/auto-detect/run
 * GET  /api/protocol-tools/auto-detect/run/:id
 * POST /api/protocol-tools/auto-detect/run/:id/stop
 */
import { request } from '../index'
import type { AutoDetectResult } from './types'
import type { AutoDetectResult as AutoDetectResultRaw } from '../generated'
import { toAutoDetectResult } from './types'

/** 启动探测，返回任务 id（最坏 7 帧约 4s，前端轮询 GET run/:id） */
export function startAutoDetect(deviceId: string): Promise<string> {
  return request<string>({ url: '/api/protocol-tools/auto-detect/run', method: 'POST', data: { device_id: deviceId } })
}

export function getAutoDetectResult(id: string): Promise<AutoDetectResult | null> {
  return request<AutoDetectResultRaw | null>({ url: '/api/protocol-tools/auto-detect/run/' + encodeURIComponent(id), method: 'GET' }).then((r) => r ? toAutoDetectResult(r) : null)
}

/** 中止探测：任务在下一探测项前停止并释放总线租约 */
export function stopAutoDetect(id: string): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/auto-detect/run/' + encodeURIComponent(id) + '/stop', method: 'POST' })
}
