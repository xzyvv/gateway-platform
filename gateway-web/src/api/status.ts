import { request, requestWithMeta } from './index'
import type { RunStatus, VersionInfo, HealthReady } from './types'
import { parsePrometheusLines } from '../utils/prometheus'

/** 获取运行状态 */
export function getStatus(): Promise<RunStatus> {
  return request<RunStatus>({ url: '/api/status', method: 'GET' })
}

/** /api/status 快照：除数据外带上「是否来自 Service Worker 离线缓存」的元信息 */
export interface StatusSnapshot {
  status: RunStatus
  /** true 表示网关不可达、返回的是缓存里的旧快照 */
  stale: boolean
  /** 旧快照的缓存年龄（秒）；非陈旧数据为 null */
  ageSeconds: number | null
}

/**
 * 获取运行状态 + 离线陈旧标记。
 *
 * 普通页面用 getStatus 即可（失败即失败）；总览页需要区分「刚到的新数据」与
 * 「离线回退的旧数据」，避免把旧快照当成实时状态展示。
 */
export async function getStatusSnapshot(): Promise<StatusSnapshot> {
  const { data, meta } = await requestWithMeta<RunStatus>({ url: '/api/status', method: 'GET' })
  return { status: data, stale: meta.stale, ageSeconds: meta.staleAgeSeconds }
}

/** 获取版本信息 */
export function getVersion(): Promise<VersionInfo> {
  return request<VersionInfo>({ url: '/api/version', method: 'GET' })
}

/**
 * 获取指标（后端 /api/metrics 返回 Prometheus 文本格式，非 JSON）。
 * 需要键值对形式的调用方配合 parsePrometheusText 使用；
 * 需要完整样本（含标签）的调用方自行解析，如 MetricsDashboard。
 */
export function getMetrics(): Promise<string> {
  return request<string>({ url: '/api/metrics', method: 'GET' })
}

/**
 * 解析 Prometheus 文本为 { 指标名: 值 }；带标签的样本按指标名聚合求和，非法行跳过。
 * 行分词由 utils/prometheus 统一提供（与 MetricsDashboard 共享），本函数只做扁平聚合。
 */
export function parsePrometheusText(text: string): Record<string, number> {
  const out: Record<string, number> = {}
  for (const line of parsePrometheusLines(text)) {
    if (line.kind !== 'sample') continue
    out[line.sample.name] = (out[line.sample.name] || 0) + line.sample.value
  }
  return out
}

/**
 * 健康检查（就绪探针）
 *
 * `/health/ready` 是 Kubernetes 风格的就绪探针：网关任一组件降级时返回
 * **HTTP 503**，但响应体仍携带完整健康快照（status/components/...）。
 * 默认 axios 会把 503 判为请求失败、被全局拦截器转成 error 并弹出「请求失败(503)」，
 * 导致调用方拿不到本该用于展示的健康数据。故此处显式放行 503，让调用方按
 * `status` 字段自行判断健康：
 * - 诊断报告的健康检查章节不再恒为「暂无数据」
 * - 顶栏健康徽章 / 仪表盘在降级时显示「异常」，而非停留在「检测中...」或误判为在线
 */
export function healthReady(): Promise<HealthReady> {
  return request<HealthReady>({
    url: '/health/ready',
    method: 'GET',
    validateStatus: (s) => (s >= 200 && s < 300) || s === 503,
  })
}

/** D10 磁盘采样点 */
export interface DiskSample { ts: number; free_pct: number }
export interface DiskHistory { samples: DiskSample[]; count: number }
export function getDiskHistory(): Promise<DiskHistory> {
  return request({ url: '/api/health/disk/history', method: 'GET' })
}

// ===== 健康聚合报告（GET /api/health/report，P1-3 接入 UI） =====

/** 一条可直接照做的处置项（level 决定分级顺序：critical > warning > info） */
export interface HealthProblem {
  level: 'critical' | 'warning' | 'info'
  /** 稳定机器码，如 QUEUE_BACKPRESSURE / BUS_ALL_OFFLINE / MQTT_DISCONNECTED */
  code: string
  message: string
  hint: string
}

/** 单总线近 24h 健康汇总；窗口内无采集时各比率字段为 null */
export interface HealthBusRow {
  bus: string
  protocol: string
  total_devices: number
  online_devices: number
  total_reads: number
  success_rate: number | null
  avg_latency_ms: number | null
  timeout_rate: number | null
  last_error_code: string | null
  reconnect_count: number
}

/** 一屏看全网关健康：组件状态 + 汇总 + 逐总线明细 + 分级问题清单 */
export interface HealthReport {
  generated_at_ms: number
  status: 'ok' | 'degraded' | 'critical'
  components: Record<string, string>
  summary: {
    total_devices: number
    /** 真正有点位落在总线上的设备数（配置了但没跑采集的不计） */
    collecting_devices: number
    online_devices: number
    bus_count: number
    queue_len: number
    total_reads: number
    success_rate: number | null
  }
  buses: HealthBusRow[]
  problems: HealthProblem[]
}

/**
 * 健康聚合报告。
 *
 * 与 `/api/status` 同量级开销，但把「队列背压 / 总线全离线 / MQTT 断链」这类
 * 需要处置的问题直接以文本给出，不必前端自己拼三个接口再判断。
 */
export function getHealthReport(): Promise<HealthReport> {
  return request<HealthReport>({ url: '/api/health/report', method: 'GET' })
}
