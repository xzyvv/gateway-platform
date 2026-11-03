import { ref, reactive, computed, onUnmounted } from 'vue'
import { useRouter } from 'vue-router'
import { getStatusSnapshot, getVersion, getMetrics, parsePrometheusText } from '@/api/status'
import { listDevices } from '@/api/devices'
import { getConfig } from '@/api/config'
import { listTemplates } from '@/api/templates'
import type { DeviceConfig, HealthComponent, RunStatus, VersionInfo } from '@/api/types'
import { loadConfigIssues, type ConfigIssue } from '@/utils/configHealth'
import { pushMetrics, getMetricsHistory, clearMetricsHistory } from '@/utils/metricsHistory'
import { usePolling } from '@/composables/core/usePolling'
import { useSharedTick } from '@/composables/core/useSharedTick'
import { useGatewayStore } from '@/stores/gateway'
import { useHealthReport } from '@/composables/system/useHealthReport'
import { useDashboardLayout } from './useDashboardLayout'
import { useHealthSignals, THRESHOLDS } from './useHealthSignals'
import { formatTimeOfDay } from '@/utils/datetime'
import { healthSeverity, healthRowsFromComponents } from '@/utils/healthStatus'

/**
 * Dashboard 页面数据层封装
 *
 * 把 Dashboard.vue 中的状态（status/version/healthRows/deviceList/trendData/summary/metrics）、
 * computed（healthSummary/overallHealthClass/...）、格式化函数、refresh 轮询、resize 处理
 * 集中到一个 composable，主组件只剩 template 与布局可见性判断。
 *
 * 同时返回 layout 状态与路由跳转方法，避免组件重复 import router/layout。
 */
