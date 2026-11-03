/**
 * 配置体检结果适配层（A3：校验规则统一走后端）
 *
 * 规则单源在后端：`POST /api/config/validate` → gateway-core
 * `GatewayConfig::validate_report_with`。完整性、模板存在性、重名、重复地址、
 * 悬空引用、超时越界、性能建议全部由后端判定，前端不再维护规则副本。
 *
 * 本文件只保留展示层职责：
 * - 按后端 `code` 映射中文「类别 / 修复建议」；
 * - 按 `field` 路径解析出可读的「对象」（设备名 / 总线端口 / 点位名）；
 * - 为超时越界提供一键修复（唯一可自动落盘的动作）。
 */
import { postConfig } from '@/api/config'
import type { GatewayConfig } from '@/api/types'
import { validateConfig } from '@/utils/configValidation'

export type IssueLevel = 'error' | 'warning' | 'info'

export interface ConfigIssue {
  id: string
  level: IssueLevel
  category: string
  target: string
  description: string
  suggestion: string
  autoFixable: boolean
  fix?: () => Promise<void>
}

interface Presentation {
  category: string
  suggestion: string
  /** 无法从字段路径解析对象时使用的兜底对象文案 */
  target?: string
}

/** 后端规则 code → 中文类别与修复建议（纯展示文案，与规则判定无关） */
const PRESENTATION: Readonly<Record<string, Presentation>> = {
  cfg_no_bus: {
    category: '完整性',
    target: '总线',
    suggestion: '到「配置管理 → 总线配置」新增第一条总线（选择端口与波特率即可）',
  },
  cfg_no_device: {
    category: '完整性',
    target: '设备',
    suggestion: '到「设备管理」新增设备，或使用「设备发现」自动识别挂载设备',
  },
  cfg_no_point: {
    category: '完整性',
    target: '点位',
    suggestion: '到「点位管理」新增点位，或使用 CSV 批量导入；使用模板的设备请确认模板内含点位',
  },
  tpl_missing: {
    category: '模板不存在',
    suggestion: '改为模板库中已存在的模板（可在「设备模板」页查看当前可用模板）',
  },
  device_dup_name: {
    category: '设备重名',
    suggestion: '把重复的设备改成不同名称（设备名参与点位 ID 生成，必须唯一）',
  },
  device_dup_id: {
    category: '设备 ID 重复',
    suggestion: '重新保存这些设备以生成唯一 ID，或修改配置文件中的 id 字段',
  },
  bus_empty: { category: '总线无设备', suggestion: '新增设备或删除该总线' },
  device_empty: { category: '空设备', suggestion: '新增点位或删除该设备' },
  bus_timeout: { category: '参数越界', suggestion: '将超时调整为 1000ms 以内' },
  bus_busy: { category: '性能建议', suggestion: '为各设备设置不同的采集起始偏移' },
  bus_port_dup: { category: '端口重复', suggestion: '修改其中一条总线的端口名' },
  device_addr_dup: { category: '设备地址重复', suggestion: '修改其中一台设备的地址或从站号' },
  point_addr_dup: { category: '重复地址', suggestion: '修改其中一个点位的寄存器地址' },
  point_device_missing: { category: '引用缺失', suggestion: '把点位挂到已存在的设备上，或先创建该设备' },
  point_bus_missing: { category: '引用缺失', suggestion: '把点位挂到已存在的总线上，或先创建该总线' },
  device_bus_missing: { category: '引用缺失', suggestion: '把设备挂到已存在的总线上，或先创建该总线' },
  device_protocol_mismatch: { category: '协议不一致', suggestion: '确认设备协议与所属总线协议一致' },
  telemetry_sample_ratio: { category: '参数越界', suggestion: '把采样率调整到 0.0~1.0' },
}

function presentationOf(code?: string): Presentation {
  return (
    (code && PRESENTATION[code]) || {
      category: '字段不合法',
      suggestion: '按问题描述修正该字段后重新校验',
    }
  )
}

/** 从字段路径解析「对象」文案：devices[i].x / buses[i].x / points[id].x */
function targetOf(field: string, cfg: GatewayConfig): string {
  if (!field) return ''
  const dev = /^devices\[(\d+)\]/.exec(field)
  if (dev) {
    const d = cfg.devices?.[Number(dev[1])]
    if (d) return d.name || d.id
  }
  const bus = /^buses\[(\d+)\]/.exec(field)
  if (bus) {
    const b = cfg.buses?.[Number(bus[1])]
    if (b) return b.port_name
  }
  const pt = /^points\[([^\]]+)\]/.exec(field)
  if (pt) {
    const p = cfg.points?.find((x) => x.id === pt[1])
    return p ? p.name || p.id : (pt[1] ?? '')
  }
  return field
}

/** 仅在「总线超时越界」上提供一键修复，其余问题需要人工判断。 */
function fixFor(code: string | undefined, field: string, cfg: GatewayConfig): (() => Promise<void>) | undefined {
  if (code !== 'bus_timeout') return undefined
  const idx = Number(/^buses\[(\d+)\]/.exec(field)?.[1] ?? NaN)
  if (!Number.isInteger(idx)) return undefined
  return async () => {
    const bus = cfg.buses?.[idx]
    if (!bus) return
    bus.timeout_ms = 1000
    await postConfig(cfg)
  }
}

/**
 * 拉取后端体检结果并适配为 UI 行。
 *
 * @param cfg 当前网关配置（一键修复会在此对象上就地改字段并回写）
 * @param templateNames 模板库中的模板名集合；为空集合时后端跳过模板类检查
 */
export async function loadConfigIssues(
  cfg: GatewayConfig,
  templateNames: Set<string>,
): Promise<ConfigIssue[]> {
  const issues = await validateConfig(cfg, templateNames)
  return issues.map((i) => {
    const p = presentationOf(i.code)
    const fix = fixFor(i.code, i.field, cfg)
    return {
      id: `${i.code ?? 'issue'}|${i.field}|${i.message}`,
      level: i.level,
      category: p.category,
      target: p.target ?? (targetOf(i.field, cfg) || '全局'),
      description: i.message,
      suggestion: p.suggestion,
      autoFixable: !!fix,
      fix,
    }
  })
}
