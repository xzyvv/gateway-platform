/**
 * 远程控制（点位写入）API
 *
 * 路由：
 * - GET  /api/devices/control/writable -> listWritablePoints
 * - GET  /api/devices/control/commands -> listCommands
 *
 * 写操作已统一走 `writePoints`（@/api/points 的 `POST /api/points/write`，B2 单一入口），
 * 本模块不再提供 `sendCommand` 这类第二写入口。
 */
import { request } from './index'

export interface WritablePoint {
  id: string
  name: string
  data_type: string
  device_id: string
  device_name: string
}

export interface ControlCommand {
  id: string
  device_id: string
  device_name: string
  point_id: string
  point_name: string
  write_value: number
  operator: string
  status: 'success' | 'failed' | 'pending'
  created_at: number
  executed_at: number | null
  result: string | null
}

export function listWritablePoints(): Promise<WritablePoint[]> {
  return request<WritablePoint[]>({ url: '/api/devices/control/writable', method: 'GET' })
}

export function listCommands(): Promise<ControlCommand[]> {
  return request<ControlCommand[]>({ url: '/api/devices/control/commands', method: 'GET' })
}
