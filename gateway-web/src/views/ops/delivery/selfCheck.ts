import { getConfig } from '@/api/config'
import type { GatewayConfig } from '@/api/types'
import type { VersionInfo } from '@/api/types'
import { getHealthReport, type HealthReport, type HealthBusRow } from '@/api/status'
import { listProtocolCapabilities, type ProtocolCapabilityEntry } from '@/api/protocols'
import { getNorthStatus, type NorthStatus } from '@/api/uplink'
import { getNtpStatus, type NtpStatus } from '@/api/network'
import { listCertificates, type Certificate } from '@/api/certificates'
import { listServices, type ServiceInfo } from '@/api/watchdog'
import { formatDateTime } from '@/utils/datetime'
import { DELIVERY_BUS_SUCCESS_RATE_WARN, queueBacklogged } from '@/utils/healthVerdict'

/** 单项结论：通过 / 提醒（不阻断交付）/ 未通过 */
export type CheckLevel = 'pass' | 'warn' | 'fail'

export const LEVEL_LABEL: Record<CheckLevel, string> = {
  pass: '通过',
  warn: '提醒',
  fail: '未通过',
}

export const LEVEL_TAG_TYPE: Record<CheckLevel, 'success' | 'warning' | 'danger'> = {
  pass: 'success',
  warn: 'warning',
  fail: 'danger',
}

export interface CheckItem {
  /** 固定标识：protocol / bus / north / clock / certificate / watchdog */
  id: string
  title: string
  level: CheckLevel
  detail: string
}

export interface SelfCheckResult {
  generatedAt: number
  items: CheckItem[]
  level: CheckLevel
}

/** 汇总多项结论：任一未通过即未通过；任一提醒即提醒；否则通过 */
export function summarizeLevel(levels: CheckLevel[]): CheckLevel {
  if (levels.includes('fail')) return 'fail'
  if (levels.includes('warn')) return 'warn'
  return 'pass'
}

/* ============================== 六项判定（纯函数，便于单测） ============================== */

/** 协议：配置里用到的协议必须都在后端注册表中 */
export function evalProtocols(
  capabilities: ProtocolCapabilityEntry[],
  config: GatewayConfig,
): CheckItem {
  const title = '协议'
  if (config.buses.length === 0) {
    return { id: 'protocol', title, level: 'fail', detail: '未配置任何总线，无法开始采集' }
  }
  const used = new Set<string>()
  for (const bus of config.buses) used.add(bus.protocol)
  for (const point of config.points) used.add(point.protocol)
  for (const device of config.devices ?? []) used.add(device.protocol)
  const registered = new Set(capabilities.map((c) => c.name))
  const unknown = [...used].filter((p) => !registered.has(p))
  if (unknown.length > 0) {
    return {
      id: 'protocol',
      title,
      level: 'fail',
      detail: `配置使用了未注册协议：${unknown.join('、')}（已注册 ${registered.size} 种）`,
    }
  }
  return {
    id: 'protocol',
    title,
    level: 'pass',
    detail: `已注册 ${registered.size} 种协议；当前使用 ${used.size} 种（${[...used].join('、')}），全部可用`,
  }
}

/** 总线：每条总线都要有采集、设备在线率达标、成功率达标 */
export function evalBuses(report: HealthReport, config: GatewayConfig): CheckItem {
  const title = '总线'
  if (config.buses.length === 0) {
    return { id: 'bus', title, level: 'fail', detail: '未配置任何总线' }
  }

  const levels: CheckLevel[] = []
  const notes: string[] = []
  for (const bus of config.buses) {
    const row = report.buses.find((b: HealthBusRow) => b.bus === bus.port_name)
    if (!row) {
      levels.push('warn')
      notes.push(`${bus.port_name}：暂无采集统计（网关可能刚启动）`)
      continue
    }
    if (row.total_reads === 0) {
      levels.push('warn')
      notes.push(`${bus.port_name}：近 24h 无采集数据`)
      continue
    }
    if (row.total_devices > 0 && row.online_devices === 0) {
      levels.push('fail')
      notes.push(`${bus.port_name}：${row.total_devices} 台设备全部离线`)
      continue
    }
    if (row.online_devices < row.total_devices) {
      levels.push('warn')
      notes.push(`${bus.port_name}：在线 ${row.online_devices}/${row.total_devices} 台`)
    } else {
      levels.push('pass')
    }
    if (row.success_rate !== null && row.success_rate < DELIVERY_BUS_SUCCESS_RATE_WARN) {
      levels.push('warn')
      notes.push(`${bus.port_name}：成功率 ${row.success_rate.toFixed(1)}%`)
    }
  }

  const level = summarizeLevel(levels)
  const summary = `共 ${config.buses.length} 条总线，设备在线 ${report.summary.online_devices}/${report.summary.total_devices}，采集成功率 ${
    report.summary.success_rate === null ? '—' : `${report.summary.success_rate.toFixed(1)}%`
  }`
  return {
    id: 'bus',
    title,
    level,
    detail: notes.length > 0 ? `${summary}；${notes.join('；')}` : summary,
  }
}

