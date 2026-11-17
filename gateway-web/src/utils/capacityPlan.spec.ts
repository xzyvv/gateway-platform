import { describe, it, expect } from 'vitest'
import {
  rowsPerDay,
  suggestMaxRows,
  aggregatedRowCap,
  aggregateCoverageDays,
  estimateCapacity,
  formatGiB,
  BYTES_PER_RAW_ROW,
  MIN_QUEUE_ROWS,
  MAX_QUEUE_ROWS,
  MIN_AGG_ROWS,
  MAX_AGG_ROWS,
  STORAGE_DAYS,
} from './capacityPlan'

const GB = 1024 ** 3

describe('rowsPerDay', () => {
  it('按 点位数 × 86400÷周期 计算（500 点/5 秒 = 864 万行/天）', () => {
    expect(rowsPerDay(500, 5)).toBe(8_640_000)
  })

  it('出厂模板口径：500 点/10 秒 = 432 万行/天', () => {
    expect(rowsPerDay(500, 10)).toBe(4_320_000)
  })

  it('非法周期/点数为 0 时不产生 Infinity 或 NaN', () => {
    expect(rowsPerDay(100, 0)).toBe(100 * 86_400)
    expect(rowsPerDay(0, 5)).toBe(0)
    expect(Number.isFinite(rowsPerDay(-5, -1))).toBe(true)
  })
})

describe('suggestMaxRows', () => {
  it('500 点/5 秒/24h → 870 万（864 万向上取整到 10 万，见文档 §0）', () => {
    expect(suggestMaxRows({ pointCount: 500, intervalSec: 5, bufferHours: 24 })).toBe(8_700_000)
  })

  it('500 点/10 秒/24h → 440 万（432 万取整，对应出厂模板）', () => {
    expect(suggestMaxRows({ pointCount: 500, intervalSec: 10, bufferHours: 24 })).toBe(4_400_000)
  })

  it('100 点/5 秒/4h → 30 万（28.8 万向上取整到 10 万）', () => {
    expect(suggestMaxRows({ pointCount: 100, intervalSec: 5, bufferHours: 4 })).toBe(300_000)
  })

  it('超过 5000 万上限时钳位', () => {
    expect(suggestMaxRows({ pointCount: 5000, intervalSec: 1, bufferHours: 72 })).toBe(MAX_QUEUE_ROWS)
  })

  it('极小规模也不低于 20 万下限', () => {
    expect(suggestMaxRows({ pointCount: 1, intervalSec: 60, bufferHours: 1 })).toBe(MIN_QUEUE_ROWS)
  })
})

describe('aggregatedRowCap', () => {
  it('与后端同口径：8GB ≈ 178 万行，32GB 触顶 500 万', () => {
    expect(aggregatedRowCap(8 * GB)).toBe(1_789_569)
    expect(aggregatedRowCap(32 * GB)).toBe(MAX_AGG_ROWS)
  })

  it('4GB ≈ 89 万行', () => {
    expect(aggregatedRowCap(4 * GB)).toBe(894_784)
  })

  it('容量探测失败（0）时回退到下限', () => {
    expect(aggregatedRowCap(0)).toBe(MIN_AGG_ROWS)
    expect(aggregatedRowCap(-1)).toBe(MIN_AGG_ROWS)
  })

  it('极小容量不低于下限，极大容量不超上限', () => {
    expect(aggregatedRowCap(1024)).toBe(MIN_AGG_ROWS)
    expect(aggregatedRowCap(1024 * GB)).toBe(MAX_AGG_ROWS)
  })
})

describe('aggregateCoverageDays', () => {
  it('钉子：5000 点 + 32GB 卡，聚合历史只有约 0.7 天', () => {
    expect(aggregateCoverageDays(32 * GB, 5000)).toBeCloseTo(0.69, 1)
  })

  it('点数决定历史长度：32GB 下 100 点远长于 5000 点', () => {
    expect(aggregateCoverageDays(32 * GB, 100)).toBeGreaterThan(aggregateCoverageDays(32 * GB, 500))
    expect(aggregateCoverageDays(32 * GB, 500)).toBeGreaterThan(aggregateCoverageDays(32 * GB, 5000))
  })

  it('磁盘越大覆盖越长，但受 30 天时间界封顶', () => {
    expect(aggregateCoverageDays(16 * GB, 100)).toBeGreaterThan(aggregateCoverageDays(4 * GB, 100))
    expect(aggregateCoverageDays(32 * GB, 100)).toBe(STORAGE_DAYS)
  })

  it('点位数为 0 时返回 30 天而不是 NaN', () => {
    expect(aggregateCoverageDays(8 * GB, 0)).toBe(STORAGE_DAYS)
  })
})

describe('estimateCapacity', () => {
  it('500 点/5 秒/24h：原始库 ≈1.66GB，建议 8GB 卡', () => {
    const est = estimateCapacity({ pointCount: 500, intervalSec: 5, bufferHours: 24 })
    expect(est.rowsPerDay).toBe(8_640_000)
    expect(est.suggestedMaxRows).toBe(8_700_000)
    expect(est.rawBytes).toBe(8_700_000 * BYTES_PER_RAW_ROW)
    expect(est.backupBytes).toBe(est.rawBytes)
    expect(est.suggestedCardBytes).toBe(8 * GB)
  })

  it('提高采集周期显著降低建议卡容量（500 点 5s→10s）', () => {
    const fast = estimateCapacity({ pointCount: 500, intervalSec: 5, bufferHours: 24 })
    const slow = estimateCapacity({ pointCount: 500, intervalSec: 10, bufferHours: 24 })
    expect(slow.suggestedCardBytes).toBeLessThan(fast.suggestedCardBytes)
  })

  it('max_rows 触顶 5000 万后卡容量按钳位后的占用算，不返回 undefined', () => {
    const est = estimateCapacity({ pointCount: 5000, intervalSec: 1, bufferHours: 72 })
    expect(est.suggestedMaxRows).toBe(MAX_QUEUE_ROWS)
    // 钳位后原始库约 9.3 GB ×2（含备份）+ 聚合/固定，32 GB 卡即可满足 75% 口径
    expect(est.suggestedCardBytes).toBe(32 * GB)
  })
})

describe('formatGiB', () => {
  it('按 1024 进制格式化', () => {
    expect(formatGiB(2 * GB)).toBe('2.00 GB')
    expect(formatGiB(1536 ** 1 * 1024 ** 2)).toBe('1.50 GB')
  })
})
