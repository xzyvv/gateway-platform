/**
 * 总线通信统计（前端聚合）
 * 基于每次点位读写所属总线，聚合各总线的请求数、成功率、错误分布
 */

export interface BusStat {
  bus: string
  total: number
  success: number
  failed: number
  errors: Record<string, number> // 错误类型 -> 次数
  avgLatency: number
  latencies: number[]
}

const STATS = new Map<string, BusStat>()
const MAX_LATENCIES = 50

export function recordBusOp(bus: string, ok: boolean, latencyMs: number, errorType?: string) {
  let s = STATS.get(bus)
  if (!s) {
    s = { bus, total: 0, success: 0, failed: 0, errors: {}, avgLatency: 0, latencies: [] }
    STATS.set(bus, s)
  }
  s.total++
  if (ok) {
    s.success++
    s.latencies.push(latencyMs)
    if (s.latencies.length > MAX_LATENCIES) s.latencies.shift()
    s.avgLatency = s.latencies.reduce((a, b) => a + b, 0) / s.latencies.length
  } else {
    s.failed++
    const key = errorType || 'unknown'
    s.errors[key] = (s.errors[key] || 0) + 1
  }
}

export function getBusStats(): BusStat[] {
  return Array.from(STATS.values())
}

export function successRate(s: BusStat): number {
  return s.total === 0 ? 0 : s.success / s.total
}

export function clearBusStats() {
  STATS.clear()
}
