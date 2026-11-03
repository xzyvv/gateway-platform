/**
 * 诊断运维 raw -> View 转换函数
 *
 * 从 ./view 抽出的纯映射：snake_case + bigint(raw, ts-rs 生成)
 * -> camelCase + number 时间戳(View)。不发起请求、不持有状态，供 ./view 包装函数调用。
 *
 * 抓包相关的 ParsedPacket/CapturedPacket 转换已随诊断侧抓包端点一起删除：
 * 抓包统一由 `api/protocolTools/packetCapture` 消费 `/api/protocol-tools/packet-capture/*`。
 */
import type {
  BusLoad,
  BusLoadHistory,
  CommonIssue,
  DiagResult,
  GatewayDiag,
  ModbusError,
  StatsReport,
} from './raw'
import type {
  BusLoadHistoryView,
  BusLoadView,
  CommonIssueView,
  DiagResultView,
  DiagStepView,
  GatewayDiagView,
  ModbusErrorView,
  StatsReportView,
} from './types'

/** 仅本文件内部使用：把总线负载历史点转成视图态 */
function toBusLoadHistoryView(h: BusLoadHistory): BusLoadHistoryView {
  return { time: Number(h.time_ms), tx: h.tx, rx: h.rx }
}

export function toBusLoadView(b: BusLoad): BusLoadView {
  return {
    bus: b.bus,
    type: b.bus_type,
    txRate: b.tx_rate,
    rxRate: b.rx_rate,
    collisionRate: b.collision_rate,
    avgLatency: b.avg_latency_ms,
    backlog: b.backlog,
    utilization: b.utilization,
    history: b.history.map(toBusLoadHistoryView),
  }
}

export function toModbusErrorView(e: ModbusError): ModbusErrorView {
  return { code: e.code, name: e.name, meaning: e.meaning, cause: e.cause, solution: e.solution }
}

export function toCommonIssueView(i: CommonIssue): CommonIssueView {
  return { symptom: i.symptom, checks: i.checks }
}

export function toStatsReportView(r: StatsReport): StatsReportView {
  return {
    period: r.period,
    startAt: Number(r.start_at_ms),
    endAt: Number(r.end_at_ms),
    deviceStats: r.device_stats.map((d) => ({
      deviceId: d.device_id,
      deviceName: d.device_name,
      protocol: d.protocol,
      totalReads: d.total_reads,
      success: d.success,
      failed: d.failed,
      successRate: d.success_rate,
      avgLatency: d.avg_latency_ms,
      dataPoints: d.data_points,
    })),
    busStats: r.bus_stats.map((b) => ({
      bus: b.bus,
      protocol: b.protocol,
      totalReads: b.total_reads,
      success: b.success,
      failed: b.failed,
      successRate: b.success_rate,
      avgLatency: b.avg_latency_ms,
      deviceCount: b.device_count,
      onlineDevices: b.online_devices,
      timeoutRate: b.timeout_rate,
      lastErrorCode: b.last_error_code,
      reconnectCount: b.reconnect_count,
    })),
    errorDist: r.error_dist.map((e) => ({
      errorType: e.error_type,
      count: e.count,
      percentage: e.percentage,
    })),
    summary: {
      totalReads: r.summary.total_reads,
      successRate: r.summary.success_rate,
      avgLatency: r.summary.avg_latency_ms,
      totalDataPoints: r.summary.total_data_points,
    },
  }
}

function mapDiagStatus(s: string): DiagStepView['status'] {
  if (s === 'pass') return 'pass'
  if (s === 'warning') return 'warning'
  if (s === 'fail') return 'fail'
  return 'pending'
}

function mapDiagResultStatus(s: string): DiagResultView['status'] {
  if (s === 'healthy') return 'healthy'
  if (s === 'degraded') return 'degraded'
  return 'offline'
}

export function toDiagResultView(r: DiagResult): DiagResultView {
  return {
    pointId: r.point_id,
    pointName: r.point_name,
    status: mapDiagResultStatus(r.status),
    steps: r.steps.map((s) => ({
      name: s.name,
      status: mapDiagStatus(s.status),
      message: s.message,
      detail: s.detail ?? undefined,
    })),
    suggestion: r.suggestion,
    lastCheckAt: Number(r.last_check_at_ms),
  }
}

export function toGatewayDiagView(d: GatewayDiag): GatewayDiagView {
  return {
    cpuTemp: d.cpu_temp,
    cpuUsage: d.cpu_usage,
    memUsage: d.mem_usage,
    diskUsage: d.disk_usage,
    hostUptime: d.host_uptime_str,
    processUptime: d.process_uptime_str,
    netInterfaces: d.net_interfaces.map((n) => ({
      name: n.name,
      ip: n.ip,
      status: n.status as 'up' | 'down',
      rx: Number(n.rx),
      tx: Number(n.tx),
    })),
    serialPorts: d.serial_ports.map((s) => ({
      name: s.name,
      status: s.status as 'ok' | 'error',
      desc: s.desc,
    })),
    loadAvg: d.load_avg,
    processes: d.processes,
  }
}
