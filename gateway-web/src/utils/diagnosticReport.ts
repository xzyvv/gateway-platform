/**
 * 一键诊断报告
 * 采集系统状态、版本、健康、NTP、设备连通性、网关运行错误、配置版本，
 * 生成可导出的诊断报告 HTML
 *
 * 修复（P1-11）：「最近错误日志」一节原先放的是浏览器 localStorage 里的前端
 * 操作审计日志，与网关运行状态无关，现场几乎恒空。现改为两节并各自标注来源：
 * 网关采集错误 / 网关侧失败的 API 请求（前端本地审计已下线，不再单列）。
 *
 * 安全修复（W-1）：
 * 旧实现把所有字段直接拼成 HTML 字符串（含 input 里的错误行、设备名、版本号等
 * 来自后端日志/设备返回的可控内容），攻击者可在日志/设备名里塞 `<script>`/`<img onerror>`
 * 触发 XSS。改为结构化 DOM API：所有用户/设备来源数据走 `textContent`，
 * 让浏览器负责 HTML 实体转义；只在 CSS 类名等固定白名单字符串处用 innerHTML。
 */
import type { GatewayConfig, VersionInfo, HealthReady } from '@/api/types'
import type { NtpStatus } from '@/api/network'
import { formatDateTime } from '@/utils/datetime'
import { healthComponentLabel, healthStatusInfo, healthWorstSeverity } from '@/utils/healthStatus'
import { triggerDownload } from '@/utils/exportCsv'

export interface DiagnosticInput {
  version?: VersionInfo
  status?: { cpu_usage?: number; mem_usage?: number; queue_length?: number; online_devices?: number }
  health?: HealthReady
  ntp?: NtpStatus
  config?: GatewayConfig
  deviceChecks?: { id: string; name: string; reachable: boolean; latency?: number }[]
  /**
   * 网关运行错误（来自网关自身，非浏览器本地）。
   *
   * 修复（P1-11）：原实现这里放的是**浏览器 localStorage 里的前端操作审计日志**，
   * 它只在用户做过失败的前端操作时才有内容——现场网关重启、采集失败、串口报错
   * 这类真正的网关故障一条都不会出现，该节几乎恒空。改为承接网关侧真实运行数据：
   *   - gatewayErrors：网关采集错误分布（/api/diagnostics/stats 的 error_dist）
   *   - apiAccessErrors：网关侧 API 访问日志里的错误请求（/api/access/rate-logs）
   */
  gatewayErrors?: string[]
  apiAccessErrors?: string[]
  /** WebSocket 连接态：断连/重连风暴是现场最常见的一类问题，报告里必须能直接看到 */
  ws?: {
    status: string
    reconnectCount: number
    subscribers: number
    lastError?: string | null
  }
  /** 最近失败的前端请求（含 traceId），离线报障时研发可据此对上网关日志 */
  failedRequests?: {
    method: string
    url: string
    status: number | string
    duration: number
    time: number
    traceId?: string
    error?: string
  }[]
}

/** HTML 实体转义：用于任何要插到 innerHTML 字符串里的"值" */
function esc(value: unknown): string {
  return String(value ?? '')
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;')
}

/** CSS 类名白名单（拒绝任何非 [a-z0-9-] 字符，避免 class 注入） */
function escClass(value: string, allowed: ReadonlySet<string>): string {
  return allowed.has(value) ? value : ''
}

const STATUS_CLASSES = new Set(['ok', 'bad', 'warn'])
const NTP_CLASSES = new Set(['ok', 'warn', 'bad'])

/** 构造一行 key/value 表格行（值走 esc） */
function kvRow(th: string, td: string, tdClass = ''): string {
  const cls = escClass(tdClass, STATUS_CLASSES)
  const clsAttr = cls ? ` class="${esc(cls)}"` : ''
  return `<tr><th>${esc(th)}</th><td${clsAttr}>${esc(td)}</td></tr>`
}

