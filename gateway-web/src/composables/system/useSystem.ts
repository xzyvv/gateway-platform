import { ref, reactive, computed, onMounted, onUnmounted } from 'vue'
import { ElMessage } from 'element-plus'
import { confirmAction } from '@/utils/confirm'
import { useClock } from '@/composables/system/useClock'
import { usePolling } from '@/composables/core/usePolling'
import { getStatus, getVersion, getMetrics, parsePrometheusText } from '@/api/status'
import { getNtpStatus, ntpSync, type NtpStatus } from '@/api/network'
import type { VersionInfo, RunStatus } from '@/api/types'
import { getConfig } from '@/api/config'
import { useWebhook } from '@/composables/uplink/useWebhook'
import { listAuditLogs, type AuditLogEntry } from '@/api/auditLog'
import { useSessions } from './useSessions'
import { useDiagnosticExport } from './useDiagnosticExport'
import { useHealthReport } from './useHealthReport'
import { healthRowsFromComponents } from '@/utils/healthStatus'
import { buildBackup, downloadBackup, parseBackupFile, type SystemBackup } from '@/utils/systemBackup'
import { extractErrorMessage } from '@/utils/errorHandler'
import { formatDateTime } from '@/utils/datetime'
import { statusTagType, type TagType } from '@/utils/statusType'

/**
 * System 页面数据层封装
 *
 * 把 System.vue 中的所有状态（status/version/ntp/metrics/loading/auditEntries/
 * sessionList/webhookUrl/diagLoading/backupLoading/lastBackup）、computed（metricsHtml/
 * nowTime/metricsText）、格式化函数、所有按钮操作（refresh/syncNtp/saveWebhook/testWebhook/
 * genDiagnostic/doBackup/onRestoreFile/revokeSession/retryAllNotify）集中到一个
 * composable，主组件只剩 template。
 */
