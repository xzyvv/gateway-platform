/**
 * 退避口径（E3）契约。
 *
 * 三类计数与「设备最后成功时间」只在这里算一次；退避监控页、趋势页、
 * 设备列表复用同一函数，避免出现「监控说 3 个退避、列表说 2 个」。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import type { PointBackoff } from '@/api/dataQuality'

const m = vi.hoisted(() => ({ listPointBackoff: vi.fn() }))

vi.mock('@/api/dataQuality', () => ({
  listPointBackoff: () => m.listPointBackoff(),
}))

import { summarizeBackoff, lastGoodByDevice, fetchPointBackoffOnce, usePointBackoff } from './usePointBackoff'

function row(patch: Partial<PointBackoff>): PointBackoff {
  return {
    point_id: 'p',
    point_name: 'p',
    device_id: 'd1',
    device_name: 'd1',
    bus: 'bus1',
    base_interval_ms: 1000,
    current_interval_ms: 1000,
    backoff_multiplier: 1,
    consecutive_failures: 0,
    stale_count: 0,
    last_good_value: null,
    last_good_ts: 0,
    quality: 'good',
    backing_off: false,
    ...patch,
  }
}

describe('summarizeBackoff', () => {
  it('按退避中 / 失败未退避 / 正常三分类计数且互斥', () => {
    const stats = summarizeBackoff([
      row({ point_id: 'a' }),
      row({ point_id: 'b', consecutive_failures: 2 }),
      row({ point_id: 'c', consecutive_failures: 5, backing_off: true }),
    ])
    expect(stats).toEqual({ total: 3, backingOff: 1, failing: 1, normal: 1 })
  })
})

describe('lastGoodByDevice', () => {
  it('取设备下各点位最大时间', () => {
    const map = lastGoodByDevice(
      [
        row({ point_id: 'a', device_id: 'd1', last_good_ts: 100 }),
        row({ point_id: 'b', device_id: 'd1', last_good_ts: 200 }),
        row({ point_id: 'c', device_id: 'd2', last_good_ts: 0 }),
      ],
      ['d1', 'd2'],
    )
    expect(map).toEqual({ d1: 200 })
  })

  it('device_id 为空时按“设备ID_”前缀反解，不猜则跳过', () => {
    const map = lastGoodByDevice(
      [
        row({ point_id: 'dev1_temp', device_id: '', last_good_ts: 300 }),
        row({ point_id: 'orphan', device_id: '', last_good_ts: 900 }),
      ],
      ['dev1'],
    )
    expect(map).toEqual({ dev1: 300 })
  })
})

describe('fetchPointBackoffOnce', () => {
  beforeEach(() => m.listPointBackoff.mockReset())

  it('同刻并发只发一次请求', async () => {
    m.listPointBackoff.mockResolvedValue([row({})])
    const [a, b] = await Promise.all([fetchPointBackoffOnce(), fetchPointBackoffOnce()])
    expect(a).toBe(b)
    expect(m.listPointBackoff).toHaveBeenCalledTimes(1)
  })

  it('请求结束后下一次重新拉取（失败也不粘住错误）', async () => {
    m.listPointBackoff.mockRejectedValueOnce(new Error('offline'))
    await expect(fetchPointBackoffOnce()).rejects.toThrow('offline')
    m.listPointBackoff.mockResolvedValue([row({})])
    await expect(fetchPointBackoffOnce()).resolves.toHaveLength(1)
    expect(m.listPointBackoff).toHaveBeenCalledTimes(2)
  })
})

describe('usePointBackoff.load', () => {
  it('传输失败收敛为 error 字段，loading 复位且不抛', async () => {
    m.listPointBackoff.mockImplementation(() => Promise.reject(new Error('network down')))
    const { points, loading, error, stats, load } = usePointBackoff()

    await expect(load()).resolves.toBeUndefined()

    expect(error.value).toContain('network down')
    expect(loading.value).toBe(false)
    expect(points.value).toEqual([])
    expect(stats.value).toEqual({ total: 0, backingOff: 0, failing: 0, normal: 0 })
  })

  it('空列表是正常态而非错误（不把「无退避点位」误报为失败）', async () => {
    m.listPointBackoff.mockResolvedValue([])
    const { points, loading, error, load } = usePointBackoff()

    await load()

    expect(error.value).toBe('')
    expect(loading.value).toBe(false)
    expect(points.value).toEqual([])
  })
})
