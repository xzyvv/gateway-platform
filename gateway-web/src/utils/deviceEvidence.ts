/**
 * 设备维度证据聚合（诊断闭环，P2 #5）
 *
 * 目标：让「某设备为什么离线」在一屏内可答。
 *
 * 现状缺口：设备列表只有「最后成功采集」「通信质量」两个结论性标签，
 * 没有支撑判断的原始证据。运维看到「离线」时无法区分是：
 *   - 设备真坏了（无响应）
 *   - 响应了但 CRC 错 / 异常码（协议层问题）
 *   - 根本没发出请求（调度/退避问题）
 *
 * 本模块把三类证据按 device_id 聚合到一处：
 *   1. 最后成功采集时间（来自 /api/devices/backoff 的 last_good_ts）
 *   2. 连续失败次数（来自 /api/devices/backoff 的 consecutive_failures）
 *   3. 最近原始帧 hex（来自 /api/protocol-tools/packet-capture/packets）
 *
 * 全部数据源都是后端已有接口，无新增后端依赖。
 */

import type { PointBackoff } from '@/api/dataQuality'
import type { Packet } from '@/api/protocolTools/types'

export interface DeviceEvidence {
  /** 最后一次成功采集时间戳（毫秒，0 = 从未成功） */
  lastGoodTs: number
  /** 连续失败次数（该设备下点位失败数的最大值） */
  consecutiveFailures: number
  /** 最近一帧原始报文（hex，空串 = 无抓包记录） */
  lastFrameHex: string
  /** 最近一帧的方向（request/response） */
  lastFrameDirection: string
  /** 最近一帧时间戳（毫秒，0 = 无） */
  lastFrameTs: number
  /** 最近一帧是否失败（响应帧存在但解析失败 / 超时） */
  lastFrameFailed: boolean
  /** 最近一帧的失败原因（来自抓包解析结果，无则空串） */
  lastFrameError: string
}

/**
 * 从点位退避列表按设备聚合「最后成功时间 + 连续失败数」。
 *
 * 设备下有多个点位时：
 *   - 最后成功时间取各点位最大值（任一点位成功即认为设备在线）
 *   - 连续失败数取各点位最大值（任一点位持续失败即视为异常信号）
 */
export function aggregateBackoffByDevice(backoffs: PointBackoff[]): Record<string, DeviceEvidence> {
  const map: Record<string, DeviceEvidence> = {}
  for (const b of backoffs) {
    const devId = b.device_id
    if (!devId) continue
    let e = map[devId]
    if (!e) {
      e = {
        lastGoodTs: 0,
        consecutiveFailures: 0,
        lastFrameHex: '',
        lastFrameDirection: '',
        lastFrameTs: 0,
        lastFrameFailed: false,
        lastFrameError: '',
      }
      map[devId] = e
    }
    if (b.last_good_ts > e.lastGoodTs) e.lastGoodTs = b.last_good_ts
    if (b.consecutive_failures > e.consecutiveFailures) e.consecutiveFailures = b.consecutive_failures
  }
  return map
}

/**
 * 从抓包缓冲按设备聚合「最近原始帧」。
 *
 * 抓包缓冲是全局共享的环形缓冲，`deviceId` 为空的老网关报文无法归到具体设备，
 * 这类帧跳过（不猜归属，避免把别的设备的帧挂到本设备上误导判断）。
 *
 * 同一设备多帧时取时间最新的一帧；请求帧优先于响应帧——排障时先看「发了什么」。
 */
export function aggregateLastFrameByDevice(packets: Packet[]): Record<string, DeviceEvidence> {
  const map: Record<string, DeviceEvidence> = {}
  for (const p of packets) {
    if (!p.deviceId) continue
    const e = map[p.deviceId] ?? {
      lastGoodTs: 0,
      consecutiveFailures: 0,
      lastFrameHex: '',
      lastFrameDirection: '',
      lastFrameTs: 0,
      lastFrameFailed: false,
      lastFrameError: '',
    }
    // 同时间戳时保留先到的 request：请求帧是「设备收到了什么」，排障价值最高
    const isNewer = p.time > e.lastFrameTs
    const sameTimeButRequest = p.time === e.lastFrameTs && p.direction === 'request' && e.lastFrameDirection !== 'request'
    if (!isNewer && !sameTimeButRequest) {
      map[p.deviceId] = e
      continue
    }
    e.lastFrameHex = p.hex || ''
    e.lastFrameDirection = p.direction
    e.lastFrameTs = p.time
    e.lastFrameFailed = isFrameFailed(p)
    e.lastFrameError = extractFrameError(p)
    map[p.deviceId] = e
  }
  return map
}

/** 抓包帧是否代表失败：解析文本里带失败/异常标记 */
function isFrameFailed(p: Packet): boolean {
  const parsed = (p.parsed || '').toLowerCase()
  return parsed.includes('error') || parsed.includes('exception') || parsed.includes('crc') || parsed.includes('失败')
}

/** 从抓包帧的解析结果里提取失败原因文本 */
function extractFrameError(p: Packet): string {
  const parsed = p.parsed || ''
  if (!parsed) return ''
  // 解析结果常见形态：「错误:xxx」「error:xxx」「异常码: 0x02」
  const m = parsed.match(/(?:错误|异常码|error|exception)\s*[:：]\s*([^\n;；|]+)/i)
  if (m && m[1]) return m[1].trim()
  // 无显式错误前缀，但文本带失败标记（如「CRC 校验失败」）→ 截断首句作为原因
  if (isFrameFailed(p)) return parsed.slice(0, 80)
  return ''
}

/**
 * 合并两类证据源（退避统计 + 抓包帧）为设备维度证据表。
 * 两边都可能缺数据，缺的那边保持字段默认值（0 / 空串）。
 */
export function mergeDeviceEvidence(
  backoffEvidence: Record<string, DeviceEvidence>,
  frameEvidence: Record<string, DeviceEvidence>,
): Record<string, DeviceEvidence> {
  const merged: Record<string, DeviceEvidence> = {}
  const ids = new Set([...Object.keys(backoffEvidence), ...Object.keys(frameEvidence)])
  for (const id of ids) {
    const b = backoffEvidence[id]
    const f = frameEvidence[id]
    merged[id] = {
      lastGoodTs: b?.lastGoodTs ?? 0,
      consecutiveFailures: b?.consecutiveFailures ?? 0,
      lastFrameHex: f?.lastFrameHex ?? '',
      lastFrameDirection: f?.lastFrameDirection ?? '',
      lastFrameTs: f?.lastFrameTs ?? 0,
      lastFrameFailed: f?.lastFrameFailed ?? false,
      lastFrameError: f?.lastFrameError ?? '',
    }
  }
  return merged
}