/** 北向：启用的通道必须连接，队列不能明显积压 */
export function evalNorth(status: NorthStatus): CheckItem {
  const title = '北向'
  const enabled = status.channels.filter((c) => c.enabled)
  const levels: CheckLevel[] = []
  const notes: string[] = []

  if (enabled.length === 0) {
    levels.push('warn')
    notes.push('未启用任何北向通道（当前仅本地采集，MQTT/Webhook 可在「上行」页启用）')
  }
  const disconnected = enabled.filter((c) => c.connected !== true)
  if (disconnected.length > 0) {
    levels.push('fail')
    notes.push(`未连接：${disconnected.map((c) => c.name).join('、')}`)
  }
  const stoppedServers = status.servers.filter((s) => !s.running)
  if (stoppedServers.length > 0) {
    levels.push('fail')
    notes.push(`北向 Server 未运行：${stoppedServers.map((s) => s.name).join('、')}`)
  }
  if (queueBacklogged(status.aggregated.pending_messages)) {
    levels.push('warn')
    notes.push(`待发送队列积压 ${status.aggregated.pending_messages} 条`)
  }
  if ((status.aggregated.lagged_dropped_batches ?? 0) > 0) {
    levels.push('warn')
    notes.push(`扇出订阅丢帧 ${status.aggregated.lagged_dropped_batches} 批`)
  }

  if (levels.length === 0) {
    return {
      id: 'north',
      title,
      level: 'pass',
      detail: `${enabled.length} 个通道已连接，待发送 ${status.aggregated.pending_messages} 条`,
    }
  }
  return { id: 'north', title, level: summarizeLevel(levels), detail: notes.join('；') }
}

/** 时钟：NTP 已校时且偏差在可接受范围 */
export function evalClock(ntp: NtpStatus): CheckItem {
  const title = '时钟'
  const status = String(ntp.status ?? '')
  if (status.startsWith('error')) {
    return { id: 'clock', title, level: 'fail', detail: `NTP 校时失败：${status}` }
  }
  if (status !== 'synced') {
    return {
      id: 'clock',
      title,
      level: 'warn',
      detail: '尚未完成 NTP 校时（status=idle），可稍后重试或检查时间源网络',
    }
  }
  const offset = Math.abs(ntp.offset_ms ?? 0)
  if (offset > 5000) {
    return {
      id: 'clock',
      title,
      level: 'fail',
      detail: `时钟偏差 ${offset} ms，超过 5000 ms 上限`,
    }
  }
  if (offset > 1000) {
    return {
      id: 'clock',
      title,
      level: 'warn',
      detail: `时钟偏差 ${offset} ms（建议 ≤1000 ms）`,
    }
  }
  return {
    id: 'clock',
    title,
    level: 'pass',
    detail: `NTP 已同步（${ntp.source ?? 'NTP'}），偏差 ${offset} ms${
      ntp.last_sync ? `，最近校时 ${formatDateTime(ntp.last_sync * 1000)}` : ''
    }`,
  }
}

/** 证书：已安装且未临近过期（无证书时提醒：HTTP 明文可交付内网） */
export function evalCertificates(certificates: Certificate[]): CheckItem {
  const title = '证书'
  const enabled = certificates.filter((c) => c.enabled)
  if (enabled.length === 0) {
    return {
      id: 'certificate',
      title,
      level: 'warn',
      detail: '未安装 TLS 证书，当前为 HTTP 明文访问（内网可接受，公网交付建议启用）',
    }
  }
  const expired = enabled.filter((c) => c.days_remaining < 0)
  if (expired.length > 0) {
    return {
      id: 'certificate',
      title,
      level: 'fail',
      detail: `证书已过期：${expired.map((c) => c.name).join('、')}`,
    }
  }
  const shortest = enabled.reduce((a, c) => (c.days_remaining < a.days_remaining ? c : a))
  if (shortest.days_remaining < 30) {
    return {
      id: 'certificate',
      title,
      level: 'warn',
      detail: `证书「${shortest.name}」剩余 ${shortest.days_remaining} 天（建议 ≥30 天）`,
    }
  }
  return {
    id: 'certificate',
    title,
    level: 'pass',
    detail: `${enabled.length} 张证书有效，最短剩余 ${shortest.days_remaining} 天（${shortest.name}）`,
  }
}

