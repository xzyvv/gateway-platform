import { ElMessage } from 'element-plus'

import { request } from './index'
import type { GatewayConfig } from './types'

/**
 * 配置保存/重载结果。
 *
 * - `applied`：本次改动中已在线生效的配置分节
 * - `restart_required`：已写入磁盘、但需重启网关才生效的配置分节
 *
 * 旧版后端不返回这两个字段（undefined），调用方需按可选处理。
 */
export interface ConfigReloadResult {
  status?: string
  notes?: string[]
  applied?: string[]
  restart_required?: string[]
}

/** 需重启分节的展示名（后端返回的是配置节 key） */
const RESTART_SECTION_LABELS: Record<string, string> = {
  webhook: 'Webhook',
  opcua: 'OPC UA 服务端',
  modbus_server: 'Modbus 服务端',
  modbus_passthrough: 'Modbus 透传',
  cellular: '蜂窝网络',
  telemetry: '遥测上报',
  self_heal: '自愈（自动重启保护）',
  web: 'Web 服务（监听地址/CORS/限流）',
}

/**
 * 保存/重载后统一提示"哪些配置需重启才生效"。
 *
 * 后端已把变更落盘并返回 `restart_required`；此处只负责让用户看到，
 * 避免"保存成功但行为没变"的困惑。旧版后端无该字段时静默（保持兼容）。
 */
function notifyRestartRequired(result: ConfigReloadResult | undefined): void {
  const sections = (result?.restart_required ?? []).filter(Boolean)
  if (sections.length === 0) return
  const names = sections.map((s) => RESTART_SECTION_LABELS[s] ?? s).join('、')
  ElMessage({
    type: 'warning',
    message: `配置已保存，但 ${names} 需重启网关后生效`,
    duration: 8000,
    showClose: true,
  })
}

/**
 * 后端 `notes` 是「本次热重载有分节未成功」的证据，例如
 * `mqtt reload failed (kept old): new mqtt client failed to connect within 10s`。
 *
 * 此前 notes 被完全丢弃：用户点「保存」后只看到「已保存并热重载」，
 * 但 MQTT 其实还连不上——用户以为配置生效了，现场排查方向被带偏。
 * 这里把 notes 显式提示出来，并说明旧配置仍在运行。
 */
function notifyReloadNotes(result: ConfigReloadResult | undefined): void {
  const notes = (result?.notes ?? []).filter(Boolean)
  if (notes.length === 0) return
  ElMessage({
    type: 'error',
    message: `配置已保存，但有 ${notes.length} 项未生效：${notes.join('；')}`,
    duration: 10000,
    showClose: true,
  })
}

/** 获取网关配置（token 会被脱敏） */
export function getConfig(): Promise<GatewayConfig> {
  return request<GatewayConfig>({ url: '/api/config', method: 'GET' })
}

/** 更新网关配置（需重启的分节会弹提示） */
export async function postConfig(config: GatewayConfig): Promise<ConfigReloadResult> {
  const r = await request<ConfigReloadResult>({ url: '/api/config', method: 'POST', data: config })
  notifyRestartRequired(r)
  notifyReloadNotes(r)
  return r
}

/** 配置校验问题（后端 ConfigValidationIssue） */
export interface ConfigValidationIssue {
  /** error 阻断保存；warning 需提示；info 仅计入体检评分 */
  level: 'error' | 'warning' | 'info'
  /**
   * 规则标识（如 `tpl_missing` / `bus_timeout`），前端据此映射中文类别与修复建议。
   * 旧后端未下发时为空，展示层退化为按字段路径归类。
   */
  code?: string
  /** 字段路径（如 `buses[0].port_name`），无法定位时为空串 */
  field: string
  message: string
}

/** 配置校验报告：规则由后端 gateway-core 统一提供 */
export interface ConfigValidationReport {
  valid: boolean
  errors: number
  warnings: number
  /** 提示级条数（后端新版本下发，旧版本缺省） */
  infos?: number
  issues: ConfigValidationIssue[]
}