export function buildDiagnosticReport(input: DiagnosticInput): string {
  const now = formatDateTime(Date.now())
  const gwErrCount = input.gatewayErrors?.length || 0
  const apiErrCount = input.apiAccessErrors?.length || 0
  const devTotal = input.deviceChecks?.length || 0
  const devOnline = input.deviceChecks?.filter((d) => d.reachable).length || 0
  const healthStatus = String(input.health?.status ?? '')
  // 整体判定按组件严重度，不按后端聚合的 status：它对任何非 ok 组件都判 degraded，
  // 直接沿用会与组件明细口径不一致。
  const healthSev = healthWorstSeverity(input.health?.components, input.health?.status)
  const healthClass = healthSev === 'ok' ? 'ok' : healthSev === 'error' ? 'bad' : 'warn'
  const healthOverallText =
    healthSev === 'ok' ? '正常' : healthSev === 'error' ? '异常' : '降级'
  // 保留后端原始状态词：报障时要能直接对着网关日志/后端代码核对
  const healthOverallLabel = input.health
    ? `${healthOverallText}（后端原始状态：${healthStatus || '—'}）`
    : '—'

  // 设备表：每行的 name/id/latency 全部走 esc；class 走白名单
  const deviceRows = (input.deviceChecks ?? [])
    .map((d) => {
      const rowClass = d.reachable ? 'ok' : 'bad'
      return (
        '<tr>' +
        `<td>${esc(d.name)} (${esc(d.id)})</td>` +
        `<td class="${escClass(rowClass, STATUS_CLASSES)}">${d.reachable ? '在线' : '离线'}</td>` +
        `<td>${esc(d.latency ?? '—')}</td>` +
        '</tr>'
      )
    })
    .join('')

  // 健康检查组件表
  // 组件名与状态名走 utils/healthStatus（与界面同源）：现场看报告不该再猜英文词，
  // 但后端原始状态词一并保留，便于研发对网关日志核对。
  const componentRows = input.health
    ? Object.entries(input.health.components)
        .map(([k, v]) => {
          const raw = String(v ?? '')
          const info = healthStatusInfo(raw)
          const cls = info.severity === 'ok' ? 'ok' : info.severity === 'error' ? 'bad' : 'warn'
          const text = info.label === raw ? raw : `${info.label}（${raw}）`
          return `<tr><td>${esc(healthComponentLabel(k))}</td><td class="${escClass(cls, STATUS_CLASSES)}">${esc(text)}</td></tr>`
        })
        .join('')
    : ''

  // 错误日志块：用 <pre> + esc；多行保留换行，浏览器会原样显示
  // 两个来源分开渲染，研发据此知道该去哪查（前端本地审计已下线，不再单列）。
  const errorBlock = (lines: string[] | undefined, emptyText: string) =>
    lines?.length
      ? `<pre>${lines.map((s) => esc(s)).join('\n')}</pre>`
      : `<p>${esc(emptyText)}</p>`

  const gatewayErrorsBlock = errorBlock(
    input.gatewayErrors,
    '网关侧未记录到采集错误（统计窗口内没有失败采集）',
  )
  const apiAccessErrorsBlock = errorBlock(
    input.apiAccessErrors,
    '网关侧未记录到失败的 API 请求',
  )

  // 最近失败请求：traceId 是报障关键凭据，单独成列便于复制；所有字段走 esc
  const failedRows = (input.failedRequests ?? [])
    .map(
      (r) =>
        '<tr>' +
        `<td>${esc(formatDateTime(r.time))}</td>` +
        `<td>${esc(r.method)} ${esc(r.url)}</td>` +
        `<td>${esc(String(r.status))}</td>` +
        `<td>${esc(r.duration)}</td>` +
        `<td class="meta">${esc(r.traceId ?? '—')}</td>` +
        `<td>${esc(r.error ?? '—')}</td>` +
        '</tr>',
    )
    .join('')
  const failedBlock = input.failedRequests?.length
    ? `<table>
  <tr><th>时间</th><th>请求</th><th>状态</th><th>耗时(ms)</th><th>traceId</th><th>错误</th></tr>
  ${failedRows}
</table>`
    : '<p>未记录到失败请求（需先开启请求监视器采样）</p>'

  const ntpStatus = String(input.ntp?.status ?? '')
  const ntpCls = ntpStatus === 'synced' ? 'ok' : 'warn'

  const titleNow = esc(now)
  const versionStr = esc(input.version?.version ?? '—')
  const buildTimeStr = esc(input.version?.build_time ?? '—')
  const gitHashStr = esc(input.version?.git_hash ?? '—')

  // 整份 HTML 模板里：只有固定 CSS / 静态结构用裸 HTML，
  // 任何来自 input.* 的"值"（含错误日志行、device name/id、health key/value、
  // version/git_hash、ntp.status）一律走 esc()。
  // 标题里的计数也走 esc（数字转字符串无注入风险，但保持口径一致）。
  return `<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>网关诊断报告 - ${titleNow}</title>
<style>
  body { font-family: -apple-system, "Segoe UI", sans-serif; max-width: 900px; margin: 24px auto; padding: 0 16px; color: #1f2937; }
  h1 { color: #2563eb; border-bottom: 2px solid #2563eb; padding-bottom: 8px; }
  h2 { color: #1e40af; margin-top: 28px; }
  table { width: 100%; border-collapse: collapse; margin: 10px 0; }
  th, td { border: 1px solid #e5e7eb; padding: 8px 12px; text-align: left; }
  th { background: #f3f4f6; }
  .ok { color: #16a34a; font-weight: 600; }
  .bad { color: #dc2626; font-weight: 600; }
  .warn { color: #d97706; font-weight: 600; }
  .meta { color: #6b7280; font-size: 0.8125rem; }
  pre { background: #f9fafb; padding: 12px; border-radius: 6px; overflow-x: auto; white-space: pre-wrap; word-break: break-all; }
</style>
</head>
<body>
<h1>网关诊断报告</h1>
<p class="meta">生成时间：${titleNow}</p>

<h2>1. 版本信息</h2>
<table>
  ${kvRow('版本', versionStr)}
  ${kvRow('构建时间', buildTimeStr)}
  ${kvRow('Git Hash', gitHashStr)}
</table>

<h2>2. 系统状态</h2>
<table>
  ${kvRow('CPU 使用率', `${input.status?.cpu_usage ?? '—'}%`)}
  ${kvRow('内存使用率', `${input.status?.mem_usage ?? '—'}%`)}
  ${kvRow('采集队列长度', String(input.status?.queue_length ?? '—'))}
  ${kvRow('在线设备数', String(input.status?.online_devices ?? '—'))}
  ${input.ws ? kvRow('WS 连接', input.ws.status, input.ws.status === 'connected' ? 'ok' : 'warn') : ''}
  ${input.ws ? kvRow('WS 重连次数', String(input.ws.reconnectCount)) : ''}
  ${input.ws ? kvRow('WS 订阅数', String(input.ws.subscribers)) : ''}
  ${input.ws?.lastError ? kvRow('WS 最近错误', input.ws.lastError) : ''}
</table>

<h2>3. 健康检查</h2>
<p>整体状态：<span class="${escClass(healthClass, STATUS_CLASSES)}">${esc(healthOverallLabel)}</span></p>
${input.health ? `
<table>
  <tr><th>组件</th><th>状态</th></tr>
  ${componentRows}
</table>` : '<p>暂无数据</p>'}

<h2>4. NTP 同步</h2>
<p>状态：<span class="${escClass(ntpCls, NTP_CLASSES)}">${esc(ntpStatus || '—')}</span>，偏移：${esc(input.ntp?.offset_ms ?? '—')} ms</p>

<h2>5. 设备连通性检测</h2>
<p>共 ${esc(devTotal)} 台设备，在线 ${esc(devOnline)} 台</p>
${input.deviceChecks?.length ? `
<table>
  <tr><th>设备</th><th>状态</th><th>延迟(ms)</th></tr>
  ${deviceRows}
</table>` : '<p>未执行检测</p>'}

<h2>6. 网关采集错误（${esc(gwErrCount)} 条）</h2>
<p class="meta">来源：网关采集统计（/api/diagnostics/stats），反映网关自身采集失败</p>
${gatewayErrorsBlock}

<h2>7. 网关侧失败的 API 请求（${esc(apiErrCount)} 条）</h2>
<p class="meta">来源：网关访问日志（/api/access/rate-logs）</p>
${apiAccessErrorsBlock}

<h2>8. 最近失败请求（含 traceId）</h2>
${failedBlock}

<h2>9. 配置概览</h2>
<table>
  ${kvRow('总线数', String((input.config?.buses || []).length))}
  ${kvRow('设备数', String(input.config?.devices?.length || 0))}
  ${kvRow('点位数', String(input.config?.points?.length || 0))}
</table>
</body>
</html>`
}

