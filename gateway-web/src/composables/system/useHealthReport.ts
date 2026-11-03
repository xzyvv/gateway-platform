/**
 * 健康聚合报告的统一入口（D3：共享快照已迁入 Pinia Store）。
 *
 * 实际状态与请求合并逻辑见 [`@/stores/healthReport`]：Store 是显式全局，
 * 避免了模块级 `ref()` 单例（隐式全局状态、测试间互相污染）。
 * 本文件保留既有函数名，避免各页面 / composable 的调用点无谓改动。
 */
import { storeToRefs } from 'pinia'
import {
  HEALTH_REPORT_TTL_MS,
  useHealthReportStore,
  type OverallHealthStatus,
  type ProblemCounts,
} from '@/stores/healthReport'

export { HEALTH_REPORT_TTL_MS }
export type { OverallHealthStatus, ProblemCounts }

/**
 * 刷新健康报告。
 *
 * @param force 忽略 TTL 强制请求（手动刷新按钮用）；并发请求仍会被合并
 */
export function refreshHealthReport(force = false) {
  return useHealthReportStore().refresh(force)
}

/** 标记快照过期：下次 refresh 会重新请求（写操作后由调用方使用） */
export function invalidateHealthReport(): void {
  useHealthReportStore().invalidate()
}

/**
 * 组件健康表 / 异常面板的统一入口。
 * 返回的 report 与 error 是所有页面共享的同一份快照，不会各自再发请求。
 */
export function useHealthReport() {
  const store = useHealthReportStore()
  const { report, loading, error, problemCounts, overallStatus } = storeToRefs(store)
  return {
    report,
    loading,
    error,
    refresh: store.refresh,
    invalidate: store.invalidate,
    problemCounts,
    overallStatus,
  }
}
