/**
 * 读点即时读值（E1）契约。
 *
 * 锁死三件事：后端 200 + status=error 必须算失败、传输错误不抛异常、
 * 延迟由同一处测量；各页面据此决定「在线/离线」「成功/失败」，不再各自解释。
 */
import { describe, it, expect, beforeEach, vi } from 'vitest'

const m = vi.hoisted(() => ({ readPointNow: vi.fn() }))

vi.mock('@/api/points', () => ({
  readPointNow: (id: string) => m.readPointNow(id),
}))

import { useReadPointNow } from './useReadPointNow'

describe('useReadPointNow', () => {
  beforeEach(() => {
    m.readPointNow.mockReset()
  })

  it('status=ok 且有值才算成功', async () => {
    m.readPointNow.mockResolvedValue({ status: 'ok', value: 0, quality: 'good' })
    const { readOnce } = useReadPointNow()
    const outcome = await readOnce('p1')
    expect(outcome.ok).toBe(true)
    expect(outcome.value).toBe(0)
    expect(outcome.error).toBe('')
    expect(outcome.quality).toBe('good')
    expect(outcome.latencyMs).toBeGreaterThanOrEqual(0)
  })

  it('status=error 即使值存在也判定失败（避免把协议失败当在线）', async () => {
    m.readPointNow.mockResolvedValue({ status: 'error', value: 12, error: 'timeout' })
    const { readOnce } = useReadPointNow()
    const outcome = await readOnce('p1')
    expect(outcome.ok).toBe(false)
    expect(outcome.error).toBe('timeout')
    expect(outcome.value).toBeNull()
  })

  it('status=ok 但没有值（空响应）判定失败并给出状态词', async () => {
    m.readPointNow.mockResolvedValue({ status: 'ok' })
    const { readOnce } = useReadPointNow()
    const outcome = await readOnce('p1')
    expect(outcome.ok).toBe(false)
    expect(outcome.error).toContain('status=ok')
  })

  it('传输错误被收敛为 outcome，不向上抛', async () => {
    m.readPointNow.mockRejectedValue(new Error('network down'))
    const { readOnce } = useReadPointNow()
    const outcome = await readOnce('p1')
    expect(outcome.ok).toBe(false)
    expect(outcome.status).toBe('')
    expect(outcome.error).toContain('network down')
    expect(outcome.raw).toBeNull()
  })

  it('协议错误与传输错误可区分（status=error 保留在后端状态词，reject 才置空）', async () => {
    m.readPointNow
      .mockResolvedValueOnce({ status: 'error' })
      .mockRejectedValueOnce(new Error('network down'))
    const { readOnce } = useReadPointNow()

    const protocol = await readOnce('p1')
    expect(protocol.status).toBe('error')
    expect(protocol.error).toContain('采集返回 status=error')

    const transport = await readOnce('p2')
    expect(transport.status).toBe('')
    expect(transport.error).toContain('network down')
  })

  it('readSequential 逐条回调，失败不中断后续', async () => {
    m.readPointNow.mockImplementation((id: string) =>
      id === 'bad'
        ? Promise.resolve({ status: 'error', error: 'no response' })
        : Promise.resolve({ status: 'ok', value: 1 }),
    )
    const seen: string[] = []
    const { readSequential } = useReadPointNow()
    const results = await readSequential(['ok1', 'bad', 'ok2'], {
      onResult: (o) => seen.push(`${o.pointId}:${o.ok}`),
    })
    expect(seen).toEqual(['ok1:true', 'bad:false', 'ok2:true'])
    expect(results.map((r) => r.ok)).toEqual([true, false, true])
    expect(m.readPointNow).toHaveBeenCalledTimes(3)
  })

  it('readSequential 在 shouldStop 后立即停止', async () => {
    m.readPointNow.mockResolvedValue({ status: 'ok', value: 1 })
    const { readSequential } = useReadPointNow()
    let calls = 0
    const results = await readSequential(['a', 'b', 'c'], { shouldStop: () => (calls++ >= 1) })
    expect(results).toHaveLength(1)
    expect(m.readPointNow).toHaveBeenCalledTimes(1)
  })
})