export function useDashboardData() {
  const router = useRouter()
  // 健康快照由 Layout 顶栏的 5s 轮询写入 store，总览只读不重复请求
  const gateway = useGatewayStore()
  // B1：健康状态 / 组件明细 / 问题清单统一取自聚合报告（模块级快照，TTL 10s）
  const {
    report: healthReport,
    overallStatus,
    refresh: refreshHealthReport,
  } = useHealthReport()
  const { sections: layoutSections, toggle: layoutToggle, reset: layoutReset } = useDashboardLayout()
  const showLayout = ref(false)
  const isVisible = (key: string) => layoutSections.value.find((s) => s.key === key)?.visible !== false

  function goDevice(d: DeviceConfig) {
    router.push({ path: '/devices', query: { id: d.id } })
  }

  // ===== 全局汇总概览 =====
  const summary = reactive({
    onlineDevices: 0,
    totalDevices: 0,
    totalPoints: 0,
    buses: 0,
    alerts: 0,
    collectToday: 0,
    gatewayOnline: true,
  })

  function ringBg(online: number, total: number) {
    const ratio = total ? (online / total) * 100 : 0
    const color = ratio >= 80
      ? 'var(--color-success)'
      : ratio >= 50
        ? 'var(--color-warning)'
        : 'var(--color-danger)'
    return `conic-gradient(${color} ${ratio * 3.6}deg, rgba(255, 255, 255, 0.12) 0)`
  }

  function formatNum(n: number) {
    if (n >= 1e8) return (n / 1e8).toFixed(1) + '亿'
    if (n >= 1e4) return (n / 1e4).toFixed(1) + '万'
    return n.toLocaleString()
  }

  // ===== 趋势数据 =====
  const trendMax = 60
  interface TrendPoint {
    time: string
    cpu: number
    mem: number
    queue: number
    online: number
  }
  const trendData = ref<TrendPoint[]>([])

  // ===== status / version / metrics / health / devices =====
  // 字段与后端 RunStatus 完全对齐（ts-rs 自动生成），避免前端维护影子类型。
  // 默认值用 0 兜底；refresh 成功后 Object.assign 整体覆盖。
  const status = reactive<RunStatus>({
    online_devices: 0,
    queue_length: 0,
    cpu_usage: 0,
    mem_usage: 0,
    total_devices: 0,
    total_points: 0,
    bus_count: 0,
    today_alerts: 0,
    collect_today: 0,
    point_rows: [],
  })

  const version = reactive<VersionInfo>({
    version: '-',
    build_time: '-',
    git_hash: '-',
  })

  const healthRows = computed<HealthComponent[]>(() =>
    healthRowsFromComponents(healthReport.value?.components),
  )
  const deviceList = ref<DeviceConfig[]>([])
  // 配置体检结果：让「配置本身就错了」也能出现在总览异常面板里。
  // 低频拉取（5 分钟），配置只在人工改动时变化，无需跟随 3s 轮询。
  const configIssues = ref<ConfigIssue[]>([])
  const loading = ref(false)
  // 是否已完成过至少一次刷新（无论成败）。仅首次加载显示整页遮罩；
  // 3s 轮询属后台静默刷新，不应反复遮挡整页（见 refresh 首行的 P1 说明）。
  let hasLoadedOnce = false

  const metrics = reactive({
    read_total: 0,
    read_success: 0,
    read_error: 0,
    write_total: 0,
    write_success: 0,
    write_error: 0,
  })

  const readSuccessRate = computed(() => {
    if (!metrics.read_total) return 0
    return +(metrics.read_success / metrics.read_total * 100).toFixed(1)
  })

  const cpuGaugeVal = computed(() => +status.cpu_usage?.toFixed?.(1) || 0)
  const memGaugeVal = computed(() => +status.mem_usage?.toFixed?.(1) || 0)

  const healthSummary = computed(() => {
    let ok = 0, err = 0, warn = 0
    for (const r of healthRows.value) {
      const sev = healthSeverity(r.status)
      if (sev === 'ok') ok++
      else if (sev === 'error') err++
      else warn++
    }
    return { ok, err, warn, total: healthRows.value.length }
  })

  const overallHealthClass = computed<'ok' | 'warn' | 'err' | 'empty'>(() => {
    if (!healthSummary.value.total) return 'empty'
    if (healthSummary.value.err > 0) return 'err'
    if (healthSummary.value.warn > 0) return 'warn'
    return 'ok'
  })

  const overallHealthText = computed(() => {
    switch (overallHealthClass.value) {
      case 'ok': return '系统运行正常'
      case 'warn': return '部分组件降级或未就绪'
      case 'err': return `${healthSummary.value.err} 个组件异常`
      default: return '暂无组件数据'
    }
  })

  // 异常聚合入口的数据源：纯派生，不新增接口请求。
  // B1：健康类信号以 report.problems / report.status 为准（后端判定，前端不再各算一套），
  // configIssues 是低频拉取的配置体检结果，同样只做派生映射。
  const healthSignals = useHealthSignals(
    () => healthReport.value,
    () => summary.gatewayOnline,
    () => configIssues.value,
  )

  function formatNumber(n: number): string {
    if (n == null || !Number.isFinite(n)) return '0'
    return n.toLocaleString('zh-CN')
  }

  const cpuStatusClass = computed(() => {
    const v = typeof status.cpu_usage === 'number' ? status.cpu_usage : 0
    if (v >= 80) return 'metric-danger'
    if (v >= 60) return 'metric-warning'
    return ''
  })

  const memStatusClass = computed(() => {
    const v = typeof status.mem_usage === 'number' ? status.mem_usage : 0
    if (v >= 85) return 'metric-danger'
    if (v >= 70) return 'metric-warning'
    return ''
  })

  const queueStatusClass = computed(() => {
    const v = status.queue_length || 0
    // BUG-L：与 useHealthSignals / 后端 gateway_core::queue::limit 同一口径，
    // 不再各写一套（旧实现 100/1000 会把「正常历史存量」染红）
    if (v >= THRESHOLDS.queueError) return 'metric-danger'
    if (v >= THRESHOLDS.queueWarning) return 'metric-warning'
    return ''
  })

  // P1: 旧 pickMetric 5+ 别名猜测表已删除，由 numOf 替代（直接读后端 prometheus.rs
  // 导出的稳定 gateway_* 字段）。原函数无外部调用方（grep 已确认）。

  // ===== 最后更新时间 =====
  const lastUpdate = ref<Date | null>(null)
  const updateTick = ref(0)

  // Service Worker 在网关不可达时回退 /api/status 缓存，并注入 X-Data-Stale。
  // 这两项用于把「离线旧数据」与「实时新数据」在视觉上区分开。
  const dataStale = ref(false)
  const staleAgeSeconds = ref<number | null>(null)

  const dataStaleText = computed(() => {
    const age = staleAgeSeconds.value
    if (age == null) return '离线旧数据'
    if (age < 60) return `离线旧数据（${age} 秒前）`
    if (age < 3600) return `离线旧数据（${Math.floor(age / 60)} 分钟前）`
    return `离线旧数据（${Math.floor(age / 3600)} 小时前）`
  })

  const lastUpdateText = computed(() => {
    void updateTick.value
    if (!lastUpdate.value) return ''
    const s = Math.floor((Date.now() - lastUpdate.value.getTime()) / 1000)
    if (s < 5) return '刚刚'
    if (s < 60) return `${s} 秒前`
    return formatTimeOfDay(lastUpdate.value)
  })

  function fmtPct(v: unknown): string {
    if (v == null) return '-'
    const n = typeof v === 'number' ? v : Number(v)
    if (!Number.isFinite(n)) return '-'
    return `${n.toFixed(1)}%`
  }

  function pctWidth(v: unknown): string {
    if (v == null) return '0%'
    const n = typeof v === 'number' ? v : Number(v)
    if (!Number.isFinite(n)) return '0%'
    return `${Math.min(100, Math.max(0, n))}%`
  }

  // ===== refresh 防覆盖（seq 机制）=====
  let refreshSeq = 0

  /**
   * 高频刷新（3s）：取运行状态，并读取 Layout 已维护的健康快照。
   *
   * 健康快照（/health/ready）由 Layout 的 5s 轮询统一请求并写入 gateway store，
   * 这里只读 store，不再自己发请求——同一个接口被 3s + 5s 两条定时器各打一遍，
   * 约 20 次/分钟是纯重复流量。
   *
   * status 走 getStatusSnapshot：SW 离线回退时能拿到 X-Data-Stale 标记，
   * 用于把旧快照显示为「离线旧数据」，而不是当作刚刚更新。
   */
  async function refreshStatus() {
    const seq = ++refreshSeq
    // P1 修复：仅首次加载显示遮罩；3s 轮询属后台静默刷新，避免整页反复闪烁
    if (!hasLoadedOnce) loading.value = true
    // try/finally 保证 loading 一定复位：任一分支抛错若未捕获，
    // loading 会永久为 true，页面被遮罩卡死
    try {
      // 健康快照与 /api/status 同批刷新；composable 的 TTL 保证 10s 内不会重复请求
      const [sRes] = await Promise.allSettled([getStatusSnapshot(), refreshHealthReport()])
      if (seq !== refreshSeq) return
      if (sRes.status === 'fulfilled') {
        Object.assign(status, sRes.value.status)
        dataStale.value = sRes.value.stale
        staleAgeSeconds.value = sRes.value.ageSeconds
      }
      // 组件明细由 report.components 派生（healthRows 是 computed，无需回写）
      // summary 直接读 status 字段，避免 listDevices 失败时静默显示 0
      summary.onlineDevices = status.online_devices || 0
      summary.totalDevices = status.total_devices || 0
      summary.totalPoints = status.total_points || 0
      summary.buses = status.bus_count || 0
      summary.alerts = status.today_alerts || 0
      summary.collectToday = status.collect_today || 0
      // 网关状态只反映「可达性」：健康探针有没有响应。
      // 组件级故障（MQTT 未连上、队列积压）交给「N 个组件异常 / 未就绪」信号和
      // 组件健康表各自表达；混进这里会让「网关状态：离线」与「MQTT 客户端：不可用」
      // 把同一件事报两遍。健康探针失败才由 errors.health 判定失联。
      // 未完成首检时按"可达"处理（不误报离线）
      summary.gatewayOnline = !gateway.errors.health
      // 缓存回退的旧数据既不算"刚刚更新"，也不写趋势：否则离线期间会用同一份旧值
      // 反复打点，画出一条并不存在的平滑曲线。
      if (sRes.status === 'fulfilled' && !sRes.value.stale) {
        lastUpdate.value = new Date()
        const cpu = typeof status.cpu_usage === 'number' ? status.cpu_usage : 0
        const mem = typeof status.mem_usage === 'number' ? status.mem_usage : 0
        const queue = status.queue_length || 0
        const online = status.online_devices || 0
        pushMetrics({ cpu, mem, queue, online })
        const hist = getMetricsHistory().slice(-trendMax)
        trendData.value = hist.map((h) => ({
          time: formatTimeOfDay(h.ts),
          cpu: h.cpu, mem: h.mem, queue: h.queue, online: h.online,
        }))
      }
      // 网关不可达（Layout 健康轮询失败）时抛给 usePolling 进入退避，
      // 避免离线时仍每 3 秒打一轮注定失败的请求。
      if (sRes.status === 'rejected' && gateway.errors.health) throw sRes.reason
    } finally {
      // 无论成败都标记"已加载过"：后续轮询（含持续失败的场景）都不再遮挡整页
      hasLoadedOnce = true
      if (seq === refreshSeq) loading.value = false
    }
  }

  /** 低频刷新（30s）：版本、设备卡片列表、采集指标 */
  async function refreshAux() {
    const [vRes, devsRes, mRes] = await Promise.allSettled([
      getVersion(),
      listDevices(),
      getMetrics(),
    ])
    if (vRes.status === 'fulfilled') Object.assign(version, vRes.value)
    if (devsRes.status === 'fulfilled') deviceList.value = devsRes.value
    if (mRes.status === 'fulfilled') {
      // P1-2: 指标只认后端稳定导出的 gateway_* 字段名（见 core/metrics/prometheus.rs），
      // 不再猜测旧别名（read_total / read_count 等）。测试 mock 直传对象时兼容。
      // getMetrics() 声明返回 string；这里以 unknown 承载，兼容测试 mock 直传对象
      const raw: unknown = mRes.value
      const m = typeof raw === 'string' ? parsePrometheusText(raw) : ((raw || {}) as Record<string, unknown>)
      // 后端导出名（prometheus.rs）：gateway_collect_total / gateway_collect_failures
      //                            / gateway_point_write_total / gateway_point_write_error_total
      // 读 / 写两类指标在此简化：read=采集侧、write=控制下行（点位写入等）
      const readTotal = numOf(m, 'gateway_collect_total')
      const readFail = numOf(m, 'gateway_collect_failures')
      const readOk = Math.max(0, readTotal - readFail)
      metrics.read_total = readTotal
      metrics.read_success = readOk
      metrics.read_error = readFail
      // 下行写入口径：gateway_point_write_total / gateway_point_write_error_total
      // （后端在单点写、批量写、远程控制、GPIO DO 四处统一计数）
      const writeTotal = numOf(m, 'gateway_point_write_total')
      const writeError = numOf(m, 'gateway_point_write_error_total')
      metrics.write_total = writeTotal
      metrics.write_success = Math.max(0, writeTotal - writeError)
      metrics.write_error = writeError
    }
  }

  /**
   * 低频刷新（5 分钟）：配置体检。
   *
   * 配置里的硬错误（模板拼错、设备重名、重复地址）在运行指标上完全不可见——
   * 界面显示一切正常、实则 0 点位。把体检结果并入总览异常面板，用户不必
   * 自己想到「去体检页看一眼」。模板库失败时退化为跳过模板类检查。
   */
  async function refreshConfigIssues() {
    const [cfgRes, tplRes] = await Promise.allSettled([getConfig(), listTemplates()])
    if (cfgRes.status !== 'fulfilled') return
    const templateNames = new Set(
      tplRes.status === 'fulfilled' ? tplRes.value.map((t) => t.name) : [],
    )
    // A3：体检规则在后端，模板清单随校验请求下发。
    // 校验请求失败时保留上一次结果：体检是总览的附加信号，不该拖垮主数据刷新。
    try {
      configIssues.value = await loadConfigIssues(cfgRes.value, templateNames)
    } catch {
      /* 忽略：保留上一次体检结果 */
    }
  }

  /** 手动刷新：一次拉全量，失败交给拦截器提示，不影响页面 */
  async function refresh() {
    await Promise.allSettled([refreshStatus(), refreshAux()])
    // 手动刷新时顺带重算体检，避免用户改完配置回到总览仍看到旧问题
    await Promise.allSettled([refreshConfigIssues()])
  }

  /**
   * 安全读取 metric 值。
   * 入参用 `unknown` record 兼容 parsePrometheusText 返回的强类型与
   * 测试 mock 直传对象两种情况；内部做类型守卫后返回 number。
   * 字段缺失或非数字返回 0。
   */
  function numOf(metrics: Record<string, unknown>, key: string): number {
    const v = metrics[key]
    return typeof v === 'number' && Number.isFinite(v) ? v : 0
  }

  function clearTrend() {
    trendData.value = []
    clearMetricsHistory()
  }

  // ===== 生命周期 =====
  // "x 秒前"文案每秒走一次，只驱动文本，不触发请求。
  // 定时器走全站共享 tick（P0）：与顶栏时钟合并为一个 setInterval，切后台时停表。
  useSharedTick(() => { updateTick.value++ }, 1000)
  onUnmounted(() => {
    refreshSeq++
  })

  // 3s：状态（健康快照读 Layout 的 5s 轮询结果）；30s：版本/设备/指标。
  // 两者都在标签页隐藏时暂停、网关失联时指数退避（usePolling 统一策略）。
  usePolling(refreshStatus, 3000)
  usePolling(refreshAux, 30000)
  // 配置只在人工改动时变化，5 分钟一次足够；挂载时会立即执行一次（runNow）
  usePolling(refreshConfigIssues, 300000)

  return {
    // layout
    showLayout, layoutSections, layoutToggle, layoutReset, isVisible, goDevice,
    // summary
    summary, ringBg, formatNum,
    // trend
    trendData, trendMax, clearTrend,
    // status / version / metrics / health / devices
    status, version, healthRows, deviceList, loading, metrics, readSuccessRate,
    // 健康聚合状态（与健康报告 / 自诊断 / 网关信息同一份快照）
    overallStatus,
    // 异常聚合（AttentionPanel 数据源）
    healthSignals,
    cpuGaugeVal, memGaugeVal, healthSummary, overallHealthClass, overallHealthText,
    formatNumber, cpuStatusClass, memStatusClass, queueStatusClass,
    fmtPct, pctWidth,
    // lastUpdate
    lastUpdate, lastUpdateText,
    // 离线旧数据标记（SW 缓存回退）
    dataStale, dataStaleText,
    // refresh
    refresh,
  }
}