export function useSystem() {
  // ===== 审计日志 =====
  // B3：前端本地操作审计已下线，这里只读后端 /api/audit/logs（唯一审计来源）。
  // 后端没有删除端点，所以不提供「清空」，只有刷新。
  const auditEntries = ref<AuditLogEntry[]>([])
  async function refreshAudit() {
    try {
      auditEntries.value = await listAuditLogs({ limit: 200 })
    } catch {
      // 后端不可达时保留上一次结果，页面其它区块照常展示
    }
  }

  // ===== Webhook =====
  const webhook = useWebhook()
  const webhookUrl = webhook.webhookUrl
  function saveWebhook() {
    webhook.setUrl(webhookUrl.value)
    ElMessage.success(webhookUrl.value ? 'Webhook 已启用' : 'Webhook 已禁用')
  }
  async function testWebhook() {
    if (!webhookUrl.value) return
    try {
      await webhook.notify('test', '这是一条来自工业网关的测试通知')
      ElMessage.success('测试通知已发送')
    } catch {
      ElMessage.warning('发送失败，请检查 URL')
    }
  }
  /** 重发本机通知重试队列（非设备端死信） */
  async function retryAllNotify() {
    const r = await webhook.retryAllNotify()
    ElMessage.success(`重发完成：成功 ${r.ok}，失败 ${r.fail}`)
  }

  // ===== 会话管理 =====
  const sessions = useSessions()
  const sessionList = computed(() => sessions.list())
  async function revokeSession(id: string) {
    if (await confirmAction({ message: '确认强制下线该会话？', title: '强制下线' })) {
      sessions.revoke(id)
      ElMessage.success('已下线')
    }
  }

  // ===== 系统指标状态 =====
  // 仅取 System 页需要的四个字段（RunStatus 还含 devices 等本页不用的字段）
  type SystemStatus = Pick<RunStatus, 'online_devices' | 'queue_length' | 'cpu_usage' | 'mem_usage'>
  const status = reactive<SystemStatus>({
    online_devices: 0,
    queue_length: 0,
    cpu_usage: 0,
    mem_usage: 0,
  })
  const version = reactive<VersionInfo>({ version: '-', build_time: '-', git_hash: '-' })
  const ntp = reactive<NtpStatus>({ status: 'unknown' })
  const metrics = ref<Record<string, unknown>>({})
  const loading = ref(false)

  // ===== 健康聚合（B1 单一真源）=====
  // 网关信息页的组件健康区块不再读 /health/ready 的另一份快照，而是复用聚合报告；
  // report 是模块级共享快照，5s 轮询下 TTL 10s 内不会重复请求。
  // 报告不含 NTP 与磁盘余量，这两块仍走各自的既有接口（见页面内注释）。
  const health = useHealthReport()
  const healthRows = computed(() => healthRowsFromComponents(health.report.value?.components))
  const healthProblems = computed(() => health.report.value?.problems ?? [])

  // NTP 状态映射
  const ntpStatusMap: Record<string, string> = {
    sync: '已同步',
    not_sync: '未同步',
    unknown: '未知',
    error: '异常',
  }
  /**
   * NTP 特有词：'sync' / 'not_sync' 未进通用精确表（通用表只有 synced / 同步），
   * 本地登记后再委托 utils/statusType 的唯一映射表。
   */
  const ntpTagOverride: Record<string, TagType> = {
    sync: 'success',
    not_sync: 'warning',
  }
  function ntpTagType(s: string): TagType {
    const key = String(s ?? '').trim().toLowerCase()
    const hit = ntpTagOverride[key]
    if (hit) return hit
    return statusTagType(s)
  }

  // 实时时钟：委托 useClock（与 Layout 顶栏共用实现，审计 P2-7 去重），
  // 格式沿用系统总览的年月日时分秒口径
  const { now: nowTime } = useClock((ts) => formatDateTime(ts))

  const metricsText = computed(() => JSON.stringify(metrics.value, null, 2))
  const metricsHtml = computed(() => {
    let json = metricsText.value
    json = json.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    json = json.replace(/"([^"\\]*(\\.[^"\\]*)*)"(\s*:)?/g, (_m, str, _esc, colon) => {
      const cls = colon ? 'json-key' : 'json-string'
      return `<span class="${cls}">"${str}"</span>${colon || ''}`
    })
    json = json.replace(/\b(\d+\.?\d*)\b/g, '<span class="json-number">$1</span>')
    json = json.replace(/\b(true|false)\b/g, '<span class="json-boolean">$1</span>')
    json = json.replace(/\b(null)\b/g, '<span class="json-null">null</span>')
    json = json.replace(/\b(undefined)\b/g, '<span class="json-null">—</span>')
    return json
  })

  function fmtPct(v: unknown): string {
    if (v == null) return '-'
    const n = typeof v === 'number' ? v : Number(v)
    if (!Number.isFinite(n)) return '-'
    return `${n.toFixed(1)}%`
  }

  let refreshSeq = 0
  // 仅首次加载显示整页遮罩：refresh 由 usePolling 5s 轮询驱动（见下方生命周期），
  // 而 loading 绑在根节点 v-loading 上。原实现每轮都置 loading=true，导致系统总览页
  // 每 5 秒被蒙层覆盖一次、期间无法操作（与首页 Dashboard 同类问题）。
  let hasLoadedOnce = false
  async function refresh() {
    const seq = ++refreshSeq
    if (!hasLoadedOnce) loading.value = true
    const [sRes, vRes, mRes, nRes] = await Promise.allSettled([
      getStatus(),
      getVersion(),
      getMetrics(),
      getNtpStatus(),
      // 健康聚合报告：与 status 同批刷新，TTL 内直接复用共享快照
      health.refresh(),
    ])
    if (seq !== refreshSeq) return
    if (sRes.status === 'fulfilled') Object.assign(status, sRes.value)
    if (vRes.status === 'fulfilled') Object.assign(version, vRes.value)
    if (mRes.status === 'fulfilled') {
      // 后端 /api/metrics 返回 Prometheus 文本（见 status.ts 契约注释），需解析为 Record 供 JSON 查看器展示
      metrics.value = typeof mRes.value === 'string'
        ? parsePrometheusText(mRes.value)
        : (mRes.value || {})
    }
    if (nRes.status === 'fulfilled') Object.assign(ntp, nRes.value)
    loading.value = false
    // 无论各请求成败都标记"已加载过"：设备离线导致持续失败时，后续轮询也不再三秒遮挡整页。
    hasLoadedOnce = true
  }

  let ntpTimer: ReturnType<typeof setTimeout> | null = null
  async function syncNtp() {
    try {
      await ntpSync()
      ElMessage.success('NTP 同步已触发')
      if (ntpTimer) clearTimeout(ntpTimer)
      ntpTimer = setTimeout(refresh, 1000)
    } catch {
      // 写操作失败全局不再弹窗（拦截器只留痕），此处给出带动作语境的提示
      ElMessage.error('NTP 同步失败，请检查网关时间服务')
    }
  }

  // ===== 一键诊断报告 =====
  // 实现下沉到 useDiagnosticExport：系统总览按钮与顶栏用户菜单共用同一份采集逻辑，
  // 这里保留 genDiagnostic 名称以兼容 System.vue 既有绑定。
  const { diagLoading, bundleLoading, exportDiagnostic: genDiagnostic, exportBundle: genDiagnosticBundle } = useDiagnosticExport()

  /** 卸载后不再回写 loading，避免对已销毁组件做无意义更新（备份按钮使用） */
  let unmounted = false

  // ===== 系统备份 / 还原 =====
  const backupLoading = ref(false)
  const lastBackup = ref<SystemBackup | null>(null)
  async function doBackup() {
    backupLoading.value = true
    try {
      const [cfg, ver, logs] = await Promise.all([
        getConfig(),
        getVersion(),
        // 备份里的审计字段取自后端日志（与页面展示同源），拉取失败不阻断备份
        listAuditLogs({ limit: 200 }).catch(() => auditEntries.value),
      ])
      auditEntries.value = logs
      const backup = buildBackup(cfg, logs, ver.version)
      downloadBackup(backup)
      lastBackup.value = backup
      ElMessage.success('备份已生成并下载')
    } catch {
      ElMessage.error('生成备份失败')
    } finally {
      if (!unmounted) backupLoading.value = false
    }
  }
  async function onRestoreFile(file: File) {
    let backup
    try {
      backup = await parseBackupFile(file)
    } catch (e) {
      ElMessage.error(extractErrorMessage(e, '还原失败'))
      return
    }
    if (!(await confirmAction({
      message: `确认还原备份？将覆盖当前配置（${backup.meta.deviceCount} 设备 / ${backup.meta.pointCount} 点位），此操作不可撤销。`,
      title: '还原备份',
    }))) return
    try {
      const { postConfig } = await import('@/api/config')
      await postConfig(backup.config)
      ElMessage.success('还原成功，配置已更新')
      lastBackup.value = backup
    } catch (e) {
      ElMessage.error(extractErrorMessage(e, '还原失败'))
    }
  }

  // ===== 生命周期 =====
  // 统一走 usePolling（P2 收敛）：标签页隐藏时暂停、卸载自动清理、首次立即执行一次。
  // refresh 内部用 allSettled 吞错，退避不会触发——这与旧裸 setInterval 行为一致。
  usePolling(refresh, 5000)
  onMounted(() => {
    refreshAudit()
  })
  onUnmounted(() => {
    if (ntpTimer) clearTimeout(ntpTimer)
    refreshSeq++
    unmounted = true
  })

  return {
    // audit（后端日志，只读）
    auditEntries, refreshAudit,
    // webhook
    webhook, webhookUrl, saveWebhook, testWebhook, retryAllNotify,
    // sessions
    sessionList, revokeSession,
    // status / version / ntp / metrics
    status, version, ntp, metrics, loading,
    // 健康聚合（组件行 / 问题清单 / 状态）
    healthReport: health.report, healthRows, healthProblems, healthLoading: health.loading,
    healthOverallStatus: health.overallStatus,
    ntpStatusMap, ntpTagType, nowTime, metricsText, metricsHtml, fmtPct,
    refresh, syncNtp,
    // diagnostic
    diagLoading, genDiagnostic, bundleLoading, genDiagnosticBundle,
    // backup
    backupLoading, lastBackup, doBackup, onRestoreFile,
  }
}
