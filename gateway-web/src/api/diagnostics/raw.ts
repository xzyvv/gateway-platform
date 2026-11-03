/**
 * 诊断运维 Raw API（直接返回后端 ts-rs 类型：bigint 时间戳 + snake_case 字段）
 *
 * 路由：
 * - GET  /api/diagnostics/bus-load              -> listBusLoads
 * - GET  /api/diagnostics/bus-load/:bus          -> getBusLoad
 * - POST /api/diagnostics/bus-load/refresh        -> refreshBusLoads
 * - GET  /api/diagnostics/modbus-errors          -> listModbusErrors
 * - GET  /api/diagnostics/modbus-errors/issues      -> listCommonIssues
 * - GET  /api/diagnostics/stats                   -> getStatsReport
 * - POST /api/diagnostics/task-diag/:pointId       -> diagnosePoint
 * - GET  /api/diagnostics/gateway                 -> getGatewayDiag
 *
 * 后端实现：[../../../../gateway-api/src/web/api/api_diagnostics/]
 * 类型由 ts-rs 生成：../generated/<Type>.ts
 *
 * View-friendly 适配层（camelCase + number 时间戳）见 [./view]。
 * 调用方应优先从 [./index]（barrel）导入以保持兼容。
 */
import { request } from '../index'

// ============================================================================
// 远程诊断命令执行（既有功能，POST /api/diagnostics/exec）
// 兼容 useLogs.ts 等历史调用方
// ============================================================================

export function diagnosticsExec(command: string): Promise<{ output: string }> {
  return request<{ output: string }>({
    url: '/api/diagnostics/exec',
    method: 'POST',
    data: { command },
  })
}

import type {
  BusLoad,
  BusLoadHistory,
  CommonIssue,
  DiagResult,
  GatewayDiag,
  ModbusError,
  ScheduleTimeline,
  StatsReport,
} from '../generated'

export type {
  BusLoad,
  BusLoadHistory,
  CommonIssue,
  DiagResult,
  GatewayDiag,
  ModbusError,
  ScheduleTimeline,
  StatsReport,
}

// ============================================================================
// 总线负载
// ============================================================================

export function listBusLoads(): Promise<BusLoad[]> {
  return request<BusLoad[]>({ url: '/api/diagnostics/bus-load', method: 'GET' })
}

export function getBusLoad(bus: string): Promise<BusLoad> {
  return request<BusLoad>({ url: `/api/diagnostics/bus-load/${encodeURIComponent(bus)}`, method: 'GET' })
}

export function refreshBusLoads(): Promise<BusLoad[]> {
  return request<BusLoad[]>({ url: '/api/diagnostics/bus-load/refresh', method: 'POST' })
}

// ============================================================================
// 总线嗅探 / 抓包 —— 已合并到 /api/protocol-tools/packet-capture（见 api/protocolTools）
// ============================================================================

// ============================================================================
// Modbus 异常码速查（静态知识库）
// ============================================================================

export function listModbusErrors(): Promise<ModbusError[]> {
  return request<ModbusError[]>({ url: '/api/diagnostics/modbus-errors', method: 'GET' })
}

export function listCommonIssues(): Promise<CommonIssue[]> {
  return request<CommonIssue[]>({ url: '/api/diagnostics/modbus-errors/issues', method: 'GET' })
}

// ============================================================================
// 采集统计
// ============================================================================

export type StatsPeriod = 'day' | 'week' | 'month'

export function getStatsReport(period: StatsPeriod = 'day'): Promise<StatsReport> {
  return request<StatsReport>({ url: '/api/diagnostics/stats', method: 'GET', params: { period } })
}

// ============================================================================
// 调度时序（F-11：scheduler 真实执行窗口）
// ============================================================================

/** 拉取采集调度真实时序快照（window_ms 缺省 10000，后端钳制到 1s~60s） */
export function getScheduleTimeline(windowMs = 10000): Promise<ScheduleTimeline> {
  return request<ScheduleTimeline>({
    url: '/api/diagnostics/schedule-timeline',
    method: 'GET',
    params: { window_ms: windowMs },
  })
}

// ============================================================================
// 任务诊断
// ============================================================================

export function diagnosePoint(pointId: string): Promise<DiagResult> {
  return request<DiagResult>({
    url: `/api/diagnostics/task-diag/${encodeURIComponent(pointId)}`,
    method: 'POST',
  })
}

// ============================================================================
// 网关自诊断
// ============================================================================

export function getGatewayDiag(): Promise<GatewayDiag> {
  return request<GatewayDiag>({ url: '/api/diagnostics/gateway', method: 'GET' })
}

// ============================================================================
// 日志快照下载（一次性，非 WebSocket）
// ============================================================================

/**
 * 拉取后端日志文件末尾若干行（纯文本）。
 *
 * 与 `/api/diagnostics/logs` 的 WebSocket tail 互补：WS 只能拿到订阅之后的新行，
 * 而现场排障通常是故障之后才来取证，需要"此刻文件里有什么"。
 */
export function downloadLogs(lines = 2000): Promise<string> {
  return request<string>({
    url: '/api/diagnostics/logs/download',
    method: 'GET',
    params: { lines },
    responseType: 'text',
  })
}
