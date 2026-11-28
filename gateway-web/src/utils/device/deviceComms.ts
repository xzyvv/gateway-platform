/**
 * 设备心跳 / 通信质量监控（前端聚合）
 * 后端没有专门的心跳接口，我们基于每次点位读写的成功/失败与耗时，
 * 按设备聚合出：最近通信时间、丢包率、平均/最大响应时延、连续失败次数
 */

import { reactive } from 'vue'

export interface DeviceCommsStat {
  deviceId: string
  lastSeen: number
  totalReads: number
  successReads: number
  failedReads: number
  consecutiveFailures: number
  latencies: number[] // 最近若干次耗时
  avgLatency: number
  maxLatency: number
}

// 用 reactive(Map) 而非裸 Map：调用方（如 useDevices 的 commsStats computed）需要
// 依赖它做响应式追踪。裸 Map 的读写不触发任何依赖收集，computed 首次求值后永久缓存，
// 导致"批量连通性检测"后通信质量列不刷新，必须离开页面再回来才更新。
//
// 有意保留为模块级单例：4 个跨页面消费方共享同一份统计，迁 Pinia 需同步改 4 个调用点
// 与 3 个 spec，收益仅为结构收敛；此处显式豁免，并已纳入 ESLint #17 的 utils 覆盖范围。
// eslint-disable-next-line no-restricted-syntax
const STATS = reactive(new Map<string, DeviceCommsStat>())
const MAX_LATENCIES = 20

export function recordRead(deviceId: string, ok: boolean, latencyMs: number) {
  let s = STATS.get(deviceId)
  if (!s) {
    s = {
      deviceId, lastSeen: 0, totalReads: 0, successReads: 0, failedReads: 0,
      consecutiveFailures: 0, latencies: [], avgLatency: 0, maxLatency: 0,
    }
    STATS.set(deviceId, s)
  }
  s.lastSeen = Date.now()
  s.totalReads++
  if (ok) {
    s.successReads++
    s.consecutiveFailures = 0
    s.latencies.push(latencyMs)
    if (s.latencies.length > MAX_LATENCIES) s.latencies.shift()
    s.avgLatency = s.latencies.reduce((a, b) => a + b, 0) / s.latencies.length
    s.maxLatency = Math.max(s.maxLatency, latencyMs)
  } else {
    s.failedReads++
    s.consecutiveFailures++
  }
}

export function getDeviceStats(): DeviceCommsStat[] {
  return Array.from(STATS.values())
}

/**
 * 清空统计（测试夹具）。
 *
 * 本模块持有模块级状态，单测之间需要复位；生产代码没有「清空全部设备通信统计」
 * 的场景（切网关时是整页重载），故仅供 spec 使用。
 */
export function clearDeviceStats() {
  STATS.clear()
}

/** 丢包率 0~1 */
export function packetLossRate(s: DeviceCommsStat): number {
  return s.totalReads === 0 ? 0 : s.failedReads / s.totalReads
}

/** 通信质量等级 */
export function commsQuality(s: DeviceCommsStat): 'good' | 'warn' | 'bad' | 'unknown' {
  if (s.totalReads === 0) return 'unknown'
  const loss = packetLossRate(s)
  if (s.consecutiveFailures >= 3) return 'bad'
  if (loss > 0.2 || s.avgLatency > 1000) return 'warn'
  return 'good'
}
