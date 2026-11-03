/**
 * inflight 登记上限测试
 *
 * Bug 2.7：注册中心必须有上界，避免 add/remove 配对失衡导致无界膨胀。
 * W-3 修复后语义变更：触顶时**跳过登记**（不再 abort 最早登记的正常业务请求），
 * 并累加 dropped count 供诊断；被跳过的请求仍会正常完成，只是不受
 * abortAllInflight() 管辖。
 */
import { describe, it, expect, beforeEach } from 'vitest'
import {
  addInflight,
  removeInflight,
  abortAllInflight,
  getInflightCount,
  getInflightDroppedCount,
  onInflightCountChange,
  INFLIGHT_LIMIT,
} from './inflight'

describe('inflight 登记上限（触顶跳过登记，W-3）', () => {
  beforeEach(() => {
    // inflight 是模块级 Map，测试间需重置
    abortAllInflight('test setup')
  })

  it('在飞请求数跟踪', () => {
    expect(getInflightCount()).toBe(0)
    const c1 = new AbortController()
    addInflight(c1)
    expect(getInflightCount()).toBe(1)
    removeInflight(c1)
    expect(getInflightCount()).toBe(0)
  })

  it('超过上限时跳过登记，且不误杀任何在飞请求（W-3）', () => {
    const limit = INFLIGHT_LIMIT
    const droppedBefore = getInflightDroppedCount()
    const ctrls: AbortController[] = []
    for (let i = 0; i < limit; i++) {
      const c = new AbortController()
      addInflight(c)
      ctrls.push(c)
    }
    expect(getInflightCount()).toBe(limit)
    expect(getInflightDroppedCount()).toBe(droppedBefore)

    const last = new AbortController()
    addInflight(last)
    // 计数不增长：新请求未被登记
    expect(getInflightCount()).toBe(limit)
    expect(getInflightDroppedCount()).toBe(droppedBefore + 1)
    // 关键：既不 abort 既有请求，也不 abort 新请求
    expect(ctrls[0]!.signal.aborted).toBe(false)
    expect(last.signal.aborted).toBe(false)

    // 未登记的请求不受 abortAllInflight 影响，但既有的会被取消
    abortAllInflight('test')
    expect(ctrls[0]!.signal.aborted).toBe(true)
    expect(last.signal.aborted).toBe(false)
  })

  it('订阅在飞数量变化', () => {
    const counts: number[] = []
    const unsub = onInflightCountChange((n) => counts.push(n))
    // 初始推送当前值（beforeEach 后应为 0）
    expect(counts[0]).toBe(0)
    const c = new AbortController()
    addInflight(c)
    addInflight(new AbortController())
    expect(counts[counts.length - 1]).toBe(2)
    removeInflight(c)
    expect(counts[counts.length - 1]).toBe(1)
    unsub()
  })

  it('重复 removeInflight 不影响计数', () => {
    const c = new AbortController()
    addInflight(c)
    expect(getInflightCount()).toBe(1)
    removeInflight(c)
    expect(getInflightCount()).toBe(0)
    removeInflight(c) // 重复调用应安全
    expect(getInflightCount()).toBe(0)
  })
})