/** 看门狗：不得有已停止的服务；重启中的服务只提醒 */
export function evalWatchdog(services: ServiceInfo[]): CheckItem {
  const title = '看门狗'
  if (services.length === 0) {
    return { id: 'watchdog', title, level: 'warn', detail: '未获取到服务状态' }
  }
  const stopped = services.filter((s) => s.status === 'Stopped')
  if (stopped.length > 0) {
    return {
      id: 'watchdog',
      title,
      level: 'fail',
      detail: `服务已停止：${stopped.map((s) => s.name).join('、')}`,
    }
  }
  const restarting = services.filter((s) => s.status === 'Restarting')
  if (restarting.length > 0) {
    return {
      id: 'watchdog',
      title,
      level: 'warn',
      detail: `服务重启中：${restarting.map((s) => s.name).join('、')}`,
    }
  }
  const unknown = services.filter((s) => s.status === 'Unknown')
  const restarts = services.reduce((sum, s) => sum + s.restarts, 0)
  return {
    id: 'watchdog',
    title,
    level: 'pass',
    detail: `${services.length} 个服务运行正常，累计重启 ${restarts} 次${
      unknown.length > 0 ? `（${unknown.length} 个未启用/平台不支持）` : ''
    }`,
  }
}

function failed(title: string, id: string, reason: string): CheckItem {
  return { id, title, level: 'fail', detail: `接口调用失败：${reason}` }
}

function rejectionReason(result: PromiseSettledResult<unknown>, fallback: string): string {
  if (result.status !== 'rejected') return fallback
  return result.reason instanceof Error ? result.reason.message : String(result.reason)
}

/* ============================== 采集与汇总 ============================== */

/**
 * 执行六项自检。
 *
 * 全部并行请求，单项接口失败不会中断其它项：失败项直接记为「未通过」并写明原因。
 * 复用已有只读端点（/api/protocols、/api/config、/api/health/report、/api/north/status、
 * /api/ntp/status、/api/certificates、/api/watchdog/services），不新增后端聚合 API。
 */
export async function runSelfCheck(): Promise<SelfCheckResult> {
  const [caps, config, report, north, ntp, certificates, services] = await Promise.allSettled([
    listProtocolCapabilities(),
    getConfig(),
    getHealthReport(),
    getNorthStatus(),
    getNtpStatus(),
    listCertificates(),
    listServices(),
  ])

  const items: CheckItem[] = []
  items.push(
    caps.status === 'fulfilled' && config.status === 'fulfilled'
      ? evalProtocols(caps.value, config.value)
      : failed('协议', 'protocol', rejectionReason(caps, rejectionReason(config, '配置接口不可用'))),
  )
  items.push(
    report.status === 'fulfilled' && config.status === 'fulfilled'
      ? evalBuses(report.value, config.value)
      : failed('总线', 'bus', rejectionReason(report, rejectionReason(config, '配置接口不可用'))),
  )
  items.push(north.status === 'fulfilled' ? evalNorth(north.value) : failed('北向', 'north', north.reason))
  items.push(ntp.status === 'fulfilled' ? evalClock(ntp.value) : failed('时钟', 'clock', ntp.reason))
  items.push(
    certificates.status === 'fulfilled'
      ? evalCertificates(certificates.value)
      : failed('证书', 'certificate', certificates.reason),
  )
  items.push(
    services.status === 'fulfilled'
      ? evalWatchdog(services.value)
      : failed('看门狗', 'watchdog', services.reason),
  )

  return {
    generatedAt: Date.now(),
    items,
    level: summarizeLevel(items.map((i) => i.level)),
  }
}

function escCell(value: string): string {
  return value.replace(/\|/g, '\\|').replace(/\r?\n/g, ' ')
}

/** 生成可复制的 Markdown 自检报告 */
export function buildSelfCheckMarkdown(result: SelfCheckResult, version?: VersionInfo | null): string {
  const versionText = version
    ? `${version.version}（build ${version.build_time || '—'} / git ${version.git_hash || '—'}）`
    : '—'
  const failCount = result.items.filter((i) => i.level === 'fail').length
  const warnCount = result.items.filter((i) => i.level === 'warn').length
  const verdict = `${LEVEL_LABEL[result.level]}（${failCount} 项未通过，${warnCount} 项提醒）`

  const rows = result.items
    .map((i) => `| ${escCell(i.title)} | ${LEVEL_LABEL[i.level]} | ${escCell(i.detail)} |`)
    .join('\n')

  return [
    '# 网关交付自检报告',
    '',
    `- 网关版本：${versionText}`,
    `- 生成时间：${formatDateTime(result.generatedAt)}`,
    `- 自检结论：${verdict}`,
    '',
    '| 检查项 | 结论 | 说明 |',
    '| --- | --- | --- |',
    rows,
    '',
    '> 判定口径：任一「未通过」即不通过；仅「提醒」表示可交付但需现场确认。',
  ].join('\n')
}

