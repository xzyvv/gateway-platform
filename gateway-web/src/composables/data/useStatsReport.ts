/**
 * 采集统计报表的单一数据入口
 *
 * 背景：GET /api/diagnostics/stats 此前被「采集统计」「通信画像」「数据压缩」
 * 三处页面各自调用、各自维护 loading 与错误处理，同一个端点散落着多套取数逻辑。
 * 现在统一由本模块负责取数与状态，页面只负责渲染。
 *
 * 说明：本模块不做跨页面缓存——各页的刷新时机不同（报表可切周期，画像/压缩
 * 随配置变更重载），缓存会让「刷新」按钮拿到旧数据。
 */
import { ref, shallowRef } from 'vue'
import { getStatsReportView, type StatsReportView } from '@/api/diagnostics/view'
import type { StatsPeriod } from '@/api/diagnostics/raw'
import { useRequestGuard } from '@/composables/core/useRequestGuard'

export function useStatsReport(initialPeriod: StatsPeriod = 'day') {
  const report = shallowRef<StatsReportView | null>(null)
  const loading = ref(false)
  const error = ref<unknown>(null)
  const period = ref<StatsPeriod>(initialPeriod)
  // 请求序号守卫：快速切换统计周期时，旧周期的报表不得覆盖新周期
  const { next: nextRequest, isCurrent } = useRequestGuard()

  /**
   * 取一份报表（按当前 period）。
   *
   * 失败时抛出（由页面决定如何提示，与各页原有行为一致），
   * 同时把错误留在 error 里供页面按需展示。
   *
   * 竞态：调用方可能用返回值直接写自己的状态（如 CommProfile 把 report
   * 写进页面级表格）。若本次请求已过期，返回**当前已持有的** report 而非
   * 这次取到的旧数据，避免旧周期的报表污染调用方状态。
   */
  async function load(): Promise<StatsReportView> {
    const seq = nextRequest()
    loading.value = true
    error.value = null
    try {
      const r = await getStatsReportView(period.value)
      // 旧周期的响应不得写入
      if (!isCurrent(seq)) return report.value ?? r
      report.value = r
      return r
    } catch (e) {
      if (isCurrent(seq)) error.value = e
      throw e
    } finally {
      if (isCurrent(seq)) loading.value = false
    }
  }

  return { report, loading, error, period, load }
}
