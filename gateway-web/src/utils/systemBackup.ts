/**
 * 系统完整备份/还原
 * 打包配置 + 审计日志为 JSON，支持下载与从文件还原
 *
 * 审计字段接收后端 /api/audit/logs 的日志（B3：前端本地审计已下线，不再有第二份来源）。
 */
import type { GatewayConfig } from '@/api/types'
import type { AuditLogEntry } from '@/api/auditLog'
import { triggerDownload } from './exportCsv'

export interface SystemBackup {
  version: string
  createdAt: number
  gatewayVersion?: string
  config: GatewayConfig
  /** 后端审计日志快照（ts/action/target/detail/source） */
  auditLogs: AuditLogEntry[]
  meta: {
    busCount: number
    deviceCount: number
    pointCount: number
  }
}

export function buildBackup(
  config: GatewayConfig,
  auditLogs: AuditLogEntry[],
  gatewayVersion?: string,
): SystemBackup {
  return {
    version: '1.0',
    createdAt: Date.now(),
    gatewayVersion,
    config: JSON.parse(JSON.stringify(config)),
    auditLogs,
    meta: {
      busCount: (config.buses || []).length,
      deviceCount: config.devices?.length || 0,
      pointCount: config.points?.length || 0,
    },
  }
}

export function downloadBackup(backup: SystemBackup) {
  const blob = new Blob([JSON.stringify(backup, null, 2)], { type: 'application/json' })
  triggerDownload(blob, `gateway-backup-${new Date().toISOString().slice(0, 10)}.json`)
}

export async function parseBackupFile(file: File): Promise<SystemBackup> {
  let text: string
  try {
    text = await file.text()
  } catch (e) {
    throw new Error('读取文件失败: ' + ((e as Error)?.message || String(e)), { cause: e })
  }
  let data: SystemBackup
  try {
    data = JSON.parse(text) as SystemBackup
  } catch (e) {
    throw new Error('备份文件不是有效的 JSON: ' + ((e as Error)?.message || String(e)), { cause: e })
  }
  // 结构校验：避免脏数据导致下游崩溃
  if (!data || typeof data !== 'object') throw new Error('备份文件格式错误')
  if (!data.config || typeof data.config !== 'object') throw new Error('备份文件缺少 config 字段')
  if (!data.version || typeof data.version !== 'string') throw new Error('备份文件缺少 version 字段')
  return data
}

