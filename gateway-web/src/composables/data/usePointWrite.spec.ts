/**
 * 写值统一入口（E2）契约。
 *
 * 四个写入口共用这里的结果解析：单写 5101 reject 与批量 partial 都必须
 * 收敛成同一结构，页面不再各自猜 `results[0]` 是否存在。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'

const m = vi.hoisted(() => ({ writePoints: vi.fn() }))

vi.mock('@/api/points', () => ({
  writePoints: (entries: unknown) => m.writePoints(entries),
}))

import { usePointWrite } from './usePointWrite'

describe('usePointWrite', () => {
  beforeEach(() => {
    m.writePoints.mockReset()
  })

  it('单点写成功返回实际回读值', async () => {
    m.writePoints.mockResolvedValue({
      status: 'ok',
      total: 1,
      succeeded: 1,
      failed: 0,
      results: [{ point_id: 'p1', status: 'ok', value: 42 }],
    })
    const { writeOnce } = usePointWrite()
    const outcome = await writeOnce('p1', 41)
    expect(outcome).toEqual({ ok: true, status: 'ok', value: 42, error: '' })
  })

  it('单写 5101 reject 被折算为失败结果，不抛异常', async () => {
    m.writePoints.mockRejectedValue(new Error('点位不可写'))
    const { writeOnce } = usePointWrite()
    const outcome = await writeOnce('p1', 1)
    expect(outcome.ok).toBe(false)
    expect(outcome.status).toBe('error')
    expect(outcome.error).toContain('点位不可写')
    expect(outcome.value).toBeNull()
  })

  it('协议层 status=error 保留后端错误原文，与传输错误区分', async () => {
    m.writePoints.mockResolvedValue({
      status: 'partial',
      total: 1,
      succeeded: 0,
      failed: 1,
      results: [{ point_id: 'p1', status: 'error', error: 'adc 超限' }],
    })
    const { writeOnce } = usePointWrite()
    const outcome = await writeOnce('p1', 1)
    expect(outcome.ok).toBe(false)
    expect(outcome.status).toBe('error')
    expect(outcome.error).toBe('adc 超限')
    expect(outcome.value).toBeNull()
  })

  it('批量部分失败保留逐条结果与成功/失败计数', async () => {
    m.writePoints.mockResolvedValue({
      status: 'partial',
      total: 2,
      succeeded: 1,
      failed: 1,
      results: [
        { point_id: 'a', status: 'ok', value: 1 },
        { point_id: 'b', status: 'error', error: 'timeout' },
      ],
    })
    const { writeBatch } = usePointWrite()
    const outcome = await writeBatch([
      { id: 'a', value: 1 },
      { id: 'b', value: 2 },
    ])
    expect(outcome.ok).toBe(false)
    expect(outcome.succeeded).toBe(1)
    expect(outcome.failed).toBe(1)
    expect(outcome.results[1]?.error).toBe('timeout')
  })

  it('批量请求级失败按入参条数补齐 null 结果', async () => {
    m.writePoints.mockRejectedValue(new Error('network down'))
    const { writeBatch } = usePointWrite()
    const outcome = await writeBatch([
      { id: 'a', value: 1 },
      { id: 'b', value: 2 },
    ])
    expect(outcome.failed).toBe(2)
    expect(outcome.results).toEqual([null, null])
    expect(outcome.error).toContain('network down')
  })
})
