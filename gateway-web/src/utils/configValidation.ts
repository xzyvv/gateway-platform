/**
 * 配置校验（P4：规则单一来源）
 *
 * 校验规则全部由后端 `POST /api/config/validate`（gateway-core
 * `GatewayConfig::validate_report`）提供，前端不再维护阈值/唯一性/引用规则副本，
 * 从根本上消除前后端规则漂移。
 *
 * 本文件只保留展示层职责：
 * - 调用校验接口；
 * - 把字段路径前缀映射为配置页签，供「定位」跳转（UI 概念不进入后端）。
 */
import { validateConfigDraft } from '@/api/config'
import type { GatewayConfig } from '@/api/types'

export interface ConfigIssue {
  level: 'error' | 'warning' | 'info'
  /** 规则标识（后端下发，用于展示层映射） */
  code?: string
  field: string
  message: string
  /** 可点击的定位信息 */
  deviceId?: string
  pointId?: string
  bus?: string
  /** 跳转标签页提示 */
  tab?: string
}

/** 字段路径前缀 -> 配置页签（仅展示层映射，不是校验规则） */
const TAB_BY_PREFIX: ReadonlyArray<readonly [string, string]> = [
  ['buses', 'buses'],
  ['devices', 'devices'],
  ['points', 'points'],
  ['telemetry', 'telemetry'],
]

function tabOf(field: string): string | undefined {
  return TAB_BY_PREFIX.find(([prefix]) => field.startsWith(prefix))?.[1]
}

/**
 * 主入口：调用后端校验，返回带定位信息的问题列表。
 *
 * @param templateNames 模板库模板名清单；给出时后端会校验「设备引用的模板是否存在」，
 *                      为空（模板库不可用）则跳过模板类检查。
 */
export async function validateConfig(
  config: GatewayConfig,
  templateNames?: ReadonlySet<string> | readonly string[],
): Promise<ConfigIssue[]> {
  const templates = templateNames
    ? 'size' in templateNames
      ? [...(templateNames as ReadonlySet<string>)]
      : [...(templateNames as readonly string[])]
    : undefined
  const report = await validateConfigDraft(config, templates)
  return (report.issues ?? []).map((issue) => ({
    ...issue,
    tab: tabOf(issue.field),
  }))
}

/** 是否有阻断性错误（error 级别） */
export function hasBlockingIssues(issues: ConfigIssue[]): boolean {
  return issues.some((i) => i.level === 'error')
}