/**
 * 单条故障的「诊断凭据」：用户复制后贴给研发 / 客服，凭 traceId 可直接对上网关日志。
 *
 * 与整份 HTML 诊断报告（buildDiagnosticReport）分工不同：
 * 这里是**即时**排障——出错当下从 toast/请求监视器/通知中心一键复制一段纯文本。
 */
export interface DiagnosticTicket {
  /** 发生时间戳（ms） */
  time?: number
  method?: string
  url?: string
  /** HTTP 状态码、`biz:8000`、`NETWORK` 等 */
  status?: number | string
  /** 业务错误码（ApiResponse.code 或 HTTP 状态码） */
  code?: number
  duration?: number
  traceId?: string
  error?: string
  /** 出错时所在页面路径 */
  page?: string
}

/** 生成可一键复制的诊断文本（字段缺省时自动省略对应行） */
export function buildDiagnosticText(t: DiagnosticTicket): string {
  const lines = ['【网关诊断信息】']
  if (t.time) lines.push(`时间: ${formatDateTime(t.time)}`)
  if (t.method || t.url) lines.push(`请求: ${[t.method, t.url].filter(Boolean).join(' ')}`)
  if (t.status !== undefined) lines.push(`结果: ${t.status}`)
  if (t.code !== undefined) lines.push(`错误码: ${t.code}`)
  if (t.duration !== undefined) lines.push(`耗时: ${t.duration}ms`)
  if (t.traceId) lines.push(`traceId: ${t.traceId}`)
  if (t.error) lines.push(`错误: ${t.error}`)
  if (t.page) lines.push(`页面: ${t.page}`)
  lines.push(t.traceId ? '（报障时请附上 traceId，研发可据此定位网关日志）' : '（请附上本段信息以便研发定位）')
  return lines.join('\n')
}

/** 触发浏览器下载 HTML 报告 */
export function downloadDiagnosticReport(reportHtml: string) {
  const blob = new Blob([reportHtml], { type: 'text/html;charset=utf-8' })
  triggerDownload(blob, `gateway-diagnostic-${Date.now()}.html`)
}