/* ============================== 交付检查单 ============================== */

export interface ManualItemDef {
  id: string
  title: string
  hint: string
}

/** 自检覆盖不到的现场人工确认项 */
export const MANUAL_ITEMS: ManualItemDef[] = [
  { id: 'wiring', title: '现场接线核对', hint: '电源、接地、RS485 屏蔽/终端电阻、天线已按图施工' },
  { id: 'addr', title: '设备地址与点表核对', hint: '从站地址、寄存器/IOA 与现场点表一致' },
  { id: 'value', title: '首点值与现场表计核对', hint: '抽测 3 个点位，数值与现场表计/后台一致' },
  { id: 'recovery', title: '断网/重启恢复验证', hint: '断开上行或重启网关后，采集与补传能自动恢复' },
]

export type ChecklistStatus = CheckLevel | 'todo'

export interface ChecklistEntry {
  id: string
  title: string
  category: string
  source: 'auto' | 'manual'
  status: ChecklistStatus
  detail: string
  note: string
}

export interface DeliveryChecklist {
  kind: 'gateway-delivery-checklist'
  schema_version: 1
  exported_at: string
  gateway: { version: string; build_time: string; git_hash: string }
  operator: string
  overall: ChecklistStatus
  checks: ChecklistEntry[]
}

/** 由六项自检结果 + 人工确认项组装交付检查单 */
export function buildChecklist(
  result: SelfCheckResult | null,
  manual: Record<string, { checked: boolean; note: string }>,
  operator: string,
  version?: VersionInfo | null,
): DeliveryChecklist {
  const checks: ChecklistEntry[] = []

  for (const item of result?.items ?? []) {
    checks.push({
      id: item.id,
      title: item.title,
      category: '自动自检',
      source: 'auto',
      status: item.level,
      detail: item.detail,
      note: '',
    })
  }
  if (!result) {
    for (const id of ['protocol', 'bus', 'north', 'clock', 'certificate', 'watchdog']) {
      checks.push({
        id,
        title: id,
        category: '自动自检',
        source: 'auto',
        status: 'todo',
        detail: '尚未执行一键自检',
        note: '',
      })
    }
  }

  for (const def of MANUAL_ITEMS) {
    const state = manual[def.id]
    checks.push({
      id: def.id,
      title: def.title,
      category: '人工确认',
      source: 'manual',
      status: state?.checked ? 'pass' : 'todo',
      detail: def.hint,
      note: state?.note ?? '',
    })
  }

  const statuses = checks.map((c) => c.status)
  let overall: ChecklistStatus = 'pass'
  if (statuses.includes('fail')) overall = 'fail'
  else if (statuses.includes('warn') || statuses.includes('todo')) overall = 'warn'

  return {
    kind: 'gateway-delivery-checklist',
    schema_version: 1,
    exported_at: new Date().toISOString(),
    gateway: {
      version: version?.version ?? '',
      build_time: version?.build_time ?? '',
      git_hash: version?.git_hash ?? '',
    },
    operator: operator.trim(),
    overall,
    checks,
  }
}

const STATUS_TEXT: Record<ChecklistStatus, string> = {
  pass: '通过',
  warn: '提醒',
  fail: '未通过',
  todo: '待确认',
}

/** 交付检查单导出为 Markdown */
export function checklistToMarkdown(list: DeliveryChecklist): string {
  const rows = list.checks
    .map(
      (c) =>
        `| ${escCell(c.title)} | ${c.category} | ${STATUS_TEXT[c.status]} | ${escCell(c.detail)}${
          c.note ? `；备注：${escCell(c.note)}` : ''
        } |`,
    )
    .join('\n')

  return [
    '# 网关交付检查单',
    '',
    `- 网关版本：${list.gateway.version || '—'}（build ${list.gateway.build_time || '—'} / git ${
      list.gateway.git_hash || '—'
    }）`,
    `- 导出时间：${formatDateTime(list.exported_at)}`,
    `- 交付人：${list.operator || '—'}`,
    `- 总体结论：${STATUS_TEXT[list.overall]}`,
    '',
    '| 检查项 | 类别 | 结论 | 说明 |',
    '| --- | --- | --- | --- |',
    rows,
    '',
    '> 「待确认」表示该项尚未执行/勾选；全部通过或仅有「提醒」方可交付。',
  ].join('\n')
}
