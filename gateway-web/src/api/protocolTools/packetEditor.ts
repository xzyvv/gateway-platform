/**
 * 报文编辑器 API
 * POST /api/protocol-tools/packet-editor/send
 * GET  /api/protocol-tools/packet-editor/presets
 *
 * P0-1：真实下发。`bus` 为必填目标总线（BusConfig.port_name）；
 * 缺省 bus 由后端返回 400 + `error="missing_bus"`（过渡期 mock 回落已于 2026-10-25 移除），
 * 前端始终传 bus，不会触发该分支。
 *
 * P3-4：预置帧改为从后端读取（后端 PRESETS 是唯一定义），
 * 前端不再维护硬编码副本，避免两份常量随协议演进漂移。
 */
import { request } from '../index'
import type { SendResult } from './types'
import type { Preset, SendResult as SendResultRaw } from '../generated'
import { toSendResult } from './types'

/** 预置帧（name + hex） */
export type PacketPreset = Preset

/** 预置帧列表 */
export function listPresets(): Promise<PacketPreset[]> {
  return request<PacketPreset[]>({ url: '/api/protocol-tools/packet-editor/presets', method: 'GET' })
}

export function sendFrame(hex: string, bus: string, timeoutMs?: number): Promise<SendResult> {
  return request<SendResultRaw>({
    url: '/api/protocol-tools/packet-editor/send',
    method: 'POST',
    data: { hex, bus, ...(timeoutMs != null ? { timeout_ms: timeoutMs } : {}) },
  }).then(toSendResult)
}
