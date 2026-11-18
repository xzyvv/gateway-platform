/**
 * 容量规划计算：把「点位数 × 采集周期 × 缓冲时长 / 磁盘容量」换算成
 * `queue.max_rows` 建议、磁盘占用与聚合表覆盖天数。
 *
 * 所有常量与 `docs/容量规划.md` 及后端代码同源，改动时三处需同步：
 * - 单行原始点 200 B：`gateway-core/src/queue/core/limit.rs`（容量上界口径）
 * - 聚合行 96 B / 磁盘 2% / 20 万下限 / 500 万上限：
 *   `gateway-core/src/queue/storage/sqlite/retention.rs`
 * - `max_rows` 范围 20 万 ~ 5000 万：`gateway-core/src/queue/core/limit.rs`
 *
 * 这些是**展示与估算**用的纯函数，不写配置、不发请求；后端才是最终校验者。
 */

/** 单行原始点占用（字节，容量规划口径的保守上界）。 */
export const BYTES_PER_RAW_ROW = 200
/** 单行聚合记录占用（字节）。 */
export const BYTES_PER_AGG_ROW = 96
/** 聚合表占用磁盘的预算比例。 */
export const AGG_DISK_SHARE = 0.02
/** 聚合表行数下限。 */
export const MIN_AGG_ROWS = 200_000
/** 聚合表行数上限。 */
export const MAX_AGG_ROWS = 5_000_000
/** queue.max_rows 合法下限。 */
export const MIN_QUEUE_ROWS = 200_000
/** queue.max_rows 合法上限。 */
export const MAX_QUEUE_ROWS = 50_000_000
/** 聚合表时间维固定保留天数（v2.1 起与 license 无关）。 */
export const STORAGE_DAYS = 30
/** 一天秒数。 */
const SECONDS_PER_DAY = 86_400
/** 一天分钟数（每分钟一个聚合桶）。 */
const MINUTES_PER_DAY = 1_440

/** 容量估算入参。 */
export interface CapacityInputs {
  /** 点位数。 */
  pointCount: number
  /** 采集周期（秒）。 */
  intervalSec: number
  /** 期望断网缓冲时长（小时）。 */
  bufferHours: number
}

/** 容量估算结果。 */
export interface CapacityEstimate {
  /** 每天写入行数。 */
  rowsPerDay: number
  /** 建议 max_rows（向上取整到 10 万，已钳在合法区间）。 */
  suggestedMaxRows: number
  /** 建议 max_rows 下原始库占用（字节）。 */
  rawBytes: number
  /** 建议 max_rows 下备份副本占用（字节，完整文件复制）。 */
  backupBytes: number
  /** 建议卡容量（字节，常见 2/4/8/16/32/64 GB 中满足 ≤75% 占用的最小档）。 */
  suggestedCardBytes: number
}

/** 常见 eMMC / SD 卡容量（字节，从小到大）。 */
const CARD_SIZES_BYTES = [2, 4, 8, 16, 32, 64].map((gb) => gb * 1024 ** 3)

/** 把数字钳在 [min, max] 区间。 */
function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value))
}

/** 正数化：非法/非有限值回退到 fallback。 */
function positive(value: number, fallback: number): number {
  return Number.isFinite(value) && value > 0 ? value : fallback
}

/**
 * 第 1 步：每天写入行数 = 点位数 × (86400 ÷ 采集周期)。
 */
export function rowsPerDay(pointCount: number, intervalSec: number): number {
  return Math.round(positive(pointCount, 0) * (SECONDS_PER_DAY / positive(intervalSec, 1)))
}

/**
 * 第 2 步：建议 max_rows = 写入速率 × 3600 × 缓冲小时数，
 * 向上取整到 10 万，钳在 [20 万, 5000 万]。
 */
export function suggestMaxRows(inputs: CapacityInputs): number {
  const { pointCount, intervalSec, bufferHours } = inputs
  const perDay = rowsPerDay(pointCount, intervalSec)
  const raw = perDay * (positive(bufferHours, 0) / 24)
  const rounded = Math.ceil(raw / 100_000) * 100_000
  return clamp(rounded, MIN_QUEUE_ROWS, MAX_QUEUE_ROWS)
}

/**
 * 聚合表行数上限（自适应）：分区容量 × 2% ÷ 96 B，钳在 [20 万, 500 万]。
 * 与后端 `adaptive_aggregated_row_cap` 同口径。
 */
export function aggregatedRowCap(cardBytes: number): number {
  if (!Number.isFinite(cardBytes) || cardBytes <= 0) return MIN_AGG_ROWS
  const rows = (cardBytes * AGG_DISK_SHARE) / BYTES_PER_AGG_ROW
  return clamp(Math.floor(rows), MIN_AGG_ROWS, MAX_AGG_ROWS)
}

/**
 * #1 聚合表能覆盖多少天（受 30 天时间界封顶）。
 *
 * 每天桶数 = 点位数 × 1440；覆盖天数 = 行数上限 ÷ 每天桶数。
 * 关键结论：**主要由点数决定，不由磁盘决定**——5000 点位给 32 GB 卡也只有约 0.7 天。
 */
export function aggregateCoverageDays(cardBytes: number, pointCount: number): number {
  const points = positive(pointCount, 0)
  if (points === 0) return STORAGE_DAYS
  const bucketsPerDay = points * MINUTES_PER_DAY
  const days = aggregatedRowCap(cardBytes) / bucketsPerDay
  return Math.min(STORAGE_DAYS, days)
}

/**
 * #3 完整容量估算：按点位数/周期/缓冲时长给出 max_rows 与卡容量建议。
 *
 * 卡容量判定口径：五项占用（原始库 + 备份副本 + 聚合库 + WAL 100MB + journald 512MB）
 * 之和 ≤ 卡容量 × 75% 的最小常见卡档；聚合库按该卡容量的 2% 预算估算。
 */
export function estimateCapacity(inputs: CapacityInputs): CapacityEstimate {
  const perDay = rowsPerDay(inputs.pointCount, inputs.intervalSec)
  const suggestedMaxRows = suggestMaxRows(inputs)
  const rawBytes = suggestedMaxRows * BYTES_PER_RAW_ROW
  const backupBytes = rawBytes
  const fixedBytes = 100 * 1024 ** 2 + 512 * 1024 ** 2 // WAL + journald

  const suggestedCardBytes =
    CARD_SIZES_BYTES.find((cardBytes) => {
      const aggBytes = aggregatedRowCap(cardBytes) * BYTES_PER_AGG_ROW
      const total = rawBytes + backupBytes + aggBytes + fixedBytes
      return total <= cardBytes * 0.75
    }) ?? (CARD_SIZES_BYTES[CARD_SIZES_BYTES.length - 1] ?? 64 * 1024 ** 3)

  return { rowsPerDay: perDay, suggestedMaxRows, rawBytes, backupBytes, suggestedCardBytes }
}
