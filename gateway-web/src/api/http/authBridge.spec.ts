/**
 * DD-65：401 登出补丁桥
 *
 * 背景：api/http/responseInterceptor.ts 曾静态引 stores/auth.ts，与 '@/api' barrel
 * 形成静态循环依赖（5 组环的共同根因）。改为由 app/main.ts 注册补丁函数后，
 * 传输层不再依赖 store。本文件锁定该桥的契约：
 *   1. 未注册 patcher 时调用不抛（等价原 try/catch 的「pinia 未激活」容错）；
 *   2. 注册后调用会把「已登出」补丁交给 patcher；
 *   3. patcher 抛异常时被吞掉（不打断 401 登出流程的后续跳登录）。
 */
import { describe, it, expect, vi, beforeEach } from 'vitest'
import { setAuthLogoutPatcher, applyAuthLogoutPatch } from './authBridge'

describe('authBridge — 401 登出补丁桥（DD-65）', () => {
  beforeEach(() => {
    // 每个用例前重置为「未注册」，避免用例间串味
    setAuthLogoutPatcher(null)
  })

  it('未注册 patcher 时调用不抛（pinia 未激活容错）', () => {
    expect(() => applyAuthLogoutPatch()).not.toThrow()
  })

  it('注册后调用把「已登出」补丁交给 patcher', () => {
    const patcher = vi.fn()
    setAuthLogoutPatcher(patcher)

    applyAuthLogoutPatch()

    expect(patcher).toHaveBeenCalledTimes(1)
    expect(patcher).toHaveBeenCalledWith({ loggedIn: false, token: '' })
  })

  it('patcher 抛异常时被吞掉，不打断 401 登出流程', () => {
    setAuthLogoutPatcher(() => {
      throw new Error('pinia 未激活')
    })

    expect(() => applyAuthLogoutPatch()).not.toThrow()
  })
})
