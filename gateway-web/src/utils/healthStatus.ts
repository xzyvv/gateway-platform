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
import type { HealthComponent, HealthReady } from '@/api/types'

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

/** 组件状态 → 徽章颜色 */
export function healthStatusTag(status: string): TagType {
  return healthStatusInfo(status).tag
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
 * 健康快照 → 组件表格行。
 *
 * 运行总览页与「系统 → 网关信息」页共用这一份派生，
 * 避免两处各写一遍 Object.entries 映射后口径漂移。
 */
export function healthRowsFrom(health: HealthReady | null | undefined): HealthComponent[] {
  return healthRowsFromComponents(health?.components)
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
