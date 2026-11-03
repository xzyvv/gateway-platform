/**
 * 诊断运维 View-Friendly 适配层（camelCase + number 时间戳 + CSV 导出）
 *
 * 后端 ts-rs 把 i64/u64 映射为 bigint，且字段名为 snake_case。
 * View 历史用 camelCase + number 时间戳，这里统一适配，减少 view 改动。
 *
 * 依赖：[./raw]（raw API + ts-rs 类型重导出）
 * 类型声明见 [./types]（经本文件重新导出），转换函数见 [./convert]。
 * 调用方应优先从 [./index]（barrel）导入以保持兼容。
 */
import {
  listBusLoads,
  getBusLoad,
  refreshBusLoads,
  listModbusErrors,
  listCommonIssues,
  getStatsReport,
  diagnosePoint,
  getGatewayDiag,
  type StatsPeriod,
} from './raw'
import {
  toBusLoadView,
  toModbusErrorView,
  toCommonIssueView,
  toStatsReportView,
  toDiagResultView,
  toGatewayDiagView,
} from './convert'
import type {
  BusLoadView,
  CommonIssueView,
  DiagResultView,
  GatewayDiagView,
  ModbusErrorView,
  StatsReportView,
} from './types'
import { formatDateTime } from '@/utils/datetime'
import { csvEscapeField, downloadCsvText } from '@/utils/exportCsv'

// View 接口 / 辅助类型 / 示例点位与本文件同属 View 适配层，统一重导出保持兼容
export * from './types'

// ==== View Wrapper 函数（raw -> View）====

export function listBusLoadsView(): Promise<BusLoadView[]> {
  return listBusLoads().then((arr) => arr.map(toBusLoadView))
}

export function getBusLoadView(bus: string): Promise<BusLoadView> {
  return getBusLoad(bus).then(toBusLoadView)
}

export function refreshBusLoadsView(): Promise<BusLoadView[]> {
  return refreshBusLoads().then((arr) => arr.map(toBusLoadView))
}

export function listModbusErrorsView(): Promise<ModbusErrorView[]> {
  return listModbusErrors().then((arr) => arr.map(toModbusErrorView))
}

export function listCommonIssuesView(): Promise<CommonIssueView[]> {
  return listCommonIssues().then((arr) => arr.map(toCommonIssueView))
}

export function getStatsReportView(period: StatsPeriod = 'day'): Promise<StatsReportView> {
  return getStatsReport(period).then(toStatsReportView)
}

export function diagnosePointView(pointId: string): Promise<DiagResultView> {
  return diagnosePoint(pointId).then(toDiagResultView)
}

export function getGatewayDiagView(): Promise<GatewayDiagView> {
  return getGatewayDiag().then(toGatewayDiagView)
}

// ============================================================================
// CSV 导出（纯前端逻辑，迁移自 mock/diagnostics/statsReport.ts）
// ============================================================================

export function exportStatsReportCsv(report: StatsReportView): void {
  // 窗口内无采集时后端返回 null，CSV 里导出「—」而不是 null 字面量
  const pct = (v: number | null) => (v == null ? '—' : `${v}%`)
  const ms = (v: number | null) => (v == null ? '—' : `${v} ms`)
  const rows: (string | number)[][] = [
    ['统计周期', report.period],
    ['起始时间', formatDateTime(report.startAt)],
    ['结束时间', formatDateTime(report.endAt)],
    ['总采集次数', report.summary.totalReads],
    ['采集成功率', pct(report.summary.successRate)],
    ['平均延迟', ms(report.summary.avgLatency)],
    ['数据点数', report.summary.totalDataPoints],
    [],
    ['设备ID', '设备名称', '协议', '总采集', '成功', '失败', '成功率', '平均延迟', '数据点'],
    ...report.deviceStats.map((d) => [d.deviceId, d.deviceName, d.protocol, d.totalReads, d.success, d.failed, pct(d.successRate), ms(d.avgLatency), d.dataPoints]),
  ]
  // 统一走 csvEscapeField（全部双引号包裹 + RFC 4180 + 公式注入防护），格式与旧实现一致；
  // 下载统一走 downloadCsvText（BOM 由统一入口附加）
  const csv = rows.map((r) => r.map((c) => csvEscapeField(c)).join(',')).join('\n')
  downloadCsvText(csv, `采集统计_${report.period}.csv`)
}