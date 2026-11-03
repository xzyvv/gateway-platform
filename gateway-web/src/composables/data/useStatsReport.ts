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
import { getStatsReportView, type StatsPeriod, type StatsReportView } from '@/api/diagnostics'

export function useStatsReport(initialPeriod: StatsPeriod = 'day') {
  const report = shallowRef<StatsReportView | null>(null)
  const loading = ref(false)
  const error = ref<unknown>(null)
  const period = ref<StatsPeriod>(initialPeriod)

  /**
   * 取一份报表（按当前 period）。
   *
   * 失败时抛出（由页面决定如何提示，与各页原有行为一致），
   * 同时把错误留在 error 里供页面按需展示。
   */
  async function load(): Promise<StatsReportView> {
    loading.value = true
    error.value = null
    try {
      const r = await getStatsReportView(period.value)
      report.value = r
      return r
    } catch (e) {
      error.value = e
      throw e
    } finally {
      loading.value = false
    }
  }

  return { report, loading, error, period, load }
}
