/**
 * 系统资源历史趋势（纯前端聚合）
 * 网关后端只提供 /api/metrics 实时值，前端轮询后把 CPU/内存/队列长度等
 * 指标写入环形缓冲，供 Dashboard 绘制趋势折线。
 */
import { scopedGet, scopedSet } from '@/utils/gatewayStorage'

const STORAGE_KEY = 'gw_metrics_history'
const MAX_POINTS = 120 // 2 小时 × 1 分钟采样

export interface MetricSample {
  ts: number
  cpu: number
  mem: number
  queue: number
  online: number
}

let cache: MetricSample[] = loadFromStorage()

function loadFromStorage(): MetricSample[] {
  try {
    const raw = scopedGet(STORAGE_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    // 过滤脏数据：缺字段或数值非有限数一律丢弃
    return parsed
      .filter((s: unknown): s is MetricSample =>
        !!s && typeof s === 'object' && typeof (s as MetricSample).ts === 'number' && Number.isFinite((s as MetricSample).ts)
      )
      .map((s: MetricSample) => ({
        ts: s.ts,
        cpu: Number.isFinite(s.cpu) ? s.cpu : 0,
        mem: Number.isFinite(s.mem) ? s.mem : 0,
        queue: Number.isFinite(s.queue) ? s.queue : 0,
        online: Number.isFinite(s.online) ? s.online : 0,
      }))
      .slice(-MAX_POINTS)
  } catch {
    return []
  }
}

function persist() {
  try {
    scopedSet(STORAGE_KEY, JSON.stringify(cache.slice(-MAX_POINTS)))
  } catch { /* ignore */ }
}

/** 追加一次采样（每分钟调用一次） */
export function pushMetrics(sample: Omit<MetricSample, 'ts'>) {
  cache.push({ ts: Date.now(), ...sample })
  if (cache.length > MAX_POINTS) cache = cache.slice(-MAX_POINTS)
  persist()
}

/** 获取历史采样序列 */
export function getMetricsHistory(): MetricSample[] {
  return cache
}

/** 清空历史（切换网关或重置时调用） */
export function clearMetricsHistory() {
  cache = []
  persist()
}