/**
 * 提交前校验（只校验、不落盘）。
 *
 * P4：校验规则单一来源在后端（`GatewayConfig::validate_report_with`），
 * 前端不再维护阈值/唯一性/引用规则副本。
 *
 * `templates` 为模板库模板名清单：给出时后端才能判定模板存在性；
 * 不传（或传空）则跳过模板类检查，用于模板库拉取失败时的降级。
 */
export function validateConfigDraft(
  config: GatewayConfig,
  templates?: string[],
): Promise<ConfigValidationReport> {
  const data: { config: GatewayConfig; templates?: string[] } = { config }
  if (templates && templates.length > 0) data.templates = templates
  return request<ConfigValidationReport>({ url: '/api/config/validate', method: 'POST', data })
}

/**
 * 导出配置。
 *
 * 默认返回**脱敏**配置（凭据字段为 `********`），导入同一网关时会被后端还原为原值；
 * 迁移到新网关时需重新填写凭据。`includeSecrets` 仅供主 token 的灾备场景使用，
 * 传 true 时返回明文（API Key 调用会被 403）。
 *
 * D-01：后端已把原 `/api/config/export` 合并进 `GET /api/config`（载荷等价），
 * 本函数仅作为「导出」语义的薄封装保留。
 */
export function exportConfig(includeSecrets = false): Promise<GatewayConfig> {
  return request<GatewayConfig>({
    url: '/api/config',
    method: 'GET',
    params: includeSecrets ? { include_secrets: true } : undefined,
  })
}

/** 导入配置（需重启的分节会弹提示） */
export async function importConfig(config: GatewayConfig): Promise<ConfigReloadResult> {
  const r = await request<ConfigReloadResult>({ url: '/api/config/import', method: 'PUT', data: { config } })
  notifyRestartRequired(r)
  notifyReloadNotes(r)
  return r
}

/** 热重载配置（需重启的分节会弹提示） */
export async function reloadConfig(): Promise<ConfigReloadResult> {
  const r = await request<ConfigReloadResult>({ url: '/api/config/reload', method: 'POST' })
  notifyRestartRequired(r)
  notifyReloadNotes(r)
  return r
}

/** 配置版本列表摘要（后端返回的精简元数据，不含完整 config_json） */
export interface ConfigVersionSummary {
  id: number
  created_at: number
  message: string
  size: number
  is_current: boolean
}

/** 配置版本列表（后端返回 {versions: [...]}，此处解开为数组） */
export async function listConfigVersions(): Promise<ConfigVersionSummary[]> {
  const r = await request<{ versions: ConfigVersionSummary[] }>({ url: '/api/config/versions', method: 'GET' })
  return r?.versions ?? []
}

/** 获取指定版本的配置快照 */
export function getConfigVersion(id: string | number): Promise<GatewayConfig> {
  return request<GatewayConfig>({ url: `/api/config/versions/${id}`, method: 'GET' })
}

/** 回滚到指定版本（需重启的分节会弹提示） */
export async function rollbackConfigVersion(id: string | number): Promise<ConfigReloadResult> {
  const r = await request<ConfigReloadResult>({ url: `/api/config/versions/${id}/rollback`, method: 'POST' })
  notifyRestartRequired(r)
  notifyReloadNotes(r)
  return r
}

/** 配置差异对比（提交配置 vs 当前内存配置） */
export function configDiff(config: GatewayConfig): Promise<{ changed_fields: string[]; has_changes: boolean }> {
  return request({ url: '/api/config/diff', method: 'POST', data: config })
}

/** 手动备份当前配置（在网关侧落一个版本快照，可通过版本历史回滚） */
export interface ConfigBackupResult { status: string; version_id: number; points: number; devices: number }
export function backupConfig(): Promise<ConfigBackupResult> {
  return request({ url: '/api/config/backup', method: 'POST' })
}

/** 两个版本对比结果 */
export interface ConfigVersionMeta {
  changed_fields: string[]
  has_changes: boolean
  from: { id: number; created_at: number; message: string }
  to: { id: number; created_at: number; message: string }
}

/** 两个版本之间的字段级差异对比 */
export function compareConfigVersions(
  id: string | number,
  compareTo: string | number,
): Promise<ConfigVersionMeta> {
  return request({
    url: `/api/config/versions/${id}/compare?compare_to=${compareTo}`,
    method: 'GET',
  })
}