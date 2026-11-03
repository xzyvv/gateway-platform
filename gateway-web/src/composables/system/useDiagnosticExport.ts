/**
 * 一键诊断报告 / 诊断包导出
 *
 * 独立成 composable 的理由：原先只有「系统总览」页有入口，网关出问题时恰恰进不去
 * 该页面。现在系统总览按钮与顶栏用户菜单共用这一份实现，故障态下也能一键取证。
 *
 * 两种产物：
 *   - exportDiagnostic：单个 HTML 报告（现状，适合直接看、贴邮件）
 *   - exportBundle：zip 诊断包 = 报告 + 请求 HAR + 后端日志快照 + meta.json
 *     （WS 状态 / traceId 清单），适合完整交给研发：HAR 可直接拖进 DevTools
 *     Network 面板回放，日志快照补上"故障发生那一刻后端在做什么"
 *
 * 安全：配置节只放规模计数，不落凭据。GatewayConfig 含 MQTT/北向的账号密码，
 * 诊断包会被邮件/IM 转发，不能因为"方便排障"就把凭据带出去。
 */
import { getCurrentScope, onScopeDispose, ref } from 'vue'
import { ElMessage } from 'element-plus'
import { getHealthReport, getStatus, getVersion, healthReady } from '@/api/status'
import { getNtpStatus } from '@/api/network'
import { getConfig } from '@/api/config'
import { getNorthStatus } from '@/api/uplink'
import { sanitizeForLog } from '@/api/http/sanitize'
import type { GatewayConfig } from '@/api/types'
import type { HealthReport } from '@/api/status'
import type { NorthStatus } from '@/api/uplink'
import { listDevices } from '@/api/devices'
import { listPoints } from '@/api/points'
import { useReadPointNow } from '@/composables/data/useReadPointNow'
import { downloadLogs, getStatsReport } from '@/api/diagnostics'
import { getRecentLogs } from '@/api/apiRateLimit'
import { recordRead as recordDeviceRead } from '@/utils/deviceComms'
import { formatDateTime } from '@/utils/datetime'
import { buildDiagnosticReport, downloadDiagnosticReport, type DiagnosticInput } from '@/utils/diagnosticReport'
import { buildHar } from '@/utils/har'
import { buildZip } from '@/utils/zip'
import { csvEscapeFieldOptionalQuote } from '@/utils/exportCsv'
import { getPackets } from '@/api/protocolTools/packetCapture'
import type { Packet } from '@/api/protocolTools/types'
import { useErrorDisplay } from '@/composables/core/useErrorDisplay'
import { useRequestLogStore } from '@/stores/requestLog'
import { useWebsocketStore } from '@/stores/websocket'

/** 设备探测并发上限：串行探测几十台设备会让报告生成久到像卡死 */
const PROBE_CONCURRENCY = 6

/** 报文样本上限：抓包缓冲按时间淘汰，500 条足够覆盖一次故障窗口 */
const PACKETS_SAMPLE_LIMIT = 500

/** 诊断包内日志快照行数：覆盖一次故障窗口，同时控制 zip 体积 */
const LOG_SNAPSHOT_LINES = 2000

/** 网关采集错误分布最多列出的条目数 */
const GATEWAY_ERROR_LIMIT = 20

/** 网关侧失败 API 请求最多列出的条目数 */
const API_ERROR_LIMIT = 20

/**
 * 抓包缓冲 → packets.csv 文本（BOM + RFC4180 转义，hex/parsed 列可直接贴分析工具）。
 * 纯函数便于单测；导出为 zip 中的 packets.csv，补齐「诊断包缺报文样本」的取证缺口。
 */
export function buildPacketsCsv(pkts: Packet[]): string {
  const header = ['id', 'time', 'direction', 'bus', 'device', 'hex', 'parsed']
  const lines = [header.map(csvEscapeFieldOptionalQuote).join(',')]
  for (const p of pkts) {
    lines.push(
      [p.id, formatDateTime(p.time), p.direction, p.bus, p.deviceName, p.hex, p.parsed]
        .map(csvEscapeFieldOptionalQuote)
        .join(','),
    )
  }
  return '\uFEFF' + lines.join('\n')
}

