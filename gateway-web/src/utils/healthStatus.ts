/**
 * 网关组件健康状态词表（/health/ready 的 components 字段）。
 *
 * 后端 `gateway-api/src/web/api/system/handlers/status.rs::health_ready`
 * 只会产出这几种状态：
 *   ok / down / backpressure / degraded
 * 前端历史上按 running / error / stopped / starting 这套词判定，两边对不上：
 *   - 判定「未在运行」用的是「非 running 且非 error」，而 ok 从不满足 running
 *     → 系统完全正常时，仪表盘也会挂一条「N 个组件未在运行」的假告警；
 *   - 徽章文字直接透出英文原词（ok / down / degraded），现场看不懂；
 *   - 组件名也全是英文原词（process / mqtt / queue / ota）。
 *
 * 这里把后端真实取值登记为第一等公民（旧词保留兼容），
 * 组件名 / 状态名 / 严重度 / 徽章色在此定义一次，
 * 徽章颜色、仪表盘总览、异常信号聚合共用同一份口径。
 */
import { statusTagType, type TagType } from '@/utils/statusType'
import type { HealthComponent } from '@/api/types'

/** 组件健康严重度：ok 正常 / warning 可服务但不健康 / error 不可用 */
export type HealthSeverity = 'ok' | 'warning' | 'error'

export interface HealthStatusInfo {
  /** 中文展示名 */
  label: string
  /** 严重度：总览计数与异常信号分档用 */
  severity: HealthSeverity
  /** 徽章颜色 */
  tag: TagType
}

/**
 * 状态 → 中文名 + 严重度。
 *
 * 徽章色不在这里写死：颜色统一由全站唯一映射表 utils/statusType 出，
 * 这里只登记它不认识的几个后端专有词。
 */
const STATUS_INFO: Record<string, { label: string; severity: HealthSeverity }> = {
  // 后端真实取值
  ok: { label: '正常', severity: 'ok' },
  down: { label: '不可用', severity: 'error' },
  backpressure: { label: '队列积压', severity: 'warning' },
  degraded: { label: '降级运行', severity: 'warning' },

  // 历史取值（保留兼容：出现这些词时不应整片变灰）
  running: { label: '运行中', severity: 'ok' },
  stopped: { label: '已停止', severity: 'warning' },
  starting: { label: '启动中', severity: 'warning' },
  error: { label: '异常', severity: 'error' },
}

/** 通用映射表不认识的词在这里补色（不重复实现通用规则） */
const TAG_OVERRIDE: Record<string, TagType> = {
  backpressure: 'warning',
  stopped: 'warning',
  starting: 'warning',
}

/**
 * 组件名 → 中文名。
 *
 * 后端 /health/ready 的 components 键是 process / mqtt / queue / ota / storage；
 * 旧表登记的却是 web_server / mqtt_client 等从未出现在该接口上的名字，
 * 导致表格里长期显示英文原名。两套都保留。
 */
const COMPONENT_NAMES: Record<string, string> = {
  // 后端真实组件名
  process: '网关主进程',
  mqtt: 'MQTT 客户端',
  queue: '采集队列',
  ota: 'OTA 升级',
  storage: '存储磁盘',

  // 历史名（保留兼容）
  modbus_tcp_master: 'Modbus TCP 主站',
  modbus_rtu_master: 'Modbus RTU 主站',
  modbus_ascii_master: 'Modbus ASCII 主站',
  web_server: 'Web 服务',
  data_collector: '数据采集器',
  mqtt_client: 'MQTT 客户端',
}

/** 组件名 → 中文名；未登记时原样透出（便于发现后端新增组件） */
export function healthComponentLabel(name: string): string {
  const key = String(name ?? '').trim()
  return COMPONENT_NAMES[key] ?? key
}

/**
 * 组件状态 → 展示信息（中文名 + 严重度 + 徽章色）。
 *
 * 未登记状态：名字原样透出（便于发现后端新增状态），严重度按 warning
 * —— 既不静默吞掉，也不当作致命故障；徽章取琥珀色，与总览/信号聚合的
 * warning 档保持一致，避免表格灰、汇总黄的同页矛盾。
 */
export function healthStatusInfo(status: string): HealthStatusInfo {
  const key = String(status ?? '').trim().toLowerCase()
  const hit = STATUS_INFO[key]
  if (hit) {
    return {
      label: hit.label,
      severity: hit.severity,
      tag: TAG_OVERRIDE[key] ?? statusTagType(key),
    }
  }
  return { label: String(status ?? ''), severity: 'warning', tag: 'warning' }
}

/** 组件状态 → 中文名 */
export function healthStatusLabel(status: string): string {
  return healthStatusInfo(status).label
}

/** 组件状态 → 严重度 */
export function healthSeverity(status: string): HealthSeverity {
  return healthStatusInfo(status).severity
}

/**
 * 一份健康快照里是否有「不健康」的组件。
 *
 * 不直接用后端聚合的 status：它对任何非 ok 组件都判 degraded，
 * 而前端按组件严重度自行判定，口径更细。
 *
 * 没有组件明细时（老版本后端不返回 components）回退到聚合 status。
 */
export function healthSnapshotDegraded(
  components?: Record<string, string> | null,
  overallStatus?: string,
): boolean {
  const entries = Object.entries(components ?? {})
  if (!entries.length) return !!overallStatus && overallStatus !== 'ok'
  return entries.some(([, st]) => healthSeverity(st) !== 'ok')
}

/**
 * components（Record<组件名, 状态词>）→ 组件表格行。
 *
 * 健康报告（`/api/health/report`）与就绪探针（`/health/ready`）共用同一套
 * components 口径，表格只关心这份 map，故单独抽出，避免各页面重复 entries 映射。
 */
