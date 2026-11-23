/**
 * 看门狗与自愈 API
 *
 * 路由（与后端 gateway-api/src/web/api/api_system/handlers/watchdog.rs 对齐）：
 * - GET  /api/watchdog/services              -> listServices
 * - GET  /api/watchdog/events                -> listHealEvents
 * - POST /api/watchdog/services/:name/restart -> restartService
 */
import { request } from './index'
import type { ServiceInfo as ServiceInfoDto } from './generated/ServiceInfo'
import type { HealEvent as HealEventRaw } from './generated/HealEvent'
import type { HealAction } from './generated/HealAction'
import type { ServiceStatus } from './generated/ServiceStatus'

export type { ServiceStatus } from './generated/ServiceStatus'
export type { HealAction } from './generated/HealAction'

/**
 * 服务行的前端视图：后端 `u64` 在 ts-rs 里是 `bigint`，而 JSON 实际传的是
 * number；统一在 API 边界转成 number，页面与自检逻辑不必再做大整数运算。
 * 字段口径见后端 `ServiceInfo`（DD-37：不再复用 `restarts` 承载多种计数）。
 */
export interface ServiceInfo {
  name: string
  status: ServiceStatus
  pid: number
  uptime: number
  cpu: number
  mem: number
  panic_count: number
  publish_fail_count: number
  collect_fail_count: number
  heartbeat_stall_count: number
}

/**
 * 视图类型从 ts-rs generated 契约派生：action 直接复用 generated 的 HealAction，
 * 仅覆盖两处视图差异——bigint 时间戳转 number、可空 detail 转可选字段。
 */
export type HealEvent = Omit<HealEventRaw, 'time' | 'action' | 'detail'> & {
  time: number
  action: HealAction
  detail?: string
}

export async function listServices(): Promise<ServiceInfo[]> {
  const rows = await request<ServiceInfoDto[]>({
    url: '/api/watchdog/services',
    method: 'GET',
  })
  return rows.map((r) => ({
    name: r.name,
    status: r.status,
    pid: Number(r.pid),
    uptime: Number(r.uptime),
    cpu: r.cpu,
    mem: Number(r.mem),
    panic_count: Number(r.panic_count),
    publish_fail_count: Number(r.publish_fail_count),
    collect_fail_count: Number(r.collect_fail_count),
    heartbeat_stall_count: Number(r.heartbeat_stall_count),
  }))
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