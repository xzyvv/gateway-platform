/**
 * 健康口径（E4）契约：同一指标的阈值只来自这里。
 */
import { describe, it, expect } from 'vitest'
import {
  deviceScoreLevel,
  deviceScoreColor,
  deviceScoreTextColor,
  communicationRating,
  communicationRateStatus,
  busSuccessTag,
  queueBacklogged,
  QUEUE_BACKLOG_WARN,
} from './healthVerdict'

describe('deviceScoreLevel', () => {
  it.each([
    [100, 'excellent'],
    [90, 'excellent'],
    [89, 'good'],
    [70, 'good'],
    [69, 'warning'],
    [50, 'warning'],
    [49, 'critical'],
    [0, 'critical'],
    [null, 'unknown'],
  ] as const)('%s → %s', (score, level) => {
    expect(deviceScoreLevel(score)).toBe(level)
  })

  it('图形色与文字色按同一等级分档', () => {
    expect(deviceScoreColor(95)).toBe('var(--color-success)')
    expect(deviceScoreTextColor(95)).toBe('var(--text-success)')
    expect(deviceScoreColor(null)).toBe('var(--text-secondary)')
  })
})

describe('communicationRating', () => {
  it.each([
    [100, 'excellent'],
    [98, 'excellent'],
    [97.9, 'good'],
    [93, 'good'],
    [92.9, 'fair'],
    [80, 'fair'],
    [79.9, 'poor'],
    [null, 'unknown'],
  ] as const)('%s → %s', (rate, rating) => {
    expect(communicationRating(rate)).toBe(rating)
  })

  it('进度条状态与评级同档', () => {
    expect(communicationRateStatus(98)).toBe('success')
    expect(communicationRateStatus(93)).toBe('success')
    expect(communicationRateStatus(85)).toBe('warning')
    expect(communicationRateStatus(60)).toBe('exception')
    expect(communicationRateStatus(null)).toBe('exception')
  })
})

describe('网关级口径', () => {
  it('总线成功率标签色 99/50 分档', () => {
    expect(busSuccessTag(99)).toBe('success')
    expect(busSuccessTag(98)).toBe('warning')
    expect(busSuccessTag(50)).toBe('warning')
    expect(busSuccessTag(49)).toBe('danger')
    expect(busSuccessTag(null)).toBe('info')
  })

  it('队列积压阈值唯一且含边界', () => {
    expect(queueBacklogged(QUEUE_BACKLOG_WARN - 1)).toBe(false)
    expect(queueBacklogged(QUEUE_BACKLOG_WARN)).toBe(true)
  })
})
