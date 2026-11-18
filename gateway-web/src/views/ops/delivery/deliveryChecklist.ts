/**
 * 交付检查单的组装与导出。
 *
 * 从 `selfCheck.ts` 拆出：检查单是「自检结果 + 人工确认项」拼出来的一张
 * 可导出表格，与六项判定（selfCheckEvals.ts）是两种不同的产物——
 * 前者是给人签字的交付凭证，后者是机器判定的结论。
 */
import type { VersionInfo } from '@/api/types'
import { formatDateTime } from '@/utils/datetime'
import { LEVEL_LABEL, type CheckLevel, type SelfCheckResult } from './selfCheckEvals'

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

/** 未执行自检时，检查单里先占位的六项 id */
const AUTO_CHECK_IDS = ['protocol', 'bus', 'north', 'clock', 'certificate', 'watchdog']

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
    for (const id of AUTO_CHECK_IDS) {
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

/** Markdown 单元格转义：竖线会截断表格列，换行会破坏行结构 */
export function escCell(value: string): string {
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
