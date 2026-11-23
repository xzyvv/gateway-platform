/**
 * 历史数据聚合统计
 * 把历史点位按时间窗口（小时/天）聚合出 max/min/avg/stddev
 *
 * DD-54：`/api/history` 的每个点可能已经是后端降采样后的桶（1 个点代表
 * `sample_count` 条原始样本，`value` 是该桶的加权均值，`min_value`/`max_value`
 * 是桶内真实极值）。二次聚合必须按 `sample_count` 加权，否则就是
 * 「平均的平均」，会让均值偏向采样稀疏的桶。
 */

export interface HistoryPoint {
  timestamp: number
  value: number
  /** 后端降采样桶内的原始样本数；原始点（未降采样）为 1 或 undefined */
  sample_count?: number
  /** 后端降采样桶内的真实最小值（原始点可缺省，回落到 value） */
  min_value?: number | null
  /** 后端降采样桶内的真实最大值（原始点可缺省，回落到 value） */
  max_value?: number | null
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

/** 该点代表的原始样本数：后端降采样桶取 sample_count，原始点按 1 计 */
function weightOf(p: HistoryPoint): number {
  return p.sample_count && p.sample_count > 0 ? p.sample_count : 1
}

/** 按粒度聚合；均值/标准差按样本数加权，stddev 用总体标准差 */
export function aggregateHistory(points: HistoryPoint[], granularity: AggregateGranularity): AggregatedPoint[] {
  if (granularity === 'raw' || points.length === 0) {
    // 原始粒度不再二次聚合：count 取该点代表的原始样本数，
    // 极值优先用后端桶内真实极值（无则回落点值）。
    return points.map((p) => ({
      bucket: p.timestamp,
      count: weightOf(p),
      min: p.min_value ?? p.value,
      max: p.max_value ?? p.value,
      avg: p.value,
      stddev: 0,
    }))
  }
  const step = granularity === 'hour' ? 3600_000 : 86_400_000
  const buckets = new Map<number, HistoryPoint[]>()
  for (const p of points) {
    const b = Math.floor(p.timestamp / step) * step
    const arr = buckets.get(b) || []
    arr.push(p)
    buckets.set(b, arr)
  }
  const result: AggregatedPoint[] = []
  for (const [bucket, values] of buckets) {
    // 加权统计：n = Σ 样本数，Σ(w·v) 与 Σ(w·v²) 用循环累加，
    // 避免 Math.min(...values) 在超大桶上栈溢出。
    let n = 0
    let sum = 0
    let sumSq = 0
    let min = Infinity
    let max = -Infinity
    for (const p of values) {
      const w = weightOf(p)
      n += w
      sum += p.value * w
      sumSq += p.value * p.value * w
      const lo = p.min_value ?? p.value
      const hi = p.max_value ?? p.value
      if (lo < min) min = lo
      if (hi > max) max = hi
    }
    const avg = sum / n
    // 浮点误差可能让方差略小于 0，钳到 0 再开方
    const variance = Math.max(0, sumSq / n - avg * avg)
    result.push({ bucket, count: n, min, max, avg, stddev: Math.sqrt(variance) })
  }
  return result.sort((a, b) => a.bucket - b.bucket)
}
