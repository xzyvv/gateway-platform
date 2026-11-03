/**
 * 健康口径唯一化（E4）
 *
 * 现状：同一个指标在多处各写一套阈值——
 *   - 设备综合评分：HealthScore 的圆环 / 进度条 / 文字色 / 等级类名各写一遍 90/70/50；
 *   - 通信评级：CommProfile 的评级函数与成功率进度条各写一遍 98/93/80；
 *   - 网关总线成功率：健康报告表格用 99/50 标色，交付自检用 90 判「提醒」；
 *   - 队列积压：健康报告视觉阈值与交付自检阈值都是 10000。
 *
 * 这里把**同一指标**的阈值收成一个常量 + 一个纯函数；不同指标（综合分 vs
 * 通信评级）保持各自独立的档位，但阈值也只在这里定义一次。
 * 页面只负责渲染，不再各自解释。
 */
import type { TagType } from '@/utils/statusType'

/* ============================== 设备综合健康评分（0–100） ============================== */

/** 分档下界：≥90 优秀 / ≥70 良好 / ≥50 告警 / <50 异常 */
export const DEVICE_SCORE_BANDS = { excellent: 90, good: 70, warning: 50 } as const

export type DeviceScoreLevel = 'excellent' | 'good' | 'warning' | 'critical' | 'unknown'

export const DEVICE_SCORE_LEVEL_LABEL: Record<DeviceScoreLevel, string> = {
  excellent: '优秀',
  good: '良好',
  warning: '告警',
  critical: '异常',
  unknown: '未采集',
}

/** 分数 → 等级（null 表示 24h 内无采集，不能当 0 分） */
export function deviceScoreLevel(score: number | null): DeviceScoreLevel {
  if (score == null) return 'unknown'
  if (score >= DEVICE_SCORE_BANDS.excellent) return 'excellent'
  if (score >= DEVICE_SCORE_BANDS.good) return 'good'
  if (score >= DEVICE_SCORE_BANDS.warning) return 'warning'
  return 'critical'
}

/** 图形填充色（圆环 / 进度条）：直接用主题色（非文字场景可用原色） */
export function deviceScoreColor(score: number | null): string {
  switch (deviceScoreLevel(score)) {
    case 'unknown':
      return 'var(--text-secondary)'
    case 'excellent':
      return 'var(--color-success)'
    case 'good':
      return 'var(--color-primary)'
    case 'warning':
      return 'var(--color-warning)'
    default:
      return 'var(--color-danger)'
  }
}

/** 文字色：--el-color-* 原色在白底对比度不足，文字统一走 --text-* 变体 */
export function deviceScoreTextColor(score: number | null): string {
  switch (deviceScoreLevel(score)) {
    case 'unknown':
      return 'var(--text-secondary)'
    case 'excellent':
      return 'var(--text-success)'
    case 'good':
      return 'var(--color-primary)'
    case 'warning':
      return 'var(--text-warning)'
    default:
      return 'var(--text-danger)'
  }
}

/* ============================== 通信评级（仅按采集成功率） ============================== */

/** 通信评级档位：≥98 优秀 / ≥93 良好 / ≥80 一般 / <80 较差 */
export const COMM_RATING_BANDS = { excellent: 98, good: 93, fair: 80 } as const

export type CommRating = 'excellent' | 'good' | 'fair' | 'poor' | 'unknown'

export function communicationRating(rate: number | null): CommRating {
  if (rate == null) return 'unknown'
  if (rate >= COMM_RATING_BANDS.excellent) return 'excellent'
  if (rate >= COMM_RATING_BANDS.good) return 'good'
  if (rate >= COMM_RATING_BANDS.fair) return 'fair'
  return 'poor'
}

/** 成功率进度条状态：与评级同档，避免「评级优秀但进度条告警」 */
export function communicationRateStatus(
  rate: number | null,
): 'success' | 'warning' | 'exception' {
  const rating = communicationRating(rate)
  if (rating === 'excellent' || rating === 'good') return 'success'
  if (rating === 'fair') return 'warning'
  return 'exception'
}

/* ============================== 网关级健康报告 ============================== */

/** 总线成功率标签色：≥99 绿 / ≥50 黄 / 其余红 */
export const BUS_SUCCESS_RATE_TAG_GOOD = 99
export const BUS_SUCCESS_RATE_TAG_BAD = 50

export function busSuccessTag(rate: number | null): TagType {
  if (rate == null) return 'info'
  if (rate >= BUS_SUCCESS_RATE_TAG_GOOD) return 'success'
  if (rate >= BUS_SUCCESS_RATE_TAG_BAD) return 'warning'
  return 'danger'
}

/** 交付自检：总线成功率低于该值给「提醒」（不阻断交付） */
export const DELIVERY_BUS_SUCCESS_RATE_WARN = 90

/** 队列积压预警阈值：健康报告视觉提示与交付自检共用 */
export const QUEUE_BACKLOG_WARN = 10_000

export function queueBacklogged(pending: number): boolean {
  return pending >= QUEUE_BACKLOG_WARN
}
