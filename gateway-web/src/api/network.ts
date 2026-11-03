import { request } from './index'

/** NTP 状态（/api/ntp/status 响应） */
export interface NtpStatus {
  /** idle（尚未校时）/ synced / error: <原因> */
  status: string
  /** 最近一次成功校时的 Unix 秒级时间戳；从未成功时为 null */
  last_sync?: number | null
  /** 时间源：NTP 或 RTC（校时失败时回退 RTC） */
  source?: string
  /** NTP 时间与本地时间差（毫秒，NTP - 本地）；正值表示本地落后 */
  offset_ms?: number
}

/** NTP 状态 */
export function getNtpStatus(): Promise<NtpStatus> {
  return request({ url: '/api/ntp/status', method: 'GET' })
}

/** NTP 同步 */
export function ntpSync(): Promise<{ status: string }> {
  return request({ url: '/api/ntp/sync', method: 'POST' })
}

/** B3 蜂窝网络状态 */
export interface CellularStatus {
  enabled: boolean
  /** 当前平台是否支持 4G 拨号（pppd，仅 Linux）；false 时仅链路监控/故障切换 */
  dial_supported: boolean
  /** 运行平台标识，如 linux / windows */
  platform: string
  status: {state: string; signal_strength: number; operator: string; ip_address: string|null; network_type: string; updated_at: number}|null
  link_state: string|null
}
export function getCellularStatus(): Promise<CellularStatus> {
  return request({ url: '/api/cellular/status', method: 'GET' })
}

