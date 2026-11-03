import { computed, ref } from 'vue'
import { defineStore } from 'pinia'
import { getHealthReport, type HealthReport } from '@/api/status'
import { extractErrorMessage } from '@/utils/errorHandler'
import type { TagType } from '@/utils/statusType'

/** 快照有效期：10s 内重复调用不会产生新请求 */
export const HEALTH_REPORT_TTL_MS = 10_000

/** 问题分级计数 */
export interface ProblemCounts {
  critical: number
  warning: number
  info: number
}

/** 综合状态：value 为后端原词，label/type 供各页面统一展示 */
export interface OverallHealthStatus {
  value: 'ok' | 'degraded' | 'critical' | 'unknown'
  label: string
  type: TagType
}

const STATUS_META: Record<Exclude<OverallHealthStatus['value'], 'unknown'>, { label: string; type: TagType }> = {
  ok: { label: '正常', type: 'success' },
  degraded: { label: '降级', type: 'warning' },
  critical: { label: '严重', type: 'danger' },
}

/**
 * 健康聚合报告 Store（B1：健康状态的单一真源；D3：由模块级单例改为 Pinia）
 *
 * 背景：运行总览、健康报告、自诊断、网关信息四个页面此前各自取数——
 * 总览按 metrics 阈值自算告警、健康报告直接打 `/api/health/report`、
 * 自诊断与网关信息读 `/health/ready` 的组件快照。同一台网关因此可能
 * 同时出现「总览说积压告警 / 报告说队列正常」这类互相矛盾的口径。
 *
 * 这里把 `/api/health/report` 收敛成 Store 快照：
 *   - 同一时刻只发一个请求（并发调用共享同一个 inflight Promise）
 *   - 10 秒内复用上一次结果，避免四个页面 + 轮询把同一接口打成密集请求
 *   - 失败保留上一次成功结果并置 error（现场断网时不该把已有健康信息清空）
 *   - 写操作后调用方先 invalidate() 再 refresh()，即可绕过 TTL 拿到新快照
 *
 * 不在此处扩张后端响应结构：报告里没有的字段（如磁盘余量、NTP）
 * 由各页面用自己的既有接口补齐，见 GatewayDiag / SystemOverview 的注释。
 */
export const useHealthReportStore = defineStore('healthReport', () => {
  const report = ref<HealthReport | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)

  /** 上一次「发起请求」的时间（成功或失败都算）：TTL 以尝试为准，避免失败时每轮轮询重试 */
  let lastAttemptAt = Number.NEGATIVE_INFINITY
  /** 进行中的请求：并发调用复用它，保证同一时刻只打一个请求 */
  let inflight: Promise<HealthReport | null> | null = null

  function fetchReport(): Promise<HealthReport | null> {
    loading.value = true
    inflight = getHealthReport()
      .then((next) => {
        report.value = next
        error.value = null
        return next
      })
      .catch((e: unknown) => {
        // 保留上一次成功结果：断网期间展示旧健康快照，比整页「暂无数据」更可用
        error.value = extractErrorMessage(e, '健康报告读取失败')
        return report.value
      })
      .finally(() => {
        lastAttemptAt = Date.now()
        loading.value = false
        inflight = null
      })
    return inflight
  }

  /**
   * 刷新健康报告。
   *
   * @param force 忽略 TTL 强制请求（手动刷新按钮用）；并发请求仍会被合并
   */
  function refresh(force = false): Promise<HealthReport | null> {
    if (inflight) return inflight
    if (!force && Date.now() - lastAttemptAt < HEALTH_REPORT_TTL_MS) return Promise.resolve(report.value)
    return fetchReport()
  }

  /** 标记快照过期：下次 refresh 会重新请求（写操作后由调用方使用） */
  function invalidate(): void {
    // 用 -Infinity 而非 0：与「首次请求」同一语义，不依赖当前时间戳的绝对值
    lastAttemptAt = Number.NEGATIVE_INFINITY
  }

  const problemCounts = computed<ProblemCounts>(() => {
    const counts: ProblemCounts = { critical: 0, warning: 0, info: 0 }
    for (const p of report.value?.problems ?? []) {
      if (p.level === 'critical' || p.level === 'warning' || p.level === 'info') counts[p.level] += 1
    }
    return counts
  })

  const overallStatus = computed<OverallHealthStatus>(() => {
    const status = report.value?.status
    if (!status) return { value: 'unknown', label: '暂无数据', type: 'info' }
    return { value: status, ...STATUS_META[status] }
  })

  return {
    report,
    loading,
    error,
    refresh,
    invalidate,
    problemCounts,
    overallStatus,
  }
})
