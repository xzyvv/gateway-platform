/**
 * 运行总览「需要关注」信号聚合
 *
 * B1 健康状态单一真源：这里不再按 CPU / 内存 / 队列阈值各算一套，
 * 而是把后端 `/api/health/report` 已经判定好的 `problems` 与 `status`
 * 直接翻译成可点击的异常条目。判定口径（背压阈值、总线离线、MQTT 断链）
 * 只存在于后端 `build_health_problems`，前端只负责「去哪处理」。
 *
 * 设计约束：不新增任何接口调用。健康报告由 useHealthReport 的模块级快照提供，
 * 仪表盘轮询只复用该快照（TTL 内不会重复请求）。
 */
import { computed } from 'vue'
import type { ComputedRef, Ref } from 'vue'
import type { HealthProblem, HealthReport } from '@/api/status'
import type { ConfigIssue } from '@/utils/configHealth'

export type SignalSeverity = 'error' | 'warning'

/**
 * 队列阈值兜底：仅用于仪表盘卡片配色（总览不再据此判定健康）。
 * 健康判定以后端 `gateway_core::queue::limit` / `/api/health/report` 的 problems 为准。
 */
export const THRESHOLDS = {
  queueWarning: 10_000,
  queueError: 100_000,
} as const

export interface HealthSignal {
  /** 稳定 id，用于 key 与测试断言 */
  id: string
  severity: SignalSeverity
  /** 一句话说明发生了什么 */
  title: string
  /** 影响到什么 / 为什么需要处理 */
  detail: string
  /** 处理入口文案 */
  action: string
  /** 处理页路由 */
  to: { path: string; query?: Record<string, string> }
}

interface SignalInput {
  /** 健康聚合报告快照（唯一真源） */
  report: HealthReport | null
  /** 健康探针是否可达；仅用于「网关不可达」条目，不参与组件判定 */
  gatewayOnline?: boolean
  /** 配置体检结果（可选）：配置类问题并非健康聚合，仍单独并入总览 */
  configIssues?: ConfigIssue[]
}

/** 问题码 → 处理入口；未登记的新问题码退化为「查看健康报告」，不吞掉 */
const PROBLEM_ACTION: Record<string, { action: string; to: HealthSignal['to'] }> = {
  MQTT_DISCONNECTED: {
    action: '查看上行服务状态',
    to: { path: '/uplink', query: { tab: 'north-status' } },
  },
  QUEUE_BACKPRESSURE: {
    action: '前往采集诊断',
    to: { path: '/ops', query: { tab: 'task-diagnosis' } },
  },
  QUEUE_CAPACITY_PRESSURE: {
    action: '前往采集诊断',
    to: { path: '/ops', query: { tab: 'task-diagnosis' } },
  },
  QUEUE_BACKLOG: {
    action: '前往采集诊断',
    to: { path: '/ops', query: { tab: 'task-diagnosis' } },
  },
  QUEUE_LOCAL_HISTORY: {
    action: '查看健康报告',
    to: { path: '/ops', query: { tab: 'health-report' } },
  },
  BUS_ALL_OFFLINE: { action: '查看设备管理', to: { path: '/devices' } },
  BUS_PARTIAL_OFFLINE: { action: '查看设备管理', to: { path: '/devices' } },
  BUS_LOW_SUCCESS_RATE: { action: '查看设备管理', to: { path: '/devices' } },
  // 磁盘水位分级：处置入口统一指向「磁盘健康」页签（剩余空间趋势 + 阈值线）
  DISK_SPACE_LOW: {
    action: '查看磁盘健康',
    to: { path: '/ops', query: { tab: 'disk-health' } },
  },
  DISK_SPACE_CRITICAL: {
    action: '查看磁盘健康',
    to: { path: '/ops', query: { tab: 'disk-health' } },
  },
  OTA_IN_PROGRESS: {
    action: '查看固件升级',
    to: { path: '/system', query: { tab: 'ota' } },
  },
}

const DEFAULT_PROBLEM_ACTION = {
  action: '查看健康报告',
  to: { path: '/ops', query: { tab: 'health-report' } },
}

/** 与 useHealthReport 的 overallStatus 同一套中文文案 */
const STATUS_LABEL: Record<HealthReport['status'], string> = {
  ok: '正常',
  degraded: '降级',
  critical: '严重',
}

/**
 * 单条 problem → 信号。
 *
 * 严重度映射：critical → error（面板红色），warning / info → warning（琥珀色）。
 * AttentionPanel 只有两档视觉，info（如 OTA 升级中）仍需要被看见，
 * 因此不静默丢弃，而是按「提醒」展示。
 */
