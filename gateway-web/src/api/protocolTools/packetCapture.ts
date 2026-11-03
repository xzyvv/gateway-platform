/**
 * 抓包 API（start/stop 为前端状态管理，后端持续生成报文）
 * GET  /api/protocol-tools/packet-capture/packets
 * POST /api/protocol-tools/packet-capture/clear
 */
import { request } from '../index'
import type { Packet } from './types'
import type { Packet as PacketRaw } from '../generated'
import { toPacket } from './types'

// 抓包进度是纯前端按钮态：后端持续缓存报文，start/stop 不参与任何请求，
// 因此无需保存状态（避免出现"看起来在抓、实际没抓"的误解）。
export function startCapture(_filterDevice?: string): void { /* 后端持续缓存报文 */ }
export function stopCapture(): void { /* 同上，无前端状态需清理 */ }
export function getPackets(limit = 100, deviceId?: string): Promise<Packet[]> {
  return request<PacketRaw[]>({ url: '/api/protocol-tools/packet-capture/packets', method: 'GET', params: { limit, device_id: deviceId } }).then((arr) => arr.map(toPacket))
}
export function clearPackets(): Promise<void> {
  return request<void>({ url: '/api/protocol-tools/packet-capture/clear', method: 'POST' })
}