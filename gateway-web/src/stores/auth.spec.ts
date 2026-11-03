/**
 * auth store \u5355\u6d4b\uff08Bug 2.8\uff09
 *
 * \u8986\u76d6\uff1a
 * - \u4e09\u6001\u9274\u6743\u72b6\u6001\u673a (unknown/verifying/authenticated/anonymous) \u8f6c\u79fb
 * - login() \u8d70 verifying -> authenticated
 * - logout() \u8d70 anonymous
 * - validate() 401 \u8d70 anonymous\uff0c\u7f51\u7edc\u9519\u8bef\u4fdd\u7559 authenticated
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { createPinia, setActivePinia } from 'pinia'
import { useAuthStore } from './auth'

const { getTokenMock, setTokenMock, clearTokenMock, whoamiMock, loginMock, markLoggingOutMock, resetLoggingOutMock } = vi.hoisted(() => ({
  getTokenMock: vi.fn(),
  setTokenMock: vi.fn(),
  clearTokenMock: vi.fn(),
  whoamiMock: vi.fn(),
  loginMock: vi.fn(),
  markLoggingOutMock: vi.fn(),
  resetLoggingOutMock: vi.fn(),
}))

vi.mock('@/api', () => ({
  getToken: getTokenMock,
  setToken: setTokenMock,
  clearToken: clearTokenMock,
}))
vi.mock('@/api/auth', () => ({
  login: loginMock,
  whoami: whoamiMock,
}))
vi.mock('@/api/http', () => ({
  markLoggingOut: markLoggingOutMock,
  resetLoggingOut: resetLoggingOutMock,
}))
// useSessions \u8fd4\u56de\u5bf9\u8c61\u4e0d\u91cd\u8981\uff0c\u53ea\u9700\u5b58\u5728
vi.mock('../composables/system/useSessions', () => ({
  useSessions: () => ({ markLogin: vi.fn(), markLogout: vi.fn() }),
}))

describe('auth store\uff08Bug 2.8 \u4e09\u6001\u72b6\u6001\u673a\uff09', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    localStorage.removeItem('gateway_api_token')
    vi.clearAllMocks()
  })

  afterEach(() => {
    ;(import.meta.env as { DEV: boolean }).DEV = true
  })

  it('\u521d\u59cb\u72b6\u6001\uff1alocalStorage \u65e0 token \u2192 authStatus=unknown, loggedIn=false', async () => {
    getTokenMock.mockResolvedValue(null)
    const auth = useAuthStore()
    expect(auth.authStatus).toBe('unknown')
    expect(auth.loggedIn).toBe(false)
  })

  it('validate() \u65e0 token \u2192 anonymous, \u8fd4\u56de false', async () => {
    getTokenMock.mockResolvedValue(null)
    const auth = useAuthStore()
    const ok = await auth.validate()
    expect(ok).toBe(false)
    expect(auth.authStatus).toBe('anonymous')
  })

  it('validate() whoami \u6210\u529f \u2192 verifying -> authenticated', async () => {
    getTokenMock.mockResolvedValue('valid-token')
    whoamiMock.mockResolvedValue({
      username: 'admin',
      using_default_password: false,
      security_warnings: [],
    })
    const auth = useAuthStore()
    const ok = await auth.validate()
    expect(ok).toBe(true)
    expect(auth.authStatus).toBe('authenticated')
    expect(auth.username).toBe('admin')
  })

  it('validate() 按 whoami 凭证推导界面角色（主 token=admin，只读密钥=viewer）', async () => {
    getTokenMock.mockResolvedValue('primary-token')
    whoamiMock.mockResolvedValue({
      username: 'admin',
      using_default_password: false,
      security_warnings: [],
      credential: 'primary',
      scopes: ['*'],
    })
    const auth = useAuthStore()
    await auth.validate()
    expect(auth.credential).toBe('primary')
    expect(auth.roleId).toBe('admin')

    whoamiMock.mockResolvedValue({
      username: 'api-client',
      using_default_password: false,
      security_warnings: [],
      credential: 'api_key',
      scopes: ['points:read'],
    })
    await auth.validate()
    expect(auth.credential).toBe('api_key')
    expect(auth.roleId).toBe('viewer')
  })

  it('validate() 401 → anonymous, 返回 false', async () => {
    getTokenMock.mockResolvedValue('expired-token')
    const err = Object.assign(new Error('Unauthorized'), {
      response: { status: 401 },
    })
    whoamiMock.mockRejectedValue(err)
    const auth = useAuthStore()
    const ok = await auth.validate()
    expect(ok).toBe(false)
    expect(auth.authStatus).toBe('anonymous')
    expect(auth.username).toBe('')
  })

  it('validate() \u7f51\u7edc\u9519\u8bef\uff08\u975e 401\uff09\u2192 \u4fdd\u7559 authenticated, \u8fd4\u56de true', async () => {
    getTokenMock.mockResolvedValue('valid-token')
    const err = Object.assign(new Error('Network Error'), {
      response: undefined,
    })
    whoamiMock.mockRejectedValue(err)
    const auth = useAuthStore()
    const ok = await auth.validate()
    expect(ok).toBe(true)
    expect(auth.authStatus, 'authStatus \u5e94\u4fdd\u7559 authenticated').toBe('authenticated')
  })

  it('login() \u540e\u8d70 verifying -> authenticated', async () => {
    loginMock.mockResolvedValue({ token: 'fresh-token' })
    setTokenMock.mockResolvedValue(undefined)
    whoamiMock.mockResolvedValue({
      username: 'admin',
      using_default_password: false,
      security_warnings: [],
    })
    const auth = useAuthStore()
    await auth.login('admin', 'pw')
    expect(auth.authStatus).toBe('authenticated')
    expect(auth.loggedIn).toBe(true)
  })

  it('logout() 走 anonymous', () => {
    const auth = useAuthStore()
    auth.authStatus = 'authenticated'
    auth.loggedIn = true
    auth.logout()
    expect(auth.authStatus).toBe('anonymous')
    expect(auth.loggedIn).toBe(false)
  })

  // ===== Bug-N3 (fix)：login() 失败路径回归 =====

  /// Bug-N3：login() 内部 API 调用抛错时（典型：密码错误 401、网络错误），
  /// authStatus 必须从 'verifying' 恢复到 'anonymous'——否则其他订阅 authStatus 的
  /// 组件（Layout / Sidebar / 路由守卫）会卡在 verifying 状态直到 reload。
  it('n3 login() 内部抛错 → authStatus 回 anonymous，且原异常上抛', async () => {
    const err = Object.assign(new Error('用户名或密码错误'), {
      response: { status: 401 },
    })
    loginMock.mockRejectedValue(err)
    const auth = useAuthStore()
    // 调用方依然能 catch 到原异常（错误信息透传给 Login.vue 用于 ElMessage）
    await expect(auth.login('admin', 'wrong-pw')).rejects.toThrow('用户名或密码错误')
    // 关键不变量：authStatus 必须重置，不能卡 verifying
    expect(auth.authStatus).toBe('anonymous')
    expect(auth.loggedIn).toBe(false)
  })

  /// Bug-N3：login 成功路径上 whoami 抛错（典型：whoami 端点 500 / 网络抖动），
  /// 不应回滚已存的 token / loggedIn（whoami 失败不等于登录失败），
  /// authStatus 仍应为 authenticated。
  /// （这是 fail-soft 设计：whoami 仅用于补全 username，调用方可能已登录成功）
  it('n3 login() whoami 抛错 → authStatus 仍 authenticated, token 保留', async () => {
    loginMock.mockResolvedValue({ token: 'fresh-token' })
    setTokenMock.mockResolvedValue(undefined)
    whoamiMock.mockRejectedValue(new Error('whoami 500'))
    const auth = useAuthStore()
    await auth.login('admin', 'pw')
    expect(auth.authStatus).toBe('authenticated')
    expect(auth.loggedIn).toBe(true)
  })

  /// Bug-N3：login() 抛错后状态被正确清空。**第二次 login()** 重新设置 verifying，
  /// 不应残留上一次的 username/usingDefaultPassword/securityWarnings 等元数据。
  it('n3 login() 抛错后再次 login() 失败 → 内部状态完全归零', async () => {
    const auth = useAuthStore()
    // 模拟上一次成功登录留下的状态
    auth.username = 'prev-user'
    auth.usingDefaultPassword = true
    auth.securityWarnings = ['prev-warn']

    // 当前 login 抛错
    const err = new Error('network error')
    loginMock.mockRejectedValue(err)
    await expect(auth.login('admin', 'pw')).rejects.toThrow('network error')

    // 元数据必须彻底清空（防止下一次连接泄漏前用户身份）
    expect(auth.username).toBe('')
    expect(auth.usingDefaultPassword).toBe(false)
    expect(auth.securityWarnings).toEqual([])
  })
})
