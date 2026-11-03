import { ref, reactive, computed, watch, nextTick, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { request } from '@/api'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import echarts, { initChart, disposeChart } from '@/utils/echarts-extra'
import { aggregateHistory, type AggregateGranularity } from '@/utils/aggregateHistory'
import { buildHistoryChartOption } from '@/utils/historyChart'
import { buildHistoryCsv, downloadCsvText, triggerDownload } from '@/utils/exportCsv'
import { qualityMap, qualityTagType } from '@/utils/qualityLabels'
import { formatDateTime } from '@/utils/datetime'

/**
 * `/api/history` 的单个样本（P2-1）。
 *
 * 原始行与聚合桶共用同一形状：
 * - `sample_count === 1` 且 `min_value === null` → 原始采样点
 * - `sample_count >= 1` 且 `min_value/max_value` 有值 → 降采样时间桶
 *   （`value` 为桶内平均值，`timestamp` 为桶起点）
 */
export interface HistorySample {
  timestamp: number
  value: number
  quality: string
  /** 桶内最小值（原始行为 null） */
  min_value?: number | null
  /** 桶内最大值（原始行为 null） */
  max_value?: number | null
  /** 归入本样本的原始样本数（原始行恒为 1） */
  sample_count?: number
  raw_value?: number | null
  raw_type?: string | null
}

/**
 * `/api/history` 返回体（P2-1③：带分页/截断元信息）。
 *
 * 查询始终保留窗口内最新的 `limit` 条，因此 `truncated === has_more`。
 */
export interface HistoryResponse {
  items: HistorySample[]
  /** 窗口内匹配的样本总数（原始行数 / 聚合桶数） */
  total: number
  /** 实际返回条数 */
  returned: number
  truncated: boolean
  has_more: boolean
  /** 实际粒度："raw" / "second:N" */
  granularity: string
}

/**
 * `/api/history` 的唯一取数入口（W1）。
 *
 * 全站只允许这里出现 `/api/history` 的 url 字面量：历史数据页（useHistoryQuery）与
 * 区间对比页（DataCompare）都走这个函数，避免同一端点出现多套查询写法
 * （此前 DataCompare 在视图内手拼 query 串，与 composable 版的 params 写法并存）。
 *
 * `downsample` 不传则不带该参数（区间对比页从不降采样，传 0 会改变后端分支语义）。
 */
export function fetchHistorySamples(opts: {
  pointId: string
  /** 毫秒时间戳 */
  start: number
  /** 毫秒时间戳 */
  end: number
  /** 0 = 不降采样 */
  downsample?: number
}): Promise<HistoryResponse> {
  const params: Record<string, string | number> = {
    point_id: opts.pointId,
    start: opts.start,
    end: opts.end,
  }
  if (opts.downsample !== undefined) params.downsample = opts.downsample
  return request<HistoryResponse>({ url: '/api/history', method: 'GET', params })
}

/**
 * History 页面数据层封装
 *
 * 把 History.vue 中的状态（query/history/multiHistory/granularity/loading/queried/chartRef）、
 * computed（displayHistory/aggregated/stats/activeQuickKey）、格式化函数、查询 / 渲染图表 /
 * 导出 CSV / 卸载清理集中到一个 composable，主组件只剩 template。
 *
 * 图表 option 构建在 [../utils/historyChart]，CSV 构建/下载在 [@/utils/exportCsv]，
 * LTTB 降采样在 [../utils/downsample]。
 */
export function useHistoryQuery() {
  const { showCopyable } = useErrorDisplay()
  // f8: 后端 history 接口的 start/end 与返回的 timestamp 均为「毫秒」时间戳
  // （见 gateway-core DataPoint::now() = timestamp_millis()，SQLite 侧裸比较）。
  // 因此 el-date-picker 使用 value-format="x"（毫秒），初始值亦为毫秒。
  const query = reactive({
    point_id: '',
    start: Date.now() - 3_600_000,
    end: Date.now(),
    downsample: 0,
  })

  const quickRanges = [
    { key: 3_600_000, label: '近 1 小时' },
    { key: 21_600_000, label: '近 6 小时' },
    { key: 86_400_000, label: '近 24 小时' },
    { key: 604_800_000, label: '近 7 天' },
    { key: 2_592_000_000, label: '近 30 天' },
  ]

  const activeQuickKey = computed(() => {
    const span = Number(query.end) - Number(query.start)
    for (const r of quickRanges) {
      // 容差 60s（毫秒单位下为 60000），抵消点击与查询之间的时间漂移
      if (Math.abs(span - r.key) < 60_000) return r.key
    }
    return null
  })

  type HistoryPoint = { timestamp: number; value: number; quality: string; rowKey?: string }
  const history = ref<HistoryPoint[]>([])
  const granularity = ref<AggregateGranularity>('raw')

  const aggregated = computed(() => aggregateHistory(history.value, granularity.value))

  const activePointId = ref('')

  const displayHistory = computed(() => {
    if (granularity.value === 'raw') return history.value
    const pid = activePointId.value || 'all'
    return aggregated.value.map((a) => ({
      timestamp: a.bucket,
      value: a.avg,
      quality: 'Aggregated',
      rowKey: `agg_${pid}_${a.bucket}`,
    }))
  })

  const multiHistory = ref<{ pointId: string; data: HistoryPoint[] }[]>([])

  /**
   * 最近一次查询的截断信息（P2-1③）。
   *
   * 后端 `limit` 只保留窗口内最新的一批数据；为 null 表示没有截断。
   * 多点位查询时取"截断最严重"（total 最大）的点位用于提示文案。
   */
  const truncation = ref<{ pointId: string; total: number; returned: number } | null>(null)

  const loading = ref(false)
  const queried = ref(false)
  const chartRef = ref<HTMLElement>()
  let chart: echarts.ECharts | null = null
  let unmounted = false
  let querySeq = 0

  function formatTime(ts: number): string {
    if (ts == null || !Number.isFinite(ts) || ts <= 0) return '-'
    // f8: ts 已是毫秒时间戳，直接构造 Date（原实现 ×1000 会导致时间轴显示到 1970 年之后）
    const d = new Date(ts)
    if (isNaN(d.getTime())) return '-'
    return formatDateTime(d)
  }

  function setQuickRange(ms: number) {
    const now = Date.now()
    query.start = now - ms
    query.end = now
  }

  const stats = computed(() => {
    const values = displayHistory.value
      .map((h) => h.value)
      .filter((v): v is number => typeof v === 'number' && Number.isFinite(v))
    if (values.length === 0) return { min: '-', max: '-', avg: '-' }
    let min = values[0]!
    let max = values[0]!
    let sum = 0
    for (let i = 0; i < values.length; i++) {
      const v = values[i]!
      if (v < min) min = v
      if (v > max) max = v
      sum += v
    }
    const avg = sum / values.length
    const fmt = (v: number) => Number.isInteger(v) ? v.toString() : v.toFixed(2)
    return { min: fmt(min), max: fmt(max), avg: fmt(avg) }
  })

  function handleResize() {
    chart?.resize()
  }

  watch(activePointId, (id) => {
    const found = multiHistory.value.find((m) => m.pointId === id)
    history.value = found?.data || []
  })

  async function queryHistory() {
    if (!query.point_id) {
      ElMessage.warning('请输入点位 ID')
      return
    }
    const start = Number(query.start)
    const end = Number(query.end)
    if (!Number.isFinite(start) || !Number.isFinite(end)) {
      ElMessage.warning('请输入有效的时间戳')
      return
    }
    if (start >= end) {
      ElMessage.warning('开始时间必须早于结束时间')
      return
    }
    const pointIds = query.point_id.split(',').map((s) => s.trim()).filter(Boolean)
    if (pointIds.length === 0) {
      ElMessage.warning('请输入有效的点位 ID')
      return
    }
    const seq = ++querySeq
    loading.value = true
    truncation.value = null
    try {
      const CONCURRENCY = 4
      const results: {
        pointId: string
        data: HistorySample[]
        total: number
        truncated: boolean
      }[] = []
      const queue = [...pointIds]
      async function worker() {
        while (!unmounted && seq === querySeq) {
          const pid = queue.shift()
          if (!pid) break
          try {
            // P2-1：返回体由「裸数组」改为 { items, total, returned, truncated, ... }
            const res = await fetchHistorySamples({
              pointId: pid,
              start,
              end,
              downsample: query.downsample,
            })
            results.push({
              pointId: pid,
              data: res.items ?? [],
              total: res.total ?? (res.items?.length ?? 0),
              truncated: Boolean(res.truncated),
            })
          } catch {
            results.push({ pointId: pid, data: [], total: 0, truncated: false })
          }
        }
      }
      await Promise.all(
        Array.from({ length: Math.min(CONCURRENCY, pointIds.length) }, () => worker()),
      )
      if (unmounted || seq !== querySeq) return
      const stamped = results.map((r) => ({
        pointId: r.pointId,
        data: r.data.map((h) => ({ ...h, rowKey: `${r.pointId}:${h.timestamp}` })),
      }))
      multiHistory.value = stamped
      activePointId.value = stamped[0]?.pointId || ''
      history.value = stamped[0]?.data || []
      // 截断提示：取 total 最大的点位，文案给"窗口内共 M 条，仅显示最近 N 条"
      const worst = results
        .filter((r) => r.truncated)
        .sort((a, b) => b.total - a.total)[0]
      truncation.value = worst
        ? { pointId: worst.pointId, total: worst.total, returned: worst.data.length }
        : null
      queried.value = true
      await nextTick()
      renderChart()
    } catch {
      /* 忽略 */
    } finally {
      if (!unmounted) loading.value = false
    }
  }

  function renderChart() {
    if (!chartRef.value) return
    if (!chart) {
      chart = initChart(chartRef.value)
      window.addEventListener('resize', handleResize)
    }
    const xData = (multiHistory.value[0]?.data || []).map((h) => formatTime(h.timestamp))
    chart.setOption(buildHistoryChartOption(multiHistory.value, xData), true)
  }

  /**
   * 拉取后端导出的 CSV Blob（/api/history/export）。
   * 失败/空数据时给出可见提示并返回 null，供 CSV / JSON 两种导出复用。
   */
  async function fetchExportBlob(): Promise<Blob | null> {
    if (!query.point_id) {
      ElMessage.warning('请输入点位 ID')
      return null
    }
    try {
      // 走 request() 而非 instance.get()：让 401/429/4021 走统一拦截器。
      // 注意：request() 已解包为 response.data，故改用 blob.type 判定内容类型。
      const blob = await request<Blob>({
        url: '/api/history/export',
        method: 'GET',
        params: {
          point_id: query.point_id,
          start: Number(query.start),
          end: Number(query.end),
          downsample: query.downsample,
        },
        responseType: 'blob',
      })
      const ct = String(blob.type || '')
      if (ct.includes('application/json')) {
        const text = await blob.text()
        let msg = '导出失败'
        try {
          const obj = JSON.parse(text)
          msg = obj.error || obj.message || msg
        } catch { /* keep default */ }
        // 导出失败需要可复制：Toast 无法选中文本，现场只能截图。这里把后端的原始
        // 响应体与查询上下文一起放进可复制弹窗，便于直接回传研发。
        void showCopyable(new Error(msg), '导出失败', `${exportContext()}\n\n后端响应：${text}`)
        return null
      }
      if (blob.size === 0) {
        ElMessage.warning('没有可导出的数据')
        return null
      }
      return blob
    } catch (e) {
      // Bug 182: 导出失败不再静默回退为「客户端已有数据」CSV。后端导出可能因鉴权/
      // 网络/超时失败，静默导出本地缓存会让用户误以为拿到了完整区间数据（实际只是
      // 当前已加载的部分），并掩盖真实故障。改为明确提示；如需导出本地已加载数据，
      // 用户可显式使用「导出当前数据」入口（exportClientCsv）。
      if (displayHistory.value.length === 0) {
        ElMessage.warning('暂无可导出的数据，请先查询')
      } else {
        void showCopyable(
          e,
          '导出失败',
          `${exportContext()}\n提示：可先用「导出当前数据」导出已加载的部分。`,
        )
      }
      return null
    }
  }

  async function exportCsv() {
    const blob = await fetchExportBlob()
    if (!blob) return
    triggerDownload(blob, `${query.point_id}-${Date.now()}.csv`)
    ElMessage.success('导出成功')
  }

  /**
   * 导出 JSON：与 CSV 共用同一份后端数据（同一区间/降采样），只在前端做格式转换，
   * 产出的是真实 JSON 文件（F-17：不再用改 MIME 后缀冒充其它格式）。
   */
  async function exportJson() {
    const blob = await fetchExportBlob()
    if (!blob) return
    const text = await blob.text()
    const lines = text.split('\n').filter((l) => l.trim())
    const header = (lines.shift() ?? '').split(',')
    const rows = lines.map((l) => {
      const cells = l.split(',')
      return Object.fromEntries(header.map((h, i) => [h, cells[i] ?? '']))
    })
    triggerDownload(
      new Blob([JSON.stringify(rows, null, 2)], { type: 'application/json' }),
      `${query.point_id}-${Date.now()}.json`,
    )
    ElMessage.success(`导出成功（${rows.length} 行）`)
  }

  /** 导出请求的上下文（复制到错误弹窗，便于定位是哪个点位/区间失败） */
  function exportContext(): string {
    return [
      `点位：${query.point_id}`,
      `时间段：${formatDateTime(query.start)} ~ ${formatDateTime(query.end)}`,
      `毫秒时间戳：${query.start} ~ ${query.end}`,
      `降采样：${query.downsample || '关闭'}`,
    ].join('\n')
  }

  function exportClientCsv() {
    const rows = displayHistory.value.map((r: { timestamp: number; point_id?: string; value?: number; quality?: string }) => [
      formatDateTime(r.timestamp),
      r.point_id || query.point_id,
      r.value ?? '',
      r.quality ?? 'good',
    ])
    const csv = buildHistoryCsv(rows)
    downloadCsvText(csv, `history-${query.point_id}-${Date.now()}.csv`)
    ElMessage.success(`已导出 ${rows.length} 条数据`)
  }

  onUnmounted(() => {
    unmounted = true
    querySeq++
    window.removeEventListener('resize', handleResize)
    // disposeChart 同步从 liveCharts 注册表摘除，防止已销毁实例被主题刷新持有（长驻内存）
    disposeChart(chart)
    chart = null
  })

  return {
    // state
    query, quickRanges, activeQuickKey,
    history, granularity, activePointId, multiHistory,
    truncation,
    displayHistory, aggregated, stats,
    loading, queried, chartRef,
    // quality labels
    qualityMap, qualityTagType,
    // actions
    setQuickRange, queryHistory, renderChart, exportCsv, exportJson, exportClientCsv, formatTime,
  }
}