import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'

/**
 * 登录会话 store（前端模拟）的契约。
 *
 * 这里最关键的一条是 **revoke 当前会话**：早期实现只 `clearToken()`，
 * 内存里的 auth store 仍认为已登录、页面也不跳转，用户点了「强制下线」
 * 还能继续操作（请求 401，但界面像没事人一样，即 Bug 180）。
 * 下面的用例锁住「真登出」的四个动作：清 token、清 store、清当前标记、跳登录页。
 *
 * 迁移说明（DD-66）：原为 `composables/system/useSessions.spec.ts`，
 * 随会话能力上移为 Pinia store 迁到 `stores/sessions.spec.ts`。
 */
const mocks = vi.hoisted(() => ({
  getToken: vi.fn(async () => 'tk'),
  clearToken: vi.fn(),
  patch: vi.fn(),
  replace: vi.fn(() => Promise.resolve()),
}))

vi.mock('@/api/index', () => ({ getToken: mocks.getToken, clearToken: mocks.clearToken }))
vi.mock('./auth', () => ({ useAuthStore: () => ({ $patch: mocks.patch }) }))
vi.mock('@/router', () => ({ default: { replace: mocks.replace } }))

import { useSessionsStore } from './sessions'

let clock = 1_700_000_000_000

beforeEach(() => {
  localStorage.clear()
  vi.clearAllMocks()
  clock = 1_700_000_000_000
  vi.spyOn(Date, 'now').mockImplementation(() => clock)
  // 每个用例一个全新的 pinia → store 实例重建，state 不跨用例泄漏
  setActivePinia(createPinia())
})

describe('markLogin', () => {
  it('新登录成为唯一 current，旧会话的 current 被清掉', () => {
    const { markLogin, list } = useSessionsStore()
    const first = markLogin('10.0.0.1')
    clock += 1000
    const second = markLogin('10.0.0.2')

    expect(list()).toHaveLength(2)
    expect(first.current).toBe(false)
    expect(second.current).toBe(true)
    expect(localStorage.getItem('gw_current_session')).toBe(second.id)
  })

  it('超过 20 条时保留最新，淘汰最早的非当前会话', () => {
    const { markLogin, list } = useSessionsStore()
    for (let i = 0; i < 25; i++) {
      clock += 1000
      markLogin()
    }
    const all = list()
    expect(all).toHaveLength(20)
    // 最后一条是 current，且一条都没被误删 current
    expect(all.filter((s) => s.current)).toHaveLength(1)
    expect(all[all.length - 1]!.current).toBe(true)
    // 25 次登录（第 1 次发生在 T+1s，第 25 次在 T+25s），保留最新 20 条 → 最旧为第 6 次
    expect(Math.min(...all.map((s) => s.loginAt))).toBe(1_700_000_000_000 + 6000)
  })

  it('缺省 IP 落成 127.0.0.1，UA 截断到 120 字符', () => {
    const { markLogin } = useSessionsStore()
    const s = markLogin()
    expect(s.ip).toBe('127.0.0.1')
    expect(s.userAgent.length).toBeLessThanOrEqual(120)
  })
})

describe('touch', () => {
  it('只刷新当前会话的 lastActiveAt，其他会话不动', () => {
    const { markLogin, touch, list } = useSessionsStore()
    markLogin('10.0.0.1')
    clock += 1000
    const older = markLogin('10.0.0.2') // older 成为 current
    const oldActive = list()[0]!.lastActiveAt

    clock += 5000
    touch()

    expect(list()[0]!.lastActiveAt).toBe(oldActive)
    expect(list().find((s) => s.id === older.id)!.lastActiveAt).toBe(clock)
  })
})

describe('revoke', () => {
  it('下线非当前会话：只移除记录，不登出', () => {
    const { markLogin, revoke, list } = useSessionsStore()
    const stale = markLogin('10.0.0.1')
    clock += 1000
    const current = markLogin('10.0.0.2')

    revoke(stale.id)

    expect(list().map((s) => s.id)).toEqual([current.id])
    expect(mocks.clearToken).not.toHaveBeenCalled()
    expect(mocks.patch).not.toHaveBeenCalled()
    expect(localStorage.getItem('gw_current_session')).toBe(current.id)
  })

  it('下线当前会话：清 token、清 store、清当前标记并跳登录（Bug 180）', async () => {
    const { markLogin, revoke } = useSessionsStore()
    const current = markLogin('10.0.0.2')

    revoke(current.id)

    expect(mocks.clearToken).toHaveBeenCalledTimes(1)
    expect(mocks.patch).toHaveBeenCalledWith({ loggedIn: false, token: '' })
    expect(localStorage.getItem('gw_current_session')).toBeNull()
    await vi.waitFor(() => expect(mocks.replace).toHaveBeenCalled())
    expect(mocks.replace).toHaveBeenCalledWith({ name: 'Login' })
  })
})

describe('isLoggedIn', () => {
  it('以 token 是否存在为准', async () => {
    const { isLoggedIn } = useSessionsStore()
    await expect(isLoggedIn()).resolves.toBe(true)
    mocks.getToken.mockResolvedValueOnce('')
    await expect(isLoggedIn()).resolves.toBe(false)
  })
})

describe('持久化读取', () => {
  it('丢弃缺 id/loginAt 的脏数据，并归一化缺省字段', async () => {
    localStorage.setItem('gw_sessions', JSON.stringify([
      { id: 'ok', loginAt: 1, ip: '1.1.1.1', userAgent: 'x'.repeat(300) },
      { id: 'no-login-at' },
      { loginAt: 5 },
      null,
    ]))
    vi.resetModules()
    setActivePinia(createPinia())
    const fresh = await import('./sessions')
    const list = fresh.useSessionsStore().list()

    expect(list).toHaveLength(1)
    expect(list[0]).toMatchObject({ id: 'ok', current: false, lastActiveAt: 1 })
    expect(list[0]!.userAgent).toHaveLength(120)
  })
})