/**
 * 配置快照（诊断包 config.snapshot.json）。
 *
 * 诊断包会被邮件/IM 转发，而 GatewayConfig 含 MQTT 密码、TLS 私钥、webhook
 * 密钥等凭据；这里统一走请求日志同一份脱敏规则（`sanitizeForLog`，按字段名
 * 子串命中替换为 `***`），保证「结构完整可排障、凭据不出网关」。
 *
 * 纯函数便于单测；导出供 spec 直接验证脱敏口径。
 */
export function buildConfigSnapshot(config: GatewayConfig): GatewayConfig {
  return sanitizeForLog(config)
}

/** 触发浏览器下载（zip / 任意 Blob 通用） */
function downloadBlob(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob)
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.click()
  URL.revokeObjectURL(url)
}

export function useDiagnosticExport() {
  const diagLoading = ref(false)
  const bundleLoading = ref(false)
  const { showCopyable } = useErrorDisplay()
  const { readOnce } = useReadPointNow()

  // 组件卸载后停止继续探测（避免离开页面后仍在后台打设备）
  let cancelled = false
  if (getCurrentScope()) {
    onScopeDispose(() => { cancelled = true })
  }

  /** 采集一次诊断输入；两种产物共用，避免口径分叉 */
  async function collect(): Promise<
    DiagnosticInput & { healthReport?: HealthReport; north?: NorthStatus }
  > {
    const [ver, st, hl, healthReport, ntpStat, cfg, devs, north] = await Promise.all([
      getVersion().catch(() => undefined),
      getStatus().catch(() => undefined),
      healthReady().catch(() => undefined),
      // C3：健康报告 / 北向状态是诊断包的两份关键快照，单独取原始结构落文件
      getHealthReport().catch(() => undefined),
      getNtpStatus().catch(() => undefined),
      getConfig().catch(() => undefined),
      listDevices().catch(() => []),
      getNorthStatus().catch(() => undefined),
    ])

    const deviceChecks: { id: string; name: string; reachable: boolean; latency?: number }[] = []
    const devQueue = [...devs]
    async function worker() {
      while (!cancelled) {
        const d = devQueue.shift()
        if (!d) break
        try {
          const pts = await listPoints(d.id).catch(() => [])
          if (pts.length === 0) {
            deviceChecks.push({ id: d.id, name: d.name, reachable: false })
            continue
          }
          const outcome = await readOnce(pts[0]!.id)
          if (outcome.ok) {
            deviceChecks.push({
              id: d.id,
              name: d.name,
              reachable: true,
              latency: Math.round(outcome.latencyMs),
            })
            recordDeviceRead(d.id, true, outcome.latencyMs)
          } else {
            deviceChecks.push({ id: d.id, name: d.name, reachable: false })
            recordDeviceRead(d.id, false, outcome.latencyMs)
          }
        } catch {
          deviceChecks.push({ id: d.id, name: d.name, reachable: false })
          recordDeviceRead(d.id, false, 0)
        }
      }
    }
    await Promise.all(
      Array.from({ length: Math.min(PROBE_CONCURRENCY, devs.length) }, () => worker()),
    )

    // 网关侧真实错误来源（两者都是网关自己的数据）：
    //   1. 采集错误分布：串口 IO / 协议解码 / 其它采集失败
    //   2. 访问日志里的失败请求：网关侧记录的 status ≥ 400
    // B3：前端本地操作审计已下线，诊断报告不再单列该节（审计只有后端一条链路）。
    const [stats, accessLogs] = await Promise.all([
      getStatsReport('day').catch(() => undefined),
      getRecentLogs(100).catch(() => []),
    ])

    const gatewayErrors = (stats?.error_dist ?? [])
      .slice(0, GATEWAY_ERROR_LIMIT)
      .map((e) => `${e.error_type}: ${e.count} 次（占比 ${e.percentage}%）`)

    const apiAccessErrors = accessLogs
      .filter((l) => l.status >= 400)
      .slice(0, API_ERROR_LIMIT)
      .map((l) => `[${formatDateTime(l.time)}] ${l.method} ${l.path} → ${l.status}（${l.latencyMs}ms）`)

    const reqLog = useRequestLogStore()
    const failedRequests = reqLog.getLogs()
      .filter((r) => r.kind !== 'ws' && (typeof r.status === 'string' || r.status >= 400))
      .slice(0, 10)
      .map((r) => ({
        method: r.method,
        url: r.url,
        status: r.status,
        duration: r.duration,
        time: r.timestamp,
        traceId: r.traceId,
        error: r.error,
      }))

    const wsStore = useWebsocketStore()
    return {
      version: ver,
      status: st
        ? {
            cpu_usage: st.cpu_usage,
            mem_usage: st.mem_usage,
            queue_length: st.queue_length,
            online_devices: st.online_devices,
          }
        : undefined,
      health: hl,
      healthReport,
      north,
      ntp: ntpStat,
      config: cfg,
      deviceChecks,
      // 网关侧真实错误（修复 P1-11：不再用浏览器本地审计日志冒充网关运行日志）
      gatewayErrors,
      apiAccessErrors,
      ws: {
        status: wsStore.status,
        reconnectCount: wsStore.reconnectCount,
        subscribers: wsStore.subscribers,
        lastError: wsStore.lastError,
      },
      failedRequests,
    }
  }

  /** 单个 HTML 报告（保持既有行为） */
  async function exportDiagnostic(): Promise<void> {
    if (diagLoading.value) return
    diagLoading.value = true
    try {
      downloadDiagnosticReport(buildDiagnosticReport(await collect()))
      ElMessage.success('诊断报告已生成并下载')
    } catch (e) {
      // 诊断报告导出失败需要可复制：现场要把完整错误回传研发，Toast 无法选中文本。
      void showCopyable(
        e,
        '生成诊断报告失败',
        `产物：诊断报告（HTML）\n时间：${formatDateTime(Date.now())}\n提示：可在浏览器 Network 中查看失败请求的 traceId 一并回传。`,
      )
    } finally {
      diagLoading.value = false
    }
  }

  /**
   * 诊断包（zip：report.html + requests.har + meta.json + packets.csv +
   * gateway.log + config.snapshot.json + health.json + north.json）
   *
   * C3 现场知识：现场排障最常问的四件事——「当时配置是什么 / 最近日志 /
   * 健康报告 / 北向通不通」——都应能在脱离网关的环境里直接查看。
   */
  async function exportBundle(): Promise<void> {
    if (bundleLoading.value) return
    bundleLoading.value = true
    try {
      const input = await collect()
      const reqLog = useRequestLogStore()
      const logs = reqLog.getLogs()
      const now = new Date()

      // packets.csv：抓包缓冲中的原始报文样本。抓包接口不可用（旧版网关）时
      // 省略该文件并在 meta 标注，不让取证缺口阻塞整个诊断包。
      let packetsCsv: string | null = null
      try {
        const pkts = await getPackets(PACKETS_SAMPLE_LIMIT)
        packetsCsv = buildPacketsCsv(pkts)
      } catch {
        /* 抓包接口不可用：省略 packets.csv */
      }

      // gateway.log：后端日志文件末尾快照。WebSocket tail 只能拿到订阅之后的新行，
      // 而故障取证往往发生在事后。旧版网关没有该接口时省略，不阻塞整个导出。
      let logText: string | null = null
      try {
        const text = await downloadLogs(LOG_SNAPSHOT_LINES)
        if (text.trim()) {
          logText = text
        }
      } catch {
        /* 日志接口不可用：省略 gateway.log */
      }

      // C3：配置快照 / 健康报告 / 北向状态落独立文件，便于研发离线比对
      // 「配置是否被改错」「健康聚合与现场现象是否一致」「北向断在哪一层」。
      const configSnapshot =
        input.config !== undefined ? buildConfigSnapshot(input.config) : null
      const healthReportJson =
        input.healthReport !== undefined ? JSON.stringify(input.healthReport, null, 2) : null
      const northJson = input.north !== undefined ? JSON.stringify(input.north, null, 2) : null

      // meta 只放"人和研发都需要先看一眼"的索引信息：版本、连接态、失败请求的 traceId。
      // 完整请求体在 requests.har 里，完整错误文本在 report.html 里。
      const meta = {
        generatedAt: now.toISOString(),
        generatedAtLocal: formatDateTime(now.getTime()),
        frontend: {
          version: input.version?.version ?? null,
          buildTime: input.version?.build_time ?? null,
          gitHash: input.version?.git_hash ?? null,
        },
        websocket: input.ws,
        requestLog: {
          sampled: logs.length,
          failed: input.failedRequests?.length ?? 0,
          samplingEnabled: reqLog.enabled,
        },
        // 修复（P1-11）：如实标注各错误节的来源，研发据此知道去哪查
        errorSources: {
          gatewayCollectErrors: {
            count: input.gatewayErrors?.length ?? 0,
            source: 'gateway collect stats (/api/diagnostics/stats error_dist)',
          },
          gatewayApiErrors: {
            count: input.apiAccessErrors?.length ?? 0,
            source: 'gateway access log (/api/access/rate-logs)',
          },
        },
        packets: {
          included: packetsCsv !== null,
          // 不重复计数：CSV 自带全量行，这里只标记是否包含
          source: 'packet-capture buffer (/api/protocol-tools/packet-capture/packets)',
        },
        logSnapshot: {
          included: logText !== null,
          lines: logText !== null ? logText.split('\n').length - 1 : 0,
          source: 'gateway log file tail (/api/diagnostics/logs/download)',
        },
        configSnapshot: {
          included: configSnapshot !== null,
          sanitized: true,
          source: 'gateway config (GET /api/config；凭据字段已替换为 ***)',
        },
        healthReport: {
          included: healthReportJson !== null,
          status: input.healthReport?.status ?? null,
          source: 'health aggregate (GET /api/health/report)',
        },
        northStatus: {
          included: northJson !== null,
          healthy: input.north?.healthy ?? null,
          channels: input.north?.channels?.length ?? 0,
          source: 'northbound channel status (GET /api/north/status)',
        },
        failedRequestTraces: (input.failedRequests ?? []).map((r) => ({
          traceId: r.traceId ?? null,
          method: r.method,
          url: r.url,
          status: r.status,
          time: r.time,
        })),
        note: '配置节仅含规模计数，不含凭据；完整配置请从配置管理页导出',
      }

      const entries = [
        { name: 'report.html', data: buildDiagnosticReport(input) },
        { name: 'requests.har', data: buildHar(logs) },
        { name: 'meta.json', data: JSON.stringify(meta, null, 2) },
      ]
      if (packetsCsv !== null) entries.push({ name: 'packets.csv', data: packetsCsv })
      if (logText !== null) entries.push({ name: 'gateway.log', data: logText })
      if (configSnapshot !== null) {
        entries.push({
          name: 'config.snapshot.json',
          data: JSON.stringify(configSnapshot, null, 2),
        })
      }
      if (healthReportJson !== null) entries.push({ name: 'health.json', data: healthReportJson })
      if (northJson !== null) entries.push({ name: 'north.json', data: northJson })
      const zip = buildZip(entries, now)
      downloadBlob(zip, `gateway-diagnostic-${now.getTime()}.zip`)

      const parts = ['报告', '请求 HAR', '连接状态']
      if (logText !== null) parts.push('后端日志')
      if (packetsCsv !== null) parts.push('报文 CSV')
      if (configSnapshot !== null) parts.push('配置快照')
      if (healthReportJson !== null) parts.push('健康报告')
      if (northJson !== null) parts.push('北向状态')
      ElMessage.success(`诊断包已生成并下载（${parts.join(' / ')}）`)
    } catch (e) {
      void showCopyable(
        e,
        '生成诊断包失败',
        `产物：诊断包（zip：report.html / requests.har / packets.csv / meta.json / gateway.log / config.snapshot.json / health.json / north.json）\n时间：${formatDateTime(Date.now())}\n提示：可在浏览器 Network 中查看失败请求的 traceId 一并回传。`,
      )
    } finally {
      bundleLoading.value = false
    }
  }

  return { diagLoading, bundleLoading, exportDiagnostic, exportBundle }
}