function problemSignal(p: HealthProblem, index: number): HealthSignal {
  const meta = PROBLEM_ACTION[p.code] ?? DEFAULT_PROBLEM_ACTION
  return {
    // 同一 code 会按总线重复（BUS_ALL_OFFLINE 可能有多条），索引保证 key 唯一
    id: `problem-${p.code}-${index}`,
    severity: p.level === 'critical' ? 'error' : 'warning',
    title: p.message,
    detail: p.hint,
    action: meta.action,
    to: meta.to,
  }
}

export function buildHealthSignals({
  report,
  gatewayOnline = true,
  configIssues = [],
}: SignalInput): HealthSignal[] {
  const out: HealthSignal[] = []

  // 探针不可达只代表「页面拿不到实时数据」，与组件健康是两件事，单独一条
  if (!gatewayOnline) {
    out.push({
      id: 'gateway-offline',
      severity: 'error',
      title: '网关健康检查未通过',
      detail: '健康检查未通过，页面数据可能不完整或已停止更新。',
      action: '查看组件健康状态',
      to: { path: '/ops', query: { tab: 'gateway-diag' } },
    })
  }

  for (const [i, p] of (report?.problems ?? []).entries()) {
    out.push(problemSignal(p, i))
  }

  // 只有「状态不 ok 且没有可处置问题」时才补一条状态条目。
  // 典型场景：storage / ota 组件降级，后端没有对应的 problem，若只看 problems
  // 总览会显示一切正常，与健康报告页的「降级」自相矛盾。
  if (report && report.status !== 'ok' && !report.problems.length) {
    const label = STATUS_LABEL[report.status]
    out.push({
      id: 'health-status',
      severity: report.status === 'critical' ? 'error' : 'warning',
      title: `网关健康状态：${label}`,
      detail: `后端健康聚合判定为「${label}」，组件明细与处置建议见健康报告。`,
      action: '查看健康报告',
      to: { path: '/ops', query: { tab: 'health-report' } },
    })
  }

  // 配置体检结果并入总览：配置层面的错误（模板不存在、设备重名、重复地址）在
  // 运行指标上完全看不出来——界面显示 0 点位、一切"正常"，用户只能进体检页才发现。
  // 只取 error/warning（info 属于改进建议，不该占据异常面板），且最多 3 条。
  const cfgProblems = configIssues.filter((i) => i.level === 'error' || i.level === 'warning')
  for (const issue of cfgProblems.slice(0, 3)) {
    out.push({
      id: `cfg-${issue.id}`,
      severity: issue.level === 'error' ? 'error' : 'warning',
      title: `配置问题：${issue.category}`,
      detail: `${issue.target} —— ${issue.description}。${issue.suggestion}`,
      action: '去配置体检',
      to: { path: '/system/config', query: { tab: 'config-health' } },
    })
  }
  if (cfgProblems.length > 3) {
    out.push({
      id: 'cfg-more',
      severity: 'warning',
      title: `另有 ${cfgProblems.length - 3} 项配置问题`,
      detail: '总览只展示最前面的 3 条，完整清单与一键修复在配置体检页。',
      action: '查看全部配置问题',
      to: { path: '/system/config', query: { tab: 'config-health' } },
    })
  }

  // 严重度排序；同级保持后端问题清单顺序（critical → warning → info），
  // 不按文案排序，避免同类条目在不同数值下跳来跳去。
  const rank: Record<SignalSeverity, number> = { error: 0, warning: 1 }
  return out.sort((a, b) => rank[a.severity] - rank[b.severity])
}

/** 供组件消费的响应式派生入口。数据源用 getter 或 ref 均可，内部按惰性读取。 */
export function useHealthSignals(
  report: Readonly<Ref<HealthReport | null>> | (() => HealthReport | null),
  gatewayOnline: Readonly<Ref<boolean>> | (() => boolean) = () => true,
  configIssues: Readonly<Ref<ConfigIssue[]>> | (() => ConfigIssue[]) = () => [],
): ComputedRef<HealthSignal[]> {
  const read = <V,>(v: Readonly<Ref<V>> | (() => V)): V =>
    typeof v === 'function' ? (v as () => V)() : v.value
  return computed(() =>
    buildHealthSignals({
      report: read(report),
      gatewayOnline: read(gatewayOnline),
      configIssues: read(configIssues),
    }),
  )
}
