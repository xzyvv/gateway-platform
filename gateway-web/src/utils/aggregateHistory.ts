/**
 * 历史数据聚合统计
 * 把原始历史点位按时间窗口（小时/天）聚合出 max/min/avg/stddev
 */

export interface HistoryPoint {
  timestamp: number
  value: number
}

export interface AggregatedPoint {
  bucket: number // 桶起始时间戳
  count: number
  min: number
  max: number
  avg: number
  stddev: number
}

export type AggregateGranularity = 'raw' | 'hour' | 'day'

/** 按粒度聚合，stddev 用总体标准差 */
export function aggregateHistory(points: HistoryPoint[], granularity: AggregateGranularity): AggregatedPoint[] {
  if (granularity === 'raw' || points.length === 0) {
    return points.map((p) => ({
      bucket: p.timestamp,
      count: 1,
      min: p.value,
      max: p.value,
      avg: p.value,
      stddev: 0,
    }))
  }
  const step = granularity === 'hour' ? 3600_000 : 86_400_000
  const buckets = new Map<number, number[]>()
  for (const p of points) {
    const b = Math.floor(p.timestamp / step) * step
    const arr = buckets.get(b) || []
    arr.push(p.value)
    buckets.set(b, arr)
  }
  const result: AggregatedPoint[] = []
  for (const [bucket, values] of buckets) {
    const n = values.length
    // 循环求极值：Math.min(...values) 在单桶超过约 10 万元素时会抛栈溢出
    let min = Infinity
    let max = -Infinity
    let sum = 0
    for (const v of values) {
      if (v < min) min = v
      if (v > max) max = v
      sum += v
    }
    const avg = sum / n
    const variance = values.reduce((s, v) => s + (v - avg) ** 2, 0) / n
    result.push({ bucket, count: n, min, max, avg, stddev: Math.sqrt(variance) })
  }
  return result.sort((a, b) => a.bucket - b.bucket)
}
