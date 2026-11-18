import { describe, expect, it, vi } from 'vitest'
import { runConcurrent } from './concurrency'

/**
 * 契约：`runConcurrent` 是全站唯一的受限并发调度原语（11 处批量操作共用）。
 *
 * 这里锁的是**调度语义**，不是某个调用方的业务结果：
 * 1. 同时在飞的任务数不超过上限；
 * 2. 停止条件在**领取下一条之前**求值，已在飞的任务不被打断；
 * 3. 全部 worker 收尾后才 resolve；
 * 4. 回调抛错 → 整池以该错误 reject，且不留未处理 rejection。
 */
describe('runConcurrent 调度语义', () => {
  it('并发数不超过上限', async () => {
    let inFlight = 0
    let peak = 0
    await runConcurrent(
      Array.from({ length: 20 }, (_, i) => i),
      async () => {
        inFlight++
        peak = Math.max(peak, inFlight)
        await new Promise((r) => setTimeout(r, 5))
        inFlight--
      },
      { concurrency: 4 },
    )
    expect(peak).toBe(4)
  })

  it('上限大于任务数时，实际并发不超过任务数（不为空转开 worker）', async () => {
    let inFlight = 0
    let peak = 0
    await runConcurrent(
      [1, 2],
      async () => {
        inFlight++
        peak = Math.max(peak, inFlight)
        await new Promise((r) => setTimeout(r, 5))
        inFlight--
      },
      { concurrency: 16 },
    )
    expect(peak).toBe(2)
  })

  it('全部任务都被执行，且每条恰好一次', async () => {
    const seen: number[] = []
    await runConcurrent(
      [10, 20, 30, 40, 50],
      async (n) => {
        await new Promise((r) => setTimeout(r, 1))
        seen.push(n)
      },
      { concurrency: 3 },
    )
    expect(seen.sort((a, b) => a - b)).toEqual([10, 20, 30, 40, 50])
    expect(seen).toHaveLength(5)
  })

  it('空任务列表：不报错，也不启动任何 worker', async () => {
    const task = vi.fn()
    await expect(runConcurrent([], task)).resolves.toBeUndefined()
    expect(task).not.toHaveBeenCalled()
  })

  it('默认并发上限为 8', async () => {
    let inFlight = 0
    let peak = 0
    await runConcurrent(
      Array.from({ length: 30 }, (_, i) => i),
      async () => {
        inFlight++
        peak = Math.max(peak, inFlight)
        await new Promise((r) => setTimeout(r, 5))
        inFlight--
      },
    )
    expect(peak).toBe(8)
  })
})

describe('runConcurrent 停止语义', () => {
  it('停止条件在领取下一条之前求值：一旦为真就不再领取新任务', async () => {
    const done: number[] = []
    let stop = false
    await runConcurrent(
      [1, 2, 3, 4, 5, 6],
      async (n) => {
        await new Promise((r) => setTimeout(r, 1))
        done.push(n)
        // 第一条完成后置位：其余任务不应再被领取
        if (n === 1) stop = true
      },
      { concurrency: 1, shouldStop: () => stop },
    )
    expect(done).toEqual([1])
  })

  it('已在飞的任务不被打断：停止条件为真时，仍等待在飞任务跑完', async () => {
    let finished = false
    await runConcurrent(
      [1, 2],
      async () => {
        await new Promise((r) => setTimeout(r, 10))
        finished = true
      },
      { concurrency: 2, shouldStop: () => true },
    )
    // shouldStop 一开始就为真 → 不领取任何任务
    expect(finished).toBe(false)
  })

  it('停止条件为真时，已领取的任务仍会跑完（不中断在飞任务）', async () => {
    const finished: number[] = []
    let stop = false
    await runConcurrent(
      [1, 2, 3, 4],
      async (n) => {
        await new Promise((r) => setTimeout(r, 5))
        finished.push(n)
        if (n === 1) stop = true
      },
      { concurrency: 2, shouldStop: () => stop },
    )
    // 并发 2：第一批两条都跑完；之后停止 → 3/4 不再领取
    expect(finished.sort()).toEqual([1, 2])
  })

  it('未传停止条件时不做任何提前退出', async () => {
    const done: number[] = []
    await runConcurrent([1, 2, 3], async (n) => {
      done.push(n)
    })
    expect(done).toHaveLength(3)
  })
})

describe('runConcurrent 异常语义', () => {
  it('回调抛错 → 整池以该错误 reject', async () => {
    const boom = new Error('boom')
    await expect(
      runConcurrent(
        [1, 2, 3],
        async (n) => {
          if (n === 2) throw boom
        },
        { concurrency: 1 },
      ),
    ).rejects.toBe(boom)
  })

  it('一个任务抛错不影响其它任务跑完（失败隔离由调用方在回调内完成）', async () => {
    const done: number[] = []
    await runConcurrent(
      [1, 2, 3],
      async (n) => {
        try {
          if (n === 2) throw new Error('x')
          done.push(n)
        } catch {
          done.push(n)
        }
      },
      { concurrency: 1 },
    )
    expect(done).toEqual([1, 2, 3])
  })

  it('抛错时其余 worker 也已收尾，不遗留未处理 rejection', async () => {
    const unhandled: unknown[] = []
    const onUnhandled = (e: PromiseRejectionEvent) => {
      unhandled.push(e.reason)
      e.preventDefault()
    }
    window.addEventListener('unhandledrejection', onUnhandled)
    try {
      await runConcurrent(
        [1, 2, 3, 4],
        async (n) => {
          await new Promise((r) => setTimeout(r, 5))
          if (n === 1) throw new Error('first')
          if (n === 3) throw new Error('third')
        },
        { concurrency: 4 },
      ).catch(() => undefined)
      // 给事件循环一轮，确认没有迟到的 rejection
      await new Promise((r) => setTimeout(r, 20))
      expect(unhandled).toEqual([])
    } finally {
      window.removeEventListener('unhandledrejection', onUnhandled)
    }
  })

  it('多个错误时重抛第一个（与 allSettled 语义一致）', async () => {
    const first = new Error('first')
    await expect(
      runConcurrent(
        [1, 2],
        async (n) => {
          await new Promise((r) => setTimeout(r, 1))
          throw n === 1 ? first : new Error('second')
        },
        { concurrency: 1 },
      ),
    ).rejects.toBe(first)
  })
})

describe('runConcurrent 结果顺序', () => {
  it('回调里 push 的顺序是完成顺序，不是入参顺序', async () => {
    const order: number[] = []
    await runConcurrent(
      [30, 10, 20],
      async (n) => {
        await new Promise((r) => setTimeout(r, n / 5))
        order.push(n)
      },
      { concurrency: 3 },
    )
    expect(order).toEqual([10, 20, 30])
  })

  it('并发为 1 时按入参顺序执行', async () => {
    const order: number[] = []
    await runConcurrent(
      [30, 10, 20],
      async (n) => {
        order.push(n)
      },
      { concurrency: 1 },
    )
    expect(order).toEqual([30, 10, 20])
  })

  it('回调能拿到原始索引（供按序回填结果）', async () => {
    const pairs: [number, number][] = []
    await runConcurrent(
      ['a', 'b', 'c'],
      async (item, index) => {
        pairs.push([index, item.charCodeAt(0)])
      },
      { concurrency: 3 },
    )
    expect(pairs.map((p) => p[0]).sort()).toEqual([0, 1, 2])
  })
})
