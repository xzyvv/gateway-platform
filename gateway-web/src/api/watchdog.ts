/**
 * 看门狗与自愈 API
 *
 * 路由（与后端 gateway-api/src/web/api/api_system/handlers/watchdog.rs 对齐）：
 * - GET  /api/watchdog/services              -> listServices
 * - GET  /api/watchdog/events                -> listHealEvents
 * - POST /api/watchdog/services/:name/restart -> restartService
 */
import { request } from './index'

export type ServiceStatus = 'Running' | 'Restarting' | 'Stopped' | 'Unknown'
export type HealAction = 'RestartService' | 'ResetSerial' | 'ResetNetwork'

export interface ServiceInfo {
  name: string
  status: ServiceStatus
  pid: number
  uptime: number
  cpu: number
  mem: number
  restarts: number
}

export interface HealEvent {
  id: string
  time: number
  action: HealAction
  target: string
  reason: string
  result: string
  detail?: string
}

export function listServices(): Promise<ServiceInfo[]> {
  return request<ServiceInfo[]>({ url: '/api/watchdog/services', method: 'GET' })
}

export function listHealEvents(): Promise<HealEvent[]> {
  return request<HealEvent[]>({ url: '/api/watchdog/events', method: 'GET' })
}

export function restartService(name: string): Promise<void> {
  return request<void>({
    url: `/api/watchdog/services/${encodeURIComponent(name)}/restart`,
    method: 'POST',
  })
}

export const ACTION_LABEL: Record<HealAction, string> = {
  RestartService: '重启服务',
  ResetSerial: '重置串口',
  ResetNetwork: '重置网络',
}

export const STATUS_LABEL: Record<ServiceStatus, string> = {
  Running: '运行中',
  Restarting: '重启中',
  Stopped: '已停止',
  Unknown: '未知',
}

export const STATUS_TYPE: Record<ServiceStatus, 'success' | 'warning' | 'danger' | 'info'> = {
  Running: 'success',
  Restarting: 'warning',
  Stopped: 'danger',
  Unknown: 'info',
}