export function healthRowsFromComponents(
  components?: Record<string, string> | null,
): HealthComponent[] {
  return Object.entries(components ?? {}).map(([name, status]) => ({ name, status }))
}

/**
 * 一组组件里最严重的级别（error > warning > ok）。
 *
 * 没有组件明细时（老版本后端不返回 components）按后端聚合 status 兜底：
 * 非 ok 记为 warning，不当作致命故障。
 */
export function healthWorstSeverity(
  components?: Record<string, string> | null,
  overallStatus?: string,
): HealthSeverity {
  const entries = Object.entries(components ?? {})
  if (!entries.length) return overallStatus && overallStatus !== 'ok' ? 'warning' : 'ok'
  let worst: HealthSeverity = 'ok'
  for (const [, st] of entries) {
    const sev = healthSeverity(st)
    if (sev === 'error') return 'error'
    if (sev === 'warning') worst = 'warning'
  }
  return worst
}

/* ============================== 网关综合健康状态 ============================== */

/**
 * 后端 `/api/health/report` 的聚合 status 取值。
 *
 * 注意：这是**聚合档位**，与上面的组件级 `HealthSeverity` 是两个层级——
 * 聚合档位只有三档（正常 / 降级 / 严重），组件级才是三档严重度
 * （ok / warning / error）。两者中文名与徽章色在此定义一次，
 * 健康报告页、总览异常信号、结论条共用，避免各写一份字面量副本。
 */
export type OverallHealthValue = 'ok' | 'degraded' | 'critical'

export interface OverallStatusMeta {
  /** 中文展示名 */
  label: string
  /** 徽章色 */
  type: TagType
}

const OVERALL_STATUS_META: Record<OverallHealthValue, OverallStatusMeta> = {
  ok: { label: '正常', type: 'success' },
  degraded: { label: '降级', type: 'warning' },
  critical: { label: '严重', type: 'danger' },
}

/**
 * 聚合状态 → 中文名 + 徽章色。
 *
 * 未知取值（含"还没有快照"）退化为「暂无数据」+ info——与
 * `stores/healthReport.ts` 的 `overallStatus` 初值一致。
 */
export function overallStatusMeta(status: string | null | undefined): OverallStatusMeta {
  const key = String(status ?? '').trim() as OverallHealthValue
  return OVERALL_STATUS_META[key] ?? { label: '暂无数据', type: 'info' }
}

/* ============================== 健康问题分级 ============================== */

/**
 * 后端 `/api/health/report` 的 `problems[].level` 取值。
 */
export type ProblemLevel = 'critical' | 'warning' | 'info'

export interface ProblemLevelMeta {
  /** 中文展示名 */
  label: string
  /** 徽章色 */
  type: TagType
}

const PROBLEM_LEVEL_META: Record<ProblemLevel, ProblemLevelMeta> = {
  critical: { label: '严重', type: 'danger' },
  warning: { label: '警告', type: 'warning' },
  info: { label: '提示', type: 'info' },
}

/**
 * 问题分级 → 中文名 + 徽章色。
 *
 * 徽章色与 `utils/statusType` 的通用表一致（critical→danger、
 * warning→warning、info→info），但中文名是问题域独有的，故在此登记；
 * 健康报告页的问题清单、结论条、协议调试台的探针结论共用这一份。
 */
export function problemLevelMeta(level: string | null | undefined): ProblemLevelMeta {
  const key = String(level ?? '').trim() as ProblemLevel
  return PROBLEM_LEVEL_META[key] ?? { label: '提示', type: 'info' }
}

/** 问题分级计数 */
export interface ProblemCounts {
  critical: number
  warning: number
  info: number
}

/**
 * 问题清单 → 分级计数。
 *
 * 健康报告 store 的 `problemCounts` 与结论条（HealthConclusionBar）
 * 此前各写一遍同样的 reduce，两处会随问题分级口径调整而漂移。
 * 这里抽成纯函数，两边共用同一份实现。
 *
 * @param problems 后端 `problems` 列表；只统计已登记的三档，其余等级忽略
 */
export function countProblemLevels(
  problems: readonly { level: string }[] | null | undefined,
): ProblemCounts {
  const counts: ProblemCounts = { critical: 0, warning: 0, info: 0 }
  for (const p of problems ?? []) {
    if (p.level === 'critical' || p.level === 'warning' || p.level === 'info') counts[p.level] += 1
  }
  return counts
}

/* ============================== 磁盘水位档位 ============================== */

/**
 * 后端 DiskLevel（`/api/health/report` 的 `summary.disk_level`）取值。
 */
export type DiskLevel = 'ok' | 'warn' | 'critical' | 'blocking'

export interface DiskLevelMeta {
  /** 中文展示名 */
  label: string
  /** 徽章色 */
  type: TagType
}

const DISK_LEVEL_META: Record<DiskLevel, DiskLevelMeta> = {
  ok: { label: '正常', type: 'success' },
  warn: { label: '预警', type: 'warning' },
  critical: { label: '严重', type: 'danger' },
  blocking: { label: '阻断', type: 'danger' },
}

/**
 * 磁盘水位档位 → 中文名 + 徽章色。
 *
 * 健康报告页与磁盘健康图表页此前各存一份逐字相同的字典、取色又各写一遍
 * （一个 switch、一个 if 链），改档位文案必然漏改一处。这里定义一次。
 */
export function diskLevelMeta(level: string | null | undefined): DiskLevelMeta {
  const key = String(level ?? '').trim() as DiskLevel
  return DISK_LEVEL_META[key] ?? DISK_LEVEL_META.ok
}
