/**
 * 远程控制（点位写入）API
 *
 * 路由：
 * - GET  /api/devices/control/writable -> listWritablePoints
 * - GET  /api/devices/control/commands -> listCommands
 *
 * 写操作已统一走 `writePoints`（@/api/points 的 `POST /api/point-tools/write`，B2 单一入口），
 * 本模块不再提供 `sendCommand` 这类第二写入口。
 */
import { request } from './index'
import type { WritablePoint as WritablePointRaw } from './generated/WritablePoint'
import type { ControlCommand as ControlCommandRaw } from './generated/ControlCommand'

// 字段与 generated 完全一致，直接复用契约类型
export type WritablePoint = WritablePointRaw

/**
 * 视图类型从 ts-rs generated 契约派生：字段名以契约为真源，
 * 仅覆盖两处视图差异——status 收窄为字面量联合、bigint 时间戳转 number。
 */
export type ControlCommand = Omit<ControlCommandRaw, 'status' | 'created_at' | 'executed_at'> & {
  status: 'success' | 'failed' | 'uncertain' | 'pending'
  created_at: number
  executed_at: number | null
}

export function listWritablePoints(): Promise<WritablePoint[]> {
  return request<WritablePoint[]>({ url: '/api/devices/control/writable', method: 'GET' })
}

export function listCommands(): Promise<ControlCommand[]> {
  return request<ControlCommand[]>({ url: '/api/devices/control/commands', method: 'GET' })
}
