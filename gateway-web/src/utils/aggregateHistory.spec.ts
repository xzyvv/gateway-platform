import { describe, it, expect } from 'vitest'
import { aggregateHistory } from './aggregateHistory'

/**
 * DD-54：后端降采样桶已是加权均值，二次聚合必须按 sample_count 加权，
 * 否则就是「平均的平均」，均值会被采样稀疏的桶拉偏。
 */
describe('aggregateHistory — 后端降采样桶的加权聚合', () => {
  const HOUR = 3600_000

  it('按 sample_count 加权求均值，而不是各桶均值简单平均', () => {
    const points = [
      // 同一小时桶内：100 个样本均值 10，1 个样本均值 40
      { timestamp: HOUR + 1, value: 10, sample_count: 100 },
      { timestamp: HOUR + 2, value: 40, sample_count: 1 },
    ]
    const [agg] = aggregateHistory(points, 'hour')
    expect(agg!.count).toBe(101)
    // 简单平均会是 25；加权应为 (10*100 + 40*1) / 101 ≈ 10.297
    expect(agg!.avg).toBeCloseTo((10 * 100 + 40) / 101, 6)
  })

  it('count 为样本数之和，极值取后端桶内真实极值', () => {
    const points = [
      { timestamp: HOUR, value: 5, sample_count: 3, min_value: 1, max_value: 9 },
      { timestamp: HOUR + 1, value: 6, sample_count: 2, min_value: 4, max_value: 8 },
    ]
    const [agg] = aggregateHistory(points, 'hour')
    expect(agg!.count).toBe(5)
    expect(agg!.min).toBe(1)
    expect(agg!.max).toBe(9)
  })

  it('原始点（无 sample_count）按 1 计，行为与旧实现一致', () => {
    const points = [
      { timestamp: HOUR, value: 2 },
      { timestamp: HOUR + 1, value: 4 },
    ]
    const [agg] = aggregateHistory(points, 'hour')
    expect(agg!.count).toBe(2)
    expect(agg!.avg).toBe(3)
    expect(agg!.min).toBe(2)
    expect(agg!.max).toBe(4)
  })

  it('raw 粒度保留后端桶的样本数与真实极值', () => {
    const points = [{ timestamp: 123, value: 7, sample_count: 8, min_value: 3, max_value: 11 }]
    const [agg] = aggregateHistory(points, 'raw')
    expect(agg).toMatchObject({ bucket: 123, count: 8, min: 3, max: 11, avg: 7, stddev: 0 })
  })

  it('方差按样本数加权且不为负数', () => {
    const points = [
      { timestamp: HOUR, value: 10, sample_count: 100 },
      { timestamp: HOUR + 1, value: 40, sample_count: 1 },
    ]
    const [agg] = aggregateHistory(points, 'hour')
    // 加权总体方差：Σw(v-μ)² / Σw
    const mean = (10 * 100 + 40) / 101
    const variance = (100 * (10 - mean) ** 2 + (40 - mean) ** 2) / 101
    expect(agg!.stddev).toBeCloseTo(Math.sqrt(variance), 6)
    expect(agg!.stddev).toBeGreaterThan(0)
  })
})
