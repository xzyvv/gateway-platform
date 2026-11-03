/**
 * Bug 2.10: 401 单飞登出改为 ref-counted
 *
 * 验证：多个并发 401 触发时，仅第一个走"弹通知+跳登录"路径，其余静默丢弃。
 */
import { describe, it, expect, beforeEach } from 'vitest'
import {
  tryAcquireLoggingOut,
  releaseLoggingOut,
  resetLoggingOut,
  markLoggingOut,
  getLoggingOutCount,
} from './http'

describe('401 单飞登出 — ref-counted (Bug 2.10)', () => {
  beforeEach(() => {
    resetLoggingOut()
  })

  it('首次 tryAcquire 返回 true（触发弹窗/跳登录）', () => {
    expect(tryAcquireLoggingOut()).toBe(true)
  })

  it('并发多次 tryAcquire 仅第一个返回 true', () => {
    const r1 = tryAcquireLoggingOut()
    const r2 = tryAcquireLoggingOut()
    const r3 = tryAcquireLoggingOut()
    expect(r1).toBe(true)
    expect(r2).toBe(false)
    expect(r3).toBe(false)
  })

  it('release 后再次 tryAcquire 返回 true（可重入登出流程）', () => {
    expect(tryAcquireLoggingOut()).toBe(true)
    releaseLoggingOut()
    expect(tryAcquireLoggingOut()).toBe(true)
  })

  it('ref-count 大于 0 时 tryAcquire 不会 reset 回 true', () => {
    expect(tryAcquireLoggingOut()).toBe(true)   // count: 0 -> 1
    expect(tryAcquireLoggingOut()).toBe(false)  // count: 1 -> 2
    expect(tryAcquireLoggingOut()).toBe(false)  // count: 2 -> 3
    releaseLoggingOut()                          // count: 3 -> 2
    expect(tryAcquireLoggingOut()).toBe(false)  // count: 2 -> 3
    releaseLoggingOut()                          // count: 3 -> 2
    releaseLoggingOut()                          // count: 2 -> 1
    releaseLoggingOut()                          // count: 1 -> 0
    expect(tryAcquireLoggingOut()).toBe(true)
  })

  it('release 不应 underflow（count=0 时调 release 不会变负）', () => {
    releaseLoggingOut()
    releaseLoggingOut()
    expect(tryAcquireLoggingOut()).toBe(true)
  })
})

/**
 * W-2：markLoggingOut 旧实现借用 tryAcquireLoggingOut 的"false 分支只自增、
 * 不配对释放"，导致 count 被永久抬高，401 再也不会跳登录页。
 */
describe('W-2 主动登出标记不得泄漏 ref-count', () => {
  beforeEach(() => {
    resetLoggingOut()
  })

  it('已有 401 在登出时反复 markLoggingOut，计数不被抬高', () => {
    expect(tryAcquireLoggingOut()).toBe(true) // count: 0 -> 1
    expect(getLoggingOutCount()).toBe(1)

    markLoggingOut()
    markLoggingOut()
    markLoggingOut()

    // 旧实现在这里会变成 4：markLoggingOut 走了 tryAcquire 的 false 分支，
    // 只自增不释放。计数泄漏后 release 一次也回不到 0 → 401 永远不跳登录页。
    expect(getLoggingOutCount()).toBe(1)

    releaseLoggingOut()
    expect(getLoggingOutCount()).toBe(0)
  })

  it('主动登出后 401 走静默分支，且 acquire/release 严格配对后归零', () => {
    markLoggingOut()
    for (let i = 0; i < 3; i++) {
      expect(tryAcquireLoggingOut()).toBe(false) // 静默丢弃，不弹窗
      releaseLoggingOut()
    }
    // 仍处于主动登出会话中
    expect(tryAcquireLoggingOut()).toBe(false)
    releaseLoggingOut()
  })

  it('登录成功 resetLoggingOut 后恢复"首个 401 触发登出"的能力', () => {
    markLoggingOut()
    expect(tryAcquireLoggingOut()).toBe(false)
    releaseLoggingOut()

    resetLoggingOut() // 等价 auth.login 成功
    expect(tryAcquireLoggingOut()).toBe(true)
    releaseLoggingOut()
  })
})
