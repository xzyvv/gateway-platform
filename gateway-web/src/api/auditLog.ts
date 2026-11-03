/**
 * 后端审计日志 API（与后端 Rust AuditEntry 对齐）
 *
 * 注意：本模块的 AuditLogEntry 字段为 { ts, action, target, detail, source }。
 *
 * 这里是全站唯一审计来源：审计日志只由后端落盘（JSONL），前端仅通过
 * listAuditLogs() 从 GET /api/audit/logs 读取展示；前端本地操作审计（localStorage）
 * 已下线，不再维护第二套记录。
 */
import { request } from './index'

/** 审计日志条目（与后端 AuditEntry 对齐） */
export interface AuditLogEntry {
  /** 时间戳（毫秒 epoch） */
  ts: number
  /** 操作类型，如 "config.update" / "ota.rollback" */
  action: string
  /** 操作对象 */
  target: string
  /** 变更详情 */
  detail: string
  /** 来源/操作人 */
  source: string
}

/** 审计日志查询过滤参数 */
export interface AuditFilter {
  action?: string
  target?: string
  source?: string
  /** 起始时间（毫秒 epoch） */
  ts_from?: number
  /** 截止时间（毫秒 epoch） */
  ts_to?: number
  /** 返回条数上限 */
  limit?: number
}

/** 查询审计日志列表 */
export async function listAuditLogs(filter: AuditFilter = {}): Promise<AuditLogEntry[]> {
  const params = new URLSearchParams()
  if (filter.action) params.set('action', filter.action)
  if (filter.target) params.set('target', filter.target)
  if (filter.source) params.set('source', filter.source)
  if (filter.ts_from != null) params.set('ts_from', String(filter.ts_from))
  if (filter.ts_to != null) params.set('ts_to', String(filter.ts_to))
  if (filter.limit != null) params.set('limit', String(filter.limit))
  const qs = params.toString()
  const url = qs ? `/api/audit/logs?${qs}` : '/api/audit/logs'
  const r = await request<{ logs: AuditLogEntry[] }>({ url, method: 'GET' })
  return r?.logs ?? []
}